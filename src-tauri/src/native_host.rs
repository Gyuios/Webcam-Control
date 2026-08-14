use crate::{
    assets::{binary_path, CAMERA_HOST_BINARY},
    coordinator::acquire_camera_lease,
    diagnostics, run_blocking,
};
use camera_domain::LeasePurpose;
use camera_protocol::{
    BackendKind, CameraDescriptor, CaptureProbeResult, FilterGraph, FilterPluginManifest,
    HostCommand, HostResponse, RequestEnvelope, ResponseEnvelope, ScalingMode, VideoFormat,
    PROTOCOL_VERSION,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::Mutex,
    thread::{self, JoinHandle},
};
use tauri::Manager;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Default)]
pub(crate) struct NativeHostState {
    process: Mutex<Option<NativeHostProcess>>,
}

pub(crate) struct NativeCaptureOptions {
    pub camera_id: String,
    pub format: VideoFormat,
    pub output_width: u32,
    pub output_height: u32,
    pub output_pixel_format: camera_protocol::PixelFormat,
    pub scaling: ScalingMode,
    pub frame_path: String,
    pub filter_graph: FilterGraph,
    pub lut_assets: BTreeMap<String, String>,
    pub plugins: Vec<FilterPluginManifest>,
}

struct NativeHostProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr_reader: Option<JoinHandle<()>>,
    next_request_id: u64,
}

enum ExchangeError {
    Transport(String),
    Application(String),
}

impl ExchangeError {
    fn into_message(self) -> String {
        match self {
            Self::Transport(message) | Self::Application(message) => message,
        }
    }
}

impl Drop for NativeHostState {
    fn drop(&mut self) {
        if let Ok(process) = self.process.get_mut() {
            stop_process(process);
        }
    }
}

fn start_process(app: &tauri::AppHandle) -> Result<NativeHostProcess, String> {
    let path = binary_path(app, CAMERA_HOST_BINARY)?;
    let mut child = Command::new(path)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("No se pudo iniciar camera-host: {error}"))?;
    let pid = child.id();
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "camera-host no expuso su entrada IPC".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "camera-host no expuso su salida IPC".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "camera-host no expuso su diagnóstico".to_string())?;
    let stderr_reader = thread::Builder::new()
        .name("camera-host-stderr".into())
        .spawn(move || {
            for line in BufReader::new(stderr).lines() {
                match line {
                    Ok(line) => diagnostics::ingest_external_line("camera.host", &line),
                    Err(error) => {
                        diagnostics::log(
                            "error",
                            "camera.host",
                            "stderr.read_failed",
                            &error.to_string(),
                            Value::Null,
                        );
                        break;
                    }
                }
            }
        })
        .map_err(|error| format!("No se pudo vigilar camera-host: {error}"))?;
    diagnostics::log(
        "info",
        "camera.host",
        "process.started",
        "Host nativo Media Foundation iniciado.",
        json!({ "pid": pid, "protocolVersion": PROTOCOL_VERSION }),
    );
    Ok(NativeHostProcess {
        child,
        stdin,
        stdout: BufReader::new(stdout),
        stderr_reader: Some(stderr_reader),
        next_request_id: 0,
    })
}

fn stop_process(process: &mut Option<NativeHostProcess>) {
    if let Some(mut process) = process.take() {
        let pid = process.child.id();
        let _ = process.child.kill();
        let status = process.child.wait().ok().map(|value| value.to_string());
        if let Some(reader) = process.stderr_reader.take() {
            let _ = reader.join();
        }
        diagnostics::log(
            "info",
            "camera.host",
            "process.stopped",
            "Host nativo Media Foundation detenido.",
            json!({ "pid": pid, "status": status }),
        );
    }
}

fn exchange(
    process: &mut NativeHostProcess,
    command: HostCommand,
) -> Result<HostResponse, ExchangeError> {
    process.next_request_id = process.next_request_id.wrapping_add(1).max(1);
    let request = RequestEnvelope::new(process.next_request_id, command);
    serde_json::to_writer(&mut process.stdin, &request).map_err(|error| {
        ExchangeError::Transport(format!(
            "No se pudo serializar la solicitud nativa: {error}"
        ))
    })?;
    process
        .stdin
        .write_all(b"\n")
        .and_then(|()| process.stdin.flush())
        .map_err(|error| {
            ExchangeError::Transport(format!("No se pudo enviar la solicitud nativa: {error}"))
        })?;

    let mut line = String::new();
    let bytes = process.stdout.read_line(&mut line).map_err(|error| {
        ExchangeError::Transport(format!("No se pudo leer camera-host: {error}"))
    })?;
    if bytes == 0 {
        return Err(ExchangeError::Transport(
            "camera-host se cerró sin responder".into(),
        ));
    }
    if line.len() > camera_protocol::MAX_MESSAGE_BYTES {
        return Err(ExchangeError::Transport(
            "camera-host devolvió un mensaje demasiado grande".into(),
        ));
    }
    let response: ResponseEnvelope<HostResponse> =
        serde_json::from_str(&line).map_err(|error| {
            ExchangeError::Transport(format!("camera-host devolvió JSON inválido: {error}"))
        })?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(ExchangeError::Transport(format!(
            "camera-host usa protocolo {}, pero la aplicación espera {}",
            response.protocol_version, PROTOCOL_VERSION
        )));
    }
    if response.request_id != request.request_id {
        return Err(ExchangeError::Transport(
            "camera-host respondió a otra solicitud".into(),
        ));
    }
    response
        .result
        .map_err(|error| ExchangeError::Application(error.message))
}

fn request(app: &tauri::AppHandle, command: HostCommand) -> Result<HostResponse, String> {
    let state = app.state::<NativeHostState>();
    let mut process = state
        .process
        .lock()
        .map_err(|_| "camera-host quedó en un estado inválido".to_string())?;
    for attempt in 0..2 {
        let exited = process
            .as_mut()
            .and_then(|running| running.child.try_wait().ok().flatten())
            .is_some();
        if exited {
            stop_process(&mut process);
        }
        if process.is_none() {
            *process = Some(start_process(app)?);
        }
        match exchange(
            process.as_mut().expect("camera-host was just started"),
            command.clone(),
        ) {
            Ok(response) => return Ok(response),
            Err(ExchangeError::Application(error)) => return Err(error),
            Err(error) if attempt == 1 => return Err(error.into_message()),
            Err(ExchangeError::Transport(error)) => {
                diagnostics::log(
                    "warn",
                    "camera.host",
                    "request.retrying",
                    &error,
                    json!({ "attempt": attempt + 1 }),
                );
                stop_process(&mut process);
            }
        }
    }
    unreachable!()
}

pub(crate) fn enumerate_formats(
    app: &tauri::AppHandle,
    camera_id: &str,
) -> Result<Vec<VideoFormat>, String> {
    match request(
        app,
        HostCommand::EnumerateFormats {
            device_id: camera_id.to_string(),
        },
    )? {
        HostResponse::Formats(formats) => Ok(formats),
        _ => Err("camera-host devolvió una respuesta inesperada".into()),
    }
}

pub(crate) fn open_capture(
    app: &tauri::AppHandle,
    options: NativeCaptureOptions,
) -> Result<(), String> {
    let NativeCaptureOptions {
        camera_id,
        format,
        output_width,
        output_height,
        output_pixel_format,
        scaling,
        frame_path,
        filter_graph,
        lut_assets,
        plugins,
    } = options;
    match request(
        app,
        HostCommand::Open {
            device_id: camera_id,
            backend: BackendKind::MediaCapture,
            format,
            output_width,
            output_height,
            output_pixel_format,
            scaling,
            frame_path,
            filter_graph,
            lut_assets,
            plugins,
        },
    )? {
        HostResponse::Acknowledged => Ok(()),
        _ => Err("camera-host devolvió una respuesta inesperada".into()),
    }
}

pub(crate) fn close_capture(app: &tauri::AppHandle) -> Result<(), String> {
    match request(app, HostCommand::Close)? {
        HostResponse::Acknowledged => Ok(()),
        _ => Err("camera-host devolvió una respuesta inesperada".into()),
    }
}

pub(crate) fn set_filter_graph(
    app: &tauri::AppHandle,
    filter_graph: FilterGraph,
) -> Result<(), String> {
    match request(app, HostCommand::SetFilterGraph { filter_graph })? {
        HostResponse::Acknowledged => Ok(()),
        _ => Err("camera-host devolvió una respuesta inesperada".into()),
    }
}

pub(crate) fn set_lut_asset(
    app: &tauri::AppHandle,
    asset_id: String,
    cube: Option<String>,
) -> Result<(), String> {
    match request(app, HostCommand::SetLutAsset { asset_id, cube })? {
        HostResponse::Acknowledged => Ok(()),
        _ => Err("camera-host devolvió una respuesta inesperada".into()),
    }
}

#[tauri::command]
pub(crate) async fn list_native_cameras(
    app: tauri::AppHandle,
) -> Result<Vec<CameraDescriptor>, String> {
    run_blocking(
        move || match request(&app, HostCommand::EnumerateDevices)? {
            HostResponse::Devices(devices) => Ok(devices),
            _ => Err("camera-host devolvió una respuesta inesperada".into()),
        },
    )
    .await
}

#[tauri::command]
pub(crate) async fn list_native_formats(
    app: tauri::AppHandle,
    camera_id: String,
) -> Result<Vec<VideoFormat>, String> {
    run_blocking(move || {
        let _lease = acquire_camera_lease(&app, &camera_id, LeasePurpose::Diagnostics)?;
        match request(
            &app,
            HostCommand::EnumerateFormats {
                device_id: camera_id,
            },
        )? {
            HostResponse::Formats(formats) => Ok(formats),
            _ => Err("camera-host devolvió una respuesta inesperada".into()),
        }
    })
    .await
}

#[tauri::command]
pub(crate) async fn probe_source_reader(
    app: tauri::AppHandle,
    camera_id: String,
    format: VideoFormat,
    frames: u32,
) -> Result<CaptureProbeResult, String> {
    run_blocking(move || {
        let _lease = acquire_camera_lease(&app, &camera_id, LeasePurpose::Diagnostics)?;
        match request(
            &app,
            HostCommand::ProbeSourceReader {
                device_id: camera_id,
                format,
                frames,
            },
        )? {
            HostResponse::CaptureProbe(result) => {
                diagnostics::log(
                    "info",
                    "camera.host",
                    "source_reader.probe_completed",
                    "Prueba de captura Media Foundation completada.",
                    json!({
                        "backend": result.backend,
                        "width": result.format.width,
                        "height": result.format.height,
                        "pixelFormat": result.format.pixel_format,
                        "requestedFrames": result.requested_frames,
                        "receivedFrames": result.received_frames,
                        "firstFrameMillis": result.first_frame_millis,
                        "elapsedMillis": result.elapsed_millis,
                        "streamFlags": result.stream_flags,
                    }),
                );
                Ok(result)
            }
            _ => Err("camera-host devolvió una respuesta inesperada".into()),
        }
    })
    .await
}

#[tauri::command]
pub(crate) async fn probe_media_frame_reader(
    app: tauri::AppHandle,
    camera_id: String,
    format: VideoFormat,
    frames: u32,
) -> Result<CaptureProbeResult, String> {
    run_blocking(move || {
        let _lease = acquire_camera_lease(&app, &camera_id, LeasePurpose::Diagnostics)?;
        match request(
            &app,
            HostCommand::ProbeMediaFrameReader {
                device_id: camera_id,
                format,
                frames,
            },
        )? {
            HostResponse::CaptureProbe(result) => {
                diagnostics::log(
                    "info",
                    "camera.host",
                    "media_frame_reader.probe_completed",
                    "Prueba de captura MediaFrameReader completada.",
                    json!({
                        "backend": result.backend,
                        "width": result.format.width,
                        "height": result.format.height,
                        "pixelFormat": result.format.pixel_format,
                        "requestedFrames": result.requested_frames,
                        "receivedFrames": result.received_frames,
                        "firstFrameMillis": result.first_frame_millis,
                        "elapsedMillis": result.elapsed_millis,
                    }),
                );
                Ok(result)
            }
            _ => Err("camera-host devolvió una respuesta inesperada".into()),
        }
    })
    .await
}
