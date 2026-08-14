#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod coordinator;
mod diagnostics;
mod models;
mod native_host;

use assets::{binary_path, BRIDGE_BINARY, MEDIA_SOURCE_BINARY, VIRTUAL_CAMERA_CONTROL_BINARY};
use camera_domain::{CameraLease, LeasePurpose};
use camera_frame::{FrameReader, FrameSnapshot};
use camera_protocol::{
    canonical_device_id, FilterGraph, FilterPluginManifest, PixelFormat, ScalingMode, VideoFormat,
};
use coordinator::{acquire_camera_lease, release_camera_lease, CoordinatorState};
use models::{
    Camera, CameraControl, DiagnosticsInfo, FilterPluginCatalog, FrontendLogEntry,
    PreviewStartResult, VirtualCameraStatus, VirtualOutputOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    env, fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::Mutex,
    thread,
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tauri::{
    ipc::Response,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;
const VIRTUAL_CAMERA_NAME: &str = "CameraTuner Virtual Camera";

#[derive(Default)]
struct BridgeState {
    process: Mutex<Option<BridgeProcess>>,
}

struct BridgeProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr_reader: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct PreviewSession {
    active: bool,
    frame_reader: Option<FrameReader>,
    frame_path: Option<PathBuf>,
    last_sequence: u64,
    last_frame_at: Option<Instant>,
    lease: Option<CameraLease>,
}

#[derive(Default)]
struct PreviewState {
    session: Mutex<PreviewSession>,
}

#[derive(Default)]
struct ProcessingState {
    graph: Mutex<FilterGraph>,
    lut_assets: Mutex<BTreeMap<String, String>>,
    plugins: Mutex<Vec<FilterPluginManifest>>,
}

#[derive(Default)]
struct VirtualOutputSession {
    active: bool,
    frame_path: Option<PathBuf>,
    width: u32,
    height: u32,
    lease: Option<CameraLease>,
}

#[derive(Default)]
struct VirtualOutputState {
    session: Mutex<VirtualOutputSession>,
}

impl Drop for PreviewState {
    fn drop(&mut self) {
        if let Ok(session) = self.session.get_mut() {
            stop_preview_session(session);
        }
    }
}

impl Drop for BridgeState {
    fn drop(&mut self) {
        if let Ok(process) = self.process.get_mut() {
            stop_bridge_process(process);
        }
    }
}

impl Drop for VirtualOutputState {
    fn drop(&mut self) {
        if let Ok(session) = self.session.get_mut() {
            stop_virtual_output_session(session);
        }
    }
}

#[derive(Deserialize)]
struct BridgeResult {
    ok: bool,
    error: Option<String>,
}

#[derive(Serialize)]
struct BridgeRequest<'a> {
    command: &'a str,
    args: &'a [String],
}

enum BridgeExchangeError {
    Transport(String),
    Application(String),
}

fn bridge_context(command: &str, args: &[String]) -> Value {
    match command {
        "set" => json!({
            "command": command,
            "cameraId": "[redacted]",
            "kind": args.get(1),
            "property": args.get(2),
            "value": args.get(3),
            "automatic": args.get(4),
        }),
        "controls" => json!({ "command": command, "cameraId": "[redacted]" }),
        "property-page" => json!({ "command": command, "cameraId": "[redacted]" }),
        _ => json!({ "command": command, "argumentCount": args.len() }),
    }
}

pub(crate) async fn run_blocking<T, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| {
            diagnostics::log(
                "error",
                "rust",
                "blocking_task.join_failed",
                &error.to_string(),
                Value::Null,
            );
            format!("La operación interna se interrumpió: {error}")
        })?
}

fn start_bridge_process(path: &Path) -> Result<BridgeProcess, String> {
    let mut child = Command::new(path)
        .arg("serve")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("No se pudo iniciar el motor de cámara: {error}"))?;
    let pid = child.id();
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "No se pudo abrir la entrada del motor de cámara.".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "No se pudo abrir la salida del motor de cámara.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "No se pudo abrir el diagnóstico del motor de cámara.".to_string())?;
    let stderr_reader = thread::Builder::new()
        .name("bridge-stderr".to_string())
        .spawn(move || {
            for line in BufReader::new(stderr).lines() {
                match line {
                    Ok(line) => diagnostics::ingest_external_line("directshow.bridge", &line),
                    Err(error) => {
                        diagnostics::log(
                            "error",
                            "directshow.bridge",
                            "stderr.read_failed",
                            &error.to_string(),
                            Value::Null,
                        );
                        break;
                    }
                }
            }
        })
        .map_err(|error| format!("No se pudo vigilar el diagnóstico del motor: {error}"))?;
    diagnostics::log(
        "info",
        "directshow.bridge",
        "process.started",
        "Motor DirectShow iniciado.",
        json!({ "pid": pid }),
    );
    Ok(BridgeProcess {
        child,
        stdin,
        stdout: BufReader::new(stdout),
        stderr_reader: Some(stderr_reader),
    })
}

fn stop_bridge_process(process: &mut Option<BridgeProcess>) {
    if let Some(mut process) = process.take() {
        let pid = process.child.id();
        let _ = process.child.kill();
        let status = process.child.wait().ok().map(|value| value.to_string());
        if let Some(reader) = process.stderr_reader.take() {
            let _ = reader.join();
        }
        diagnostics::log(
            "info",
            "directshow.bridge",
            "process.stopped",
            "Motor DirectShow detenido.",
            json!({ "pid": pid, "status": status }),
        );
    }
}

fn bridge_exchange(
    process: &mut BridgeProcess,
    request: &BridgeRequest<'_>,
) -> Result<String, BridgeExchangeError> {
    serde_json::to_writer(&mut process.stdin, request).map_err(|error| {
        BridgeExchangeError::Transport(format!("No se pudo enviar la solicitud al motor: {error}"))
    })?;
    process
        .stdin
        .write_all(b"\n")
        .and_then(|()| process.stdin.flush())
        .map_err(|error| {
            BridgeExchangeError::Transport(format!(
                "No se pudo enviar la solicitud al motor: {error}"
            ))
        })?;
    let mut response = String::new();
    let count = process.stdout.read_line(&mut response).map_err(|error| {
        BridgeExchangeError::Transport(format!("No se pudo leer la respuesta del motor: {error}"))
    })?;
    if count == 0 {
        return Err(BridgeExchangeError::Transport(
            "El motor de cámara se cerró inesperadamente.".to_string(),
        ));
    }
    let response = response.trim().to_owned();
    if response.is_empty() {
        return Err(BridgeExchangeError::Transport(
            "El motor de cámara no devolvió información.".to_string(),
        ));
    }
    if let Ok(result) = serde_json::from_str::<BridgeResult>(&response) {
        if !result.ok {
            return Err(BridgeExchangeError::Application(
                result
                    .error
                    .unwrap_or_else(|| "El controlador rechazó la operación.".to_string()),
            ));
        }
    }
    Ok(response)
}

fn bridge(app: &tauri::AppHandle, args: &[String]) -> Result<String, String> {
    let (command, parameters) = args
        .split_first()
        .ok_or_else(|| "La operación del motor está vacía.".to_string())?;
    let request = BridgeRequest {
        command,
        args: parameters,
    };
    let context = bridge_context(command, parameters);
    let started = Instant::now();
    diagnostics::log(
        "debug",
        "directshow.bridge",
        "request.started",
        "Solicitud enviada al motor DirectShow.",
        context.clone(),
    );
    let binary = binary_path(app, BRIDGE_BINARY)?;
    let state = app.state::<BridgeState>();
    let mut process = state
        .process
        .lock()
        .map_err(|_| "El motor de cámara quedó en un estado inválido.".to_string())?;

    for attempt in 0..2 {
        let exited = process
            .as_mut()
            .and_then(|running| running.child.try_wait().ok().flatten())
            .is_some();
        if exited {
            stop_bridge_process(&mut process);
        }
        if process.is_none() {
            *process = Some(start_bridge_process(&binary)?);
        }
        match bridge_exchange(process.as_mut().expect("bridge was just started"), &request) {
            Ok(response) => {
                diagnostics::log(
                    "debug",
                    "directshow.bridge",
                    "request.completed",
                    "Solicitud DirectShow completada.",
                    json!({ "request": context.clone(), "durationMs": started.elapsed().as_millis(), "attempt": attempt + 1 }),
                );
                return Ok(response);
            }
            Err(BridgeExchangeError::Application(message)) => {
                diagnostics::log(
                    "error",
                    "directshow.bridge",
                    "request.rejected",
                    &message,
                    json!({ "request": context.clone(), "durationMs": started.elapsed().as_millis(), "attempt": attempt + 1 }),
                );
                return Err(message);
            }
            Err(BridgeExchangeError::Transport(message)) if attempt == 1 => {
                diagnostics::log(
                    "error",
                    "directshow.bridge",
                    "request.transport_failed",
                    &message,
                    json!({ "request": context.clone(), "durationMs": started.elapsed().as_millis(), "attempt": attempt + 1 }),
                );
                return Err(message);
            }
            Err(BridgeExchangeError::Transport(message)) => {
                diagnostics::log(
                    "warn",
                    "directshow.bridge",
                    "request.retrying",
                    &message,
                    json!({ "request": context.clone(), "attempt": attempt + 1 }),
                );
            }
        }
        stop_bridge_process(&mut process);
    }
    unreachable!()
}

fn virtual_camera_control(
    app: &tauri::AppHandle,
    action: &str,
    media_source: Option<&Path>,
) -> Result<String, String> {
    let started = Instant::now();
    diagnostics::log(
        "info",
        "virtual_camera.control",
        "action.started",
        "Operación del componente de cámara virtual iniciada.",
        json!({ "action": action }),
    );
    let mut command = Command::new(binary_path(app, VIRTUAL_CAMERA_CONTROL_BINARY)?);
    command.arg(action);
    if let Some(path) = media_source {
        command.arg(path);
    }
    let output = command
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("No se pudo ejecutar el componente de cámara virtual: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if output.status.success() {
        diagnostics::log(
            "info",
            "virtual_camera.control",
            "action.completed",
            "Operación del componente de cámara virtual completada.",
            json!({ "action": action, "durationMs": started.elapsed().as_millis(), "status": output.status.code() }),
        );
        Ok(stdout)
    } else {
        let message = if stderr.is_empty() { stdout } else { stderr };
        diagnostics::log(
            "error",
            "virtual_camera.control",
            "action.failed",
            &message,
            json!({ "action": action, "durationMs": started.elapsed().as_millis(), "status": output.status.code() }),
        );
        Err(message)
    }
}

fn get_camera_list(app: &tauri::AppHandle) -> Result<Vec<Camera>, String> {
    let mut cameras: Vec<Camera> = serde_json::from_str(&bridge(app, &["list".into()])?)
        .map_err(|error| format!("El motor devolvió una lista de cámaras inválida: {error}"))?;
    for camera in &mut cameras {
        camera.backend_id = camera.id.clone();
        camera.id = canonical_device_id(&camera.backend_id);
    }
    Ok(cameras)
}

fn resolve_backend_camera_id(app: &tauri::AppHandle, camera_id: &str) -> Result<String, String> {
    get_camera_list(app)?
        .into_iter()
        .find(|camera| camera.id == canonical_device_id(camera_id))
        .map(|camera| camera.backend_id)
        .ok_or_else(|| "La cámara seleccionada ya no está disponible.".to_string())
}

fn remove_if_present(path: Option<&Path>) {
    if let Some(path) = path {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
}

fn stop_preview_session(session: &mut PreviewSession) {
    session.active = false;
    session.frame_reader = None;
    session.frame_path = None;
    session.last_sequence = 0;
    session.last_frame_at = None;
    release_camera_lease(&mut session.lease);
}

fn stop_native_preview(app: &tauri::AppHandle, session: &mut PreviewSession) {
    if session.active {
        if let Err(error) = native_host::close_capture(app) {
            diagnostics::log(
                "warn",
                "media_capture.preview",
                "session.close_failed",
                &error,
                Value::Null,
            );
        }
        diagnostics::log(
            "info",
            "media_capture.preview",
            "session.stopped",
            "Vista previa nativa detenida.",
            Value::Null,
        );
    }
    stop_preview_session(session);
}

fn preferred_preview_format(formats: &[VideoFormat]) -> Option<VideoFormat> {
    formats
        .iter()
        .filter(|format| {
            format.width > 0
                && format.height > 0
                && format.fps_numerator > 0
                && format.fps_denominator > 0
                && format.pixel_format != PixelFormat::Unknown
        })
        .min_by_key(|format| {
            let fps = format.fps_numerator as f64 / format.fps_denominator as f64;
            let resolution_priority = if format.width == 640 && format.height == 360 {
                0
            } else if format.width == 640 && format.height == 480 {
                1
            } else if format.width == 1280 && format.height == 720 {
                2
            } else if format.width == 1920 && format.height == 1080 {
                3
            } else {
                4
            };
            let fps_priority = if (24.0..=30.5).contains(&fps) { 0 } else { 1 };
            let pixel_priority = match format.pixel_format {
                PixelFormat::Nv12 => 0,
                PixelFormat::Yuy2 => 1,
                PixelFormat::Mjpeg => 2,
                PixelFormat::H264 => 3,
                PixelFormat::Bgra => 4,
                PixelFormat::Unknown => 5,
            };
            (
                resolution_priority,
                fps_priority,
                pixel_priority,
                format.width.saturating_mul(format.height),
            )
        })
        .cloned()
}

fn preferred_output_input_format(
    formats: &[VideoFormat],
    output_width: u32,
    output_height: u32,
) -> Option<VideoFormat> {
    let output_aspect = output_width as f64 / output_height.max(1) as f64;
    let output_pixels = output_width.saturating_mul(output_height);
    formats
        .iter()
        .filter(|format| {
            format.width > 0
                && format.height > 0
                && format.fps_numerator > 0
                && format.fps_denominator > 0
                && format.pixel_format != PixelFormat::Unknown
                && ((format.width as f64 / format.height as f64) - output_aspect).abs() < 0.02
        })
        .min_by_key(|format| {
            let fps = format.fps_numerator as f64 / format.fps_denominator as f64;
            let exact_size =
                u8::from(format.width != output_width || format.height != output_height);
            let fps_distance = ((fps - 30.0).abs() * 100.0) as u32;
            let pixel_priority = match format.pixel_format {
                PixelFormat::Nv12 => 0,
                PixelFormat::Yuy2 => 1,
                PixelFormat::Mjpeg => 2,
                PixelFormat::H264 => 3,
                PixelFormat::Bgra => 4,
                PixelFormat::Unknown => 5,
            };
            (
                exact_size,
                format
                    .width
                    .saturating_mul(format.height)
                    .abs_diff(output_pixels),
                fps_distance,
                pixel_priority,
            )
        })
        .cloned()
}

fn requested_capture_format(
    formats: &[VideoFormat],
    requested: &VideoFormat,
) -> Result<VideoFormat, String> {
    formats
        .iter()
        .find(|available| *available == requested)
        .cloned()
        .ok_or_else(|| {
            "El modo seleccionado ya no está disponible en esta cámara. Vuelve a elegirlo en Controles de la cámara."
                .to_string()
        })
}

fn preview_output_dimensions(format: &VideoFormat) -> (u32, u32) {
    const MAX_PREVIEW_WIDTH: u32 = 960;
    if format.width <= MAX_PREVIEW_WIDTH {
        return (format.width, format.height);
    }
    let height =
        ((format.height as u64 * MAX_PREVIEW_WIDTH as u64) / format.width as u64).max(1) as u32;
    (MAX_PREVIEW_WIDTH, height)
}

fn encode_preview_jpeg(snapshot: FrameSnapshot) -> Result<Vec<u8>, String> {
    let metadata = snapshot.metadata;
    let mut rgb = Vec::with_capacity(
        metadata
            .width
            .saturating_mul(metadata.height)
            .saturating_mul(3) as usize,
    );
    for pixel in snapshot.bytes.chunks_exact(4) {
        rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
    }
    let (encoded, width, height) = if metadata.width > 960 {
        let image = image::RgbImage::from_raw(metadata.width, metadata.height, rgb)
            .ok_or_else(|| "El cuadro BGRA no coincide con sus dimensiones.".to_string())?;
        let height = ((metadata.height as u64 * 960) / metadata.width as u64).max(1) as u32;
        let resized =
            image::imageops::resize(&image, 960, height, image::imageops::FilterType::Triangle);
        let width = resized.width();
        let height = resized.height();
        (resized.into_raw(), width, height)
    } else {
        (rgb, metadata.width, metadata.height)
    };
    let mut output = Vec::with_capacity((width * height / 4) as usize);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 74)
        .encode(&encoded, width, height, image::ExtendedColorType::Rgb8)
        .map_err(|error| format!("No se pudo codificar la vista previa: {error}"))?;
    Ok(output)
}

fn frame_exchange_path() -> PathBuf {
    env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("CameraTuner")
        .join("frame-v3.bin")
}

fn stop_virtual_output_session(session: &mut VirtualOutputSession) {
    session.active = false;
    session.frame_path = None;
    session.width = 0;
    session.height = 0;
    release_camera_lease(&mut session.lease);
}

fn stop_native_virtual_output(app: &tauri::AppHandle, session: &mut VirtualOutputSession) {
    if session.active {
        if let Err(error) = native_host::close_capture(app) {
            diagnostics::log(
                "warn",
                "media_capture.virtual_output",
                "session.close_failed",
                &error,
                Value::Null,
            );
        }
        diagnostics::log(
            "info",
            "media_capture.virtual_output",
            "session.stopped",
            "Salida virtual nativa detenida.",
            Value::Null,
        );
    }
    stop_virtual_output_session(session);
}

fn validate_output_options(options: &VirtualOutputOptions) -> Result<(), String> {
    const ALLOWED: &[(u32, u32)] = &[
        (640, 360),
        (640, 480),
        (1280, 720),
        (1920, 1080),
        (2560, 1440),
        (3840, 2160),
    ];
    if !ALLOWED.contains(&(options.width, options.height)) {
        return Err("La resolución seleccionada no está soportada.".to_string());
    }
    if !matches!(options.quality.as_str(), "none" | "fast" | "quality" | "ai") {
        return Err("El modo de reescalado no es válido.".to_string());
    }
    if options.quality == "ai" {
        return Err(
            "El backend Windows ML todavía no está instalado. Usa Alta calidad hasta preparar el modelo ONNX."
                .to_string(),
        );
    }
    Ok(())
}

fn stop_preview_locked(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<PreviewState>();
    let mut session = state
        .session
        .lock()
        .map_err(|_| "No se pudo detener la vista previa.".to_string())?;
    stop_native_preview(app, &mut session);
    Ok(())
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[tauri::command]
async fn list_cameras(app: tauri::AppHandle) -> Result<Vec<Camera>, String> {
    run_blocking(move || {
        let cameras = get_camera_list(&app)?;
        diagnostics::log(
            "info",
            "camera",
            "enumeration.completed",
            "Enumeración de cámaras completada.",
            json!({ "count": cameras.len(), "names": cameras.iter().map(|camera| &camera.name).collect::<Vec<_>>() }),
        );
        Ok(cameras)
    })
    .await
}

#[tauri::command]
async fn get_controls(
    app: tauri::AppHandle,
    camera_id: String,
) -> Result<Vec<CameraControl>, String> {
    run_blocking(move || {
        let preview_active = app
            .state::<PreviewState>()
            .session
            .lock()
            .map_err(|_| "No se pudo comprobar la vista previa.".to_string())?
            .active;
        let virtual_output_active = app
            .state::<VirtualOutputState>()
            .session
            .lock()
            .map_err(|_| "No se pudo comprobar la salida virtual.".to_string())?
            .active;
        if preview_active || virtual_output_active {
            return Err("Detén la captura antes de consultar controles de la cámara.".to_string());
        }
        let backend_id = resolve_backend_camera_id(&app, &camera_id)?;
        let _lease = acquire_camera_lease(&app, &camera_id, LeasePurpose::ReadControls)?;
        let controls: Vec<CameraControl> =
            serde_json::from_str(&bridge(&app, &["controls".into(), backend_id])?)
                .map_err(|error| format!("El motor devolvió controles inválidos: {error}"))?;
        diagnostics::log(
            "info",
            "camera.controls",
            "enumeration.completed",
            "Lectura de controles completada.",
            json!({ "count": controls.len(), "controls": controls.iter().map(|control| &control.id).collect::<Vec<_>>() }),
        );
        Ok(controls)
    })
    .await
}

#[tauri::command]
async fn set_control(
    app: tauri::AppHandle,
    camera_id: String,
    kind: String,
    property: i32,
    value: i32,
    automatic: bool,
) -> Result<(), String> {
    run_blocking(move || {
        let preview_active = app
            .state::<PreviewState>()
            .session
            .lock()
            .map_err(|_| "No se pudo comprobar la vista previa.".to_string())?
            .active;
        if preview_active {
            return Err(
                "La vista previa debe detenerse antes de cambiar controles de esta cámara."
                    .to_string(),
            );
        }
        let virtual_output_active = app
            .state::<VirtualOutputState>()
            .session
            .lock()
            .map_err(|_| "No se pudo comprobar la salida virtual.".to_string())?
            .active;
        if virtual_output_active {
            return Err(
                "Detén la salida virtual antes de cambiar controles de la cámara.".to_string(),
            );
        }
        let backend_id = resolve_backend_camera_id(&app, &camera_id)?;
        let _lease = acquire_camera_lease(&app, &camera_id, LeasePurpose::WriteControl)?;
        let output = bridge(
            &app,
            &[
                "set".into(),
                backend_id,
                kind.clone(),
                property.to_string(),
                value.to_string(),
                automatic.to_string(),
            ],
        )?;
        let result: BridgeResult = serde_json::from_str(&output)
            .map_err(|error| format!("El motor devolvió una respuesta inválida: {error}"))?;
        if result.ok {
            diagnostics::log(
                "info",
                "camera.controls",
                "value.applied",
                "Control de cámara aplicado.",
                json!({ "kind": kind, "property": property, "value": value, "automatic": automatic, "cameraId": "[redacted]" }),
            );
            Ok(())
        } else {
            Err(result
                .error
                .unwrap_or_else(|| "El controlador rechazó el cambio.".to_string()))
        }
    })
    .await
}

#[tauri::command]
async fn open_driver_property_page(app: tauri::AppHandle, camera_id: String) -> Result<(), String> {
    run_blocking(move || {
        let preview_active = app
            .state::<PreviewState>()
            .session
            .lock()
            .map_err(|_| "No se pudo comprobar la vista previa.".to_string())?
            .active;
        let virtual_output_active = app
            .state::<VirtualOutputState>()
            .session
            .lock()
            .map_err(|_| "No se pudo comprobar la salida virtual.".to_string())?
            .active;
        if preview_active || virtual_output_active {
            return Err("Detén la captura antes de abrir el panel del fabricante.".to_string());
        }
        let backend_id = resolve_backend_camera_id(&app, &camera_id)?;
        let _lease = acquire_camera_lease(&app, &camera_id, LeasePurpose::WriteControl)?;
        bridge(&app, &["property-page".into(), backend_id])?;
        diagnostics::log(
            "info",
            "camera.controls",
            "property_page.closed",
            "Panel original del fabricante cerrado.",
            json!({ "cameraId": "[redacted]" }),
        );
        Ok(())
    })
    .await
}

#[tauri::command]
async fn start_preview(
    app: tauri::AppHandle,
    camera_id: String,
    requested_format: Option<VideoFormat>,
) -> Result<PreviewStartResult, String> {
    run_blocking(move || {
        let state = app.state::<PreviewState>();
        let mut session = state
            .session
            .lock()
            .map_err(|_| "No se pudo iniciar la vista previa.".to_string())?;
        stop_native_preview(&app, &mut session);

        let camera = get_camera_list(&app)?
            .into_iter()
            .find(|item| item.id == camera_id)
            .ok_or_else(|| "La cámara seleccionada ya no está disponible.".to_string())?;
        let lease = acquire_camera_lease(&app, &camera.id, LeasePurpose::Preview)?;
        let formats = native_host::enumerate_formats(&app, &camera.id)?;
        let format = match requested_format.as_ref() {
            Some(requested) => requested_capture_format(&formats, requested)?,
            None => preferred_preview_format(&formats).ok_or_else(|| {
                "La cámara no expone un modo compatible con la vista previa.".to_string()
            })?,
        };
        let (preview_width, preview_height) = preview_output_dimensions(&format);
        let cache_dir = app
            .path()
            .app_cache_dir()
            .map_err(|error| format!("No se pudo preparar la vista previa: {error}"))?
            .join("preview");
        fs::create_dir_all(&cache_dir)
            .map_err(|error| format!("No se pudo preparar la carpeta de vista previa: {error}"))?;
        let frame_path = cache_dir.join("native-preview-v1.bin");
        remove_if_present(Some(&frame_path));
        let started = Instant::now();
        let filter_graph = app
            .state::<ProcessingState>()
            .graph
            .lock()
            .map_err(|_| "Los filtros quedaron en un estado inválido.".to_string())?
            .clone();
        let lut_assets = app
            .state::<ProcessingState>()
            .lut_assets
            .lock()
            .map_err(|_| "Los recursos LUT quedaron en un estado inválido.".to_string())?
            .clone();
        let plugins = app
            .state::<ProcessingState>()
            .plugins
            .lock()
            .map_err(|_| "El catálogo de plugins quedó en un estado inválido.".to_string())?
            .clone();
        if let Err(error) = native_host::open_capture(
            &app,
            native_host::NativeCaptureOptions {
                camera_id: camera.id.clone(),
                format: format.clone(),
                output_width: preview_width,
                output_height: preview_height,
                output_pixel_format: PixelFormat::Bgra,
                scaling: ScalingMode::FastBilinear,
                frame_path: frame_path.to_string_lossy().into_owned(),
                filter_graph,
                lut_assets,
                plugins,
            },
        ) {
            remove_if_present(Some(&frame_path));
            return Err(error);
        }
        let frame_reader = match FrameReader::open(&frame_path) {
            Ok(reader) => reader,
            Err(error) => {
                let _ = native_host::close_capture(&app);
                remove_if_present(Some(&frame_path));
                return Err(format!(
                    "No se pudo abrir el intercambio de cuadros: {error}"
                ));
            }
        };
        let initial_frame = match frame_reader.read_latest() {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                let _ = native_host::close_capture(&app);
                remove_if_present(Some(&frame_path));
                return Err("El host nativo no publicó un primer cuadro válido.".into());
            }
            Err(error) => {
                let _ = native_host::close_capture(&app);
                remove_if_present(Some(&frame_path));
                return Err(format!("No se pudo validar el primer cuadro: {error}"));
            }
        };
        diagnostics::log(
            "info",
            "media_capture.preview",
            "session.started",
            "Vista previa nativa iniciada.",
            json!({
                "cameraName": camera.name,
                "width": format.width,
                "height": format.height,
                "fpsNumerator": format.fps_numerator,
                "fpsDenominator": format.fps_denominator,
                "pixelFormat": format.pixel_format,
                "previewWidth": preview_width,
                "previewHeight": preview_height,
                "firstFrameMillis": started.elapsed().as_millis(),
            }),
        );
        session.active = true;
        session.frame_reader = Some(frame_reader);
        session.frame_path = Some(frame_path);
        session.last_sequence = initial_frame.metadata.sequence.saturating_sub(2);
        session.last_frame_at = Some(Instant::now());
        session.lease = Some(lease);
        Ok(PreviewStartResult {
            format,
            preview_width,
            preview_height,
        })
    })
    .await
}

#[tauri::command]
async fn get_preview_frame(app: tauri::AppHandle) -> Result<Response, String> {
    let bytes = run_blocking(move || {
        let state = app.state::<PreviewState>();
        let mut session = state
            .session
            .lock()
            .map_err(|_| "No se pudo leer la vista previa.".to_string())?;
        if !session.active {
            return Ok(Vec::new());
        }
        let snapshot = session
            .frame_reader
            .as_ref()
            .ok_or_else(|| "La vista previa no tiene un lector de cuadros.".to_string())?
            .read_latest_after(session.last_sequence)
            .map_err(|error| format!("No se pudo leer el cuadro nativo: {error}"))?;
        let Some(snapshot) = snapshot else {
            if session
                .last_frame_at
                .is_some_and(|instant| instant.elapsed() >= Duration::from_secs(3))
            {
                stop_native_preview(&app, &mut session);
                return Err("La cámara dejó de producir cuadros durante 3 segundos.".into());
            }
            return Ok(Vec::new());
        };
        session.last_sequence = snapshot.metadata.sequence;
        session.last_frame_at = Some(Instant::now());
        encode_preview_jpeg(snapshot)
    })
    .await?;
    Ok(Response::new(bytes))
}

#[tauri::command]
async fn stop_preview(app: tauri::AppHandle) -> Result<(), String> {
    run_blocking(move || stop_preview_locked(&app)).await
}

fn processing_is_active(app: &tauri::AppHandle) -> Result<bool, String> {
    let preview = app
        .state::<PreviewState>()
        .session
        .lock()
        .map_err(|_| "No se pudo comprobar la vista previa.".to_string())?
        .active;
    let output = app
        .state::<VirtualOutputState>()
        .session
        .lock()
        .map_err(|_| "No se pudo comprobar la salida virtual.".to_string())?
        .active;
    Ok(preview || output)
}

#[tauri::command]
async fn get_filter_graph(app: tauri::AppHandle) -> Result<FilterGraph, String> {
    run_blocking(move || {
        app.state::<ProcessingState>()
            .graph
            .lock()
            .map(|graph| graph.clone())
            .map_err(|_| "El grafo de filtros quedó en un estado inválido.".to_string())
    })
    .await
}

#[tauri::command]
async fn set_filter_graph(app: tauri::AppHandle, graph: FilterGraph) -> Result<(), String> {
    run_blocking(move || {
        let state = app.state::<ProcessingState>();
        let assets = state
            .lut_assets
            .lock()
            .map_err(|_| "Los recursos LUT quedaron en un estado inválido.".to_string())?
            .clone();
        let plugins = state
            .plugins
            .lock()
            .map_err(|_| "El catálogo de plugins quedó en un estado inválido.".to_string())?
            .clone();
        camera_processing::validate_filter_graph_config(&graph, &assets, &plugins)?;
        if processing_is_active(&app)? {
            native_host::set_filter_graph(&app, graph.clone())?;
        }
        *state
            .graph
            .lock()
            .map_err(|_| "El grafo de filtros quedó en un estado inválido.".to_string())? = graph;
        Ok(())
    })
    .await
}

#[tauri::command]
async fn set_filter_lut_asset(
    app: tauri::AppHandle,
    asset_id: String,
    cube: Option<String>,
) -> Result<(), String> {
    run_blocking(move || {
        const MAX_LUT_BYTES: usize = 8 * 1024 * 1024;
        camera_processing::validate_lut_asset_id(&asset_id)?;
        if cube
            .as_ref()
            .is_some_and(|value| value.len() > MAX_LUT_BYTES)
        {
            return Err("La LUT supera el límite de 8 MiB.".to_string());
        }
        let state = app.state::<ProcessingState>();
        let graph = state
            .graph
            .lock()
            .map_err(|_| "El grafo de filtros quedó en un estado inválido.".to_string())?
            .clone();
        let plugins = state
            .plugins
            .lock()
            .map_err(|_| "El catálogo de plugins quedó en un estado inválido.".to_string())?
            .clone();
        let mut assets = state
            .lut_assets
            .lock()
            .map_err(|_| "Los recursos LUT quedaron en un estado inválido.".to_string())?;
        let mut candidate = assets.clone();
        match cube.clone() {
            Some(value) => {
                candidate.insert(asset_id.clone(), value);
            }
            None => {
                candidate.remove(&asset_id);
            }
        }
        camera_processing::ProcessingPipeline::new(graph, candidate.clone(), plugins)?;
        if processing_is_active(&app)? {
            native_host::set_lut_asset(&app, asset_id.clone(), cube)?;
        }
        let directory = filter_lut_directory(&app)?;
        fs::create_dir_all(&directory)
            .map_err(|error| format!("No se pudo preparar la biblioteca LUT: {error}"))?;
        let path = directory.join(format!("{}.cube", asset_id));
        if let Some(value) = candidate.get(&asset_id) {
            fs::write(&path, value)
                .map_err(|error| format!("No se pudo guardar la LUT: {error}"))?;
        } else if path.exists() {
            fs::remove_file(&path)
                .map_err(|error| format!("No se pudo eliminar la LUT: {error}"))?;
        }
        *assets = candidate;
        Ok(())
    })
    .await
}

fn filter_plugin_directory(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("filter-plugins"))
        .map_err(|error| format!("No se pudo localizar la carpeta de plugins: {error}"))
}

fn filter_lut_directory(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("filter-assets").join("luts"))
        .map_err(|error| format!("No se pudo localizar la biblioteca LUT: {error}"))
}

fn load_persisted_lut_assets(app: &tauri::AppHandle) -> Result<(), String> {
    const MAX_LUT_BYTES: u64 = 8 * 1024 * 1024;
    const MAX_ASSETS: usize = 64;
    let directory = filter_lut_directory(app)?;
    fs::create_dir_all(&directory)
        .map_err(|error| format!("No se pudo preparar la biblioteca LUT: {error}"))?;
    let mut assets = BTreeMap::new();
    let mut entries = fs::read_dir(&directory)
        .map_err(|error| format!("No se pudo leer la biblioteca LUT: {error}"))?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("cube"))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries.into_iter().take(MAX_ASSETS) {
        let path = entry.path();
        let Some(id) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let metadata = entry.metadata().map_err(|error| error.to_string())?;
        if metadata.len() > MAX_LUT_BYTES {
            diagnostics::log(
                "warn",
                "processing.lut",
                "asset.skipped",
                "Se omitió una LUT persistida que supera 8 MiB.",
                json!({ "file": entry.file_name().to_string_lossy() }),
            );
            continue;
        }
        let cube = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        if camera_processing::CubeLut::parse(&cube).is_ok() {
            assets.insert(id.to_string(), cube);
        }
    }
    *app.state::<ProcessingState>()
        .lut_assets
        .lock()
        .map_err(|_| "Los recursos LUT quedaron en un estado inválido.".to_string())? = assets;
    Ok(())
}

#[tauri::command]
async fn list_filter_plugins(app: tauri::AppHandle) -> Result<FilterPluginCatalog, String> {
    run_blocking(move || {
        const MAX_PLUGIN_BYTES: u64 = 256 * 1024;
        const MAX_PLUGIN_FILES: usize = 64;
        let directory = filter_plugin_directory(&app)?;
        fs::create_dir_all(&directory)
            .map_err(|error| format!("No se pudo crear la carpeta de plugins: {error}"))?;
        if processing_is_active(&app)? {
            let plugins = app
                .state::<ProcessingState>()
                .plugins
                .lock()
                .map_err(|_| "El catálogo de plugins quedó en un estado inválido.".to_string())?
                .clone();
            return Ok(FilterPluginCatalog {
                directory: directory.to_string_lossy().into_owned(),
                plugins,
                warnings: Vec::new(),
            });
        }
        let mut plugins = Vec::new();
        let mut warnings = Vec::new();
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| format!("No se pudo leer la carpeta de plugins: {error}"))?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("json")
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries.into_iter().take(MAX_PLUGIN_FILES) {
            let path = entry.path();
            let result = (|| -> Result<FilterPluginManifest, String> {
                let metadata = entry.metadata().map_err(|error| error.to_string())?;
                if metadata.len() > MAX_PLUGIN_BYTES {
                    return Err("el manifiesto supera 256 KiB".into());
                }
                let bytes = fs::read(&path).map_err(|error| error.to_string())?;
                let manifest: FilterPluginManifest =
                    serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
                camera_processing::validate_plugin_manifest(&manifest)?;
                Ok(manifest)
            })();
            match result {
                Ok(manifest) => {
                    if plugins
                        .iter()
                        .any(|item: &FilterPluginManifest| item.id == manifest.id)
                    {
                        warnings.push(format!(
                            "{}: id de plugin duplicado '{}'",
                            path.display(),
                            manifest.id
                        ));
                    } else {
                        plugins.push(manifest);
                    }
                }
                Err(error) => warnings.push(format!("{}: {error}", path.display())),
            }
        }
        let state = app.state::<ProcessingState>();
        let current_graph = state
            .graph
            .lock()
            .map_err(|_| "El grafo de filtros quedó en un estado inválido.".to_string())?
            .clone();
        let assets = state
            .lut_assets
            .lock()
            .map_err(|_| "Los recursos LUT quedaron en un estado inválido.".to_string())?
            .clone();
        camera_processing::validate_filter_graph_config(&current_graph, &assets, &plugins)?;
        *state
            .plugins
            .lock()
            .map_err(|_| "El catálogo de plugins quedó en un estado inválido.".to_string())? =
            plugins.clone();
        Ok(FilterPluginCatalog {
            directory: directory.to_string_lossy().into_owned(),
            plugins,
            warnings,
        })
    })
    .await
}

#[tauri::command]
async fn install_filter_plugin(
    app: tauri::AppHandle,
    file_name: String,
    manifest_json: String,
) -> Result<FilterPluginManifest, String> {
    run_blocking(move || {
        const MAX_PLUGIN_BYTES: usize = 256 * 1024;
        if processing_is_active(&app)? {
            return Err(
                "Detén la vista previa o la salida virtual antes de instalar un plugin."
                    .to_string(),
            );
        }
        if manifest_json.len() > MAX_PLUGIN_BYTES {
            return Err("El manifiesto del plugin supera 256 KiB.".to_string());
        }
        let manifest: FilterPluginManifest = serde_json::from_str(&manifest_json)
            .map_err(|error| format!("El archivo no es un manifiesto JSON válido: {error}"))?;
        camera_processing::validate_plugin_manifest(&manifest)
            .map_err(|error| format!("El manifiesto no es válido: {error}"))?;
        let directory = filter_plugin_directory(&app)?;
        fs::create_dir_all(&directory)
            .map_err(|error| format!("No se pudo preparar la biblioteca de plugins: {error}"))?;
        let path = directory.join(format!("{}.json", manifest.id));
        let normalized = serde_json::to_string_pretty(&manifest)
            .map_err(|error| format!("No se pudo normalizar el manifiesto: {error}"))?;
        fs::write(&path, normalized)
            .map_err(|error| format!("No se pudo guardar el plugin: {error}"))?;
        diagnostics::log(
            "info",
            "processing.plugin",
            "plugin.installed",
            "Plugin de filtros instalado.",
            json!({ "id": manifest.id, "name": manifest.name, "sourceFile": file_name }),
        );
        Ok(manifest)
    })
    .await
}

#[tauri::command]
async fn get_virtual_camera_status(app: tauri::AppHandle) -> Result<VirtualCameraStatus, String> {
    run_blocking(move || {
        let supported = cfg!(target_os = "windows")
            && std::env::var("OS").is_ok()
            && binary_path(&app, VIRTUAL_CAMERA_CONTROL_BINARY).is_ok()
            && binary_path(&app, MEDIA_SOURCE_BINARY).is_ok();
        let component_status = if supported {
            virtual_camera_control(&app, "status", None).unwrap_or_else(|_| "unavailable".into())
        } else {
            "unsupported".into()
        };
        let installed = component_status.eq_ignore_ascii_case("installed");
        let state = app.state::<VirtualOutputState>();
        let mut session = state
            .session
            .lock()
            .map_err(|_| "El estado de la salida virtual no está disponible.".to_string())?;
        let frame_inactive = session.active
            && session
                .frame_path
                .as_deref()
                .is_none_or(|path| !camera_frame::has_active_frame(path).unwrap_or(false));
        if frame_inactive {
            stop_native_virtual_output(&app, &mut session);
        }
        Ok(VirtualCameraStatus {
            supported,
            installed,
            running: session.active,
            name: VIRTUAL_CAMERA_NAME,
            width: session.width,
            height: session.height,
            detail: match component_status.as_str() {
                "unsupported" => Some(
                    "Requiere Windows 11 y el componente Media Foundation compilado."
                        .to_string(),
                ),
                "source-not-registered" => Some(
                    "Lista para instalar. Windows solicitará permiso de administrador una sola vez."
                        .to_string(),
                ),
                "source-needs-repair" => Some(
                    "La Media Source está registrada desde una ubicación no accesible para Windows Frame Server. La instalación la reparará con permisos de administrador."
                        .to_string(),
                ),
                "storage-not-ready" => Some(
                    "La instalación necesita reparar el almacenamiento compartido. Windows solicitará permiso."
                        .to_string(),
                ),
                "source-invalid" => Some(
                    "El componente COM de la cámara virtual no se puede activar. Vuelve a instalarlo para repararlo."
                        .to_string(),
                ),
                "unavailable" => Some(
                    "No se pudo consultar el componente de cámara virtual. Revisa los diagnósticos."
                        .to_string(),
                ),
                _ => None,
            },
        })
    })
    .await
}

#[tauri::command]
async fn install_virtual_camera(app: tauri::AppHandle) -> Result<(), String> {
    run_blocking(move || {
        let media_source = binary_path(&app, MEDIA_SOURCE_BINARY)?;
        virtual_camera_control(&app, "install", Some(&media_source)).map(|_| ())
    })
    .await
}

#[tauri::command]
async fn remove_virtual_camera(app: tauri::AppHandle) -> Result<(), String> {
    run_blocking(move || {
        let state = app.state::<VirtualOutputState>();
        let mut session = state
            .session
            .lock()
            .map_err(|_| "No se pudo detener la salida virtual.".to_string())?;
        stop_native_virtual_output(&app, &mut session);
        virtual_camera_control(&app, "remove", None).map(|_| ())
    })
    .await
}

#[tauri::command]
async fn start_virtual_output(
    app: tauri::AppHandle,
    options: VirtualOutputOptions,
) -> Result<(), String> {
    run_blocking(move || {
        validate_output_options(&options)?;
        if !virtual_camera_control(&app, "status", None)?.eq_ignore_ascii_case("installed") {
            return Err("Instala primero CameraTuner Virtual Camera.".to_string());
        }

        stop_preview_locked(&app)?;
        let camera = get_camera_list(&app)?
            .into_iter()
            .find(|item| item.id == options.camera_id)
            .ok_or_else(|| "La cámara seleccionada ya no está disponible.".to_string())?;
        let state = app.state::<VirtualOutputState>();
        let mut session = state
            .session
            .lock()
            .map_err(|_| "No se pudo iniciar la salida virtual.".to_string())?;
        stop_native_virtual_output(&app, &mut session);
        let lease = acquire_camera_lease(&app, &camera.id, LeasePurpose::VirtualOutput)?;
        let frame_path = frame_exchange_path();
        if let Some(parent) = frame_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!("No se pudo abrir el intercambio de cuadros en ProgramData: {error}")
            })?;
        }
        let formats = native_host::enumerate_formats(&app, &camera.id)?;
        let format = match options.input_format.as_ref() {
            Some(requested) => requested_capture_format(&formats, requested)?,
            None => preferred_output_input_format(&formats, options.width, options.height)
                .ok_or_else(|| {
                    "La cámara no expone un modo compatible con la salida virtual.".to_string()
                })?,
        };
        let scaling = if matches!(options.quality.as_str(), "none" | "fast") {
            ScalingMode::FastBilinear
        } else {
            ScalingMode::QualityLanczos3
        };
        let filter_graph = app
            .state::<ProcessingState>()
            .graph
            .lock()
            .map_err(|_| "Los filtros quedaron en un estado inválido.".to_string())?
            .clone();
        let lut_assets = app
            .state::<ProcessingState>()
            .lut_assets
            .lock()
            .map_err(|_| "Los recursos LUT quedaron en un estado inválido.".to_string())?
            .clone();
        let plugins = app
            .state::<ProcessingState>()
            .plugins
            .lock()
            .map_err(|_| "El catálogo de plugins quedó en un estado inválido.".to_string())?
            .clone();
        native_host::open_capture(
            &app,
            native_host::NativeCaptureOptions {
                camera_id: camera.id.clone(),
                format: format.clone(),
                output_width: options.width,
                output_height: options.height,
                output_pixel_format: PixelFormat::Nv12,
                scaling,
                frame_path: frame_path.to_string_lossy().into_owned(),
                filter_graph,
                lut_assets,
                plugins,
            },
        )?;
        match FrameReader::open(&frame_path).and_then(|reader| reader.read_latest()) {
            Ok(Some(_)) => {}
            Ok(None) => {
                let _ = native_host::close_capture(&app);
                return Err("El host nativo no publicó un cuadro virtual válido.".into());
            }
            Err(error) => {
                let _ = native_host::close_capture(&app);
                return Err(format!("No se pudo validar el cuadro virtual: {error}"));
            }
        }
        diagnostics::log(
            "info",
            "media_capture.virtual_output",
            "session.started",
            "Salida virtual nativa iniciada.",
            json!({
                "cameraName": camera.name,
                "inputWidth": format.width,
                "inputHeight": format.height,
                "inputFpsNumerator": format.fps_numerator,
                "inputFpsDenominator": format.fps_denominator,
                "inputPixelFormat": format.pixel_format,
                "width": options.width,
                "height": options.height,
                "quality": options.quality,
                "scaler": scaling,
                "fps": 30
            }),
        );
        session.active = true;
        session.frame_path = Some(frame_path);
        session.width = options.width;
        session.height = options.height;
        session.lease = Some(lease);
        Ok(())
    })
    .await
}

#[tauri::command]
async fn stop_virtual_output(app: tauri::AppHandle) -> Result<(), String> {
    run_blocking(move || {
        let state = app.state::<VirtualOutputState>();
        let mut session = state
            .session
            .lock()
            .map_err(|_| "No se pudo detener la salida virtual.".to_string())?;
        stop_native_virtual_output(&app, &mut session);
        Ok(())
    })
    .await
}

#[tauri::command]
async fn get_virtual_output_running(app: tauri::AppHandle) -> Result<bool, String> {
    run_blocking(move || {
        let state = app.state::<VirtualOutputState>();
        let mut session = state
            .session
            .lock()
            .map_err(|_| "No se pudo comprobar la salida virtual.".to_string())?;
        if !session.active {
            return Ok(false);
        }
        let frame_active = session
            .frame_path
            .as_deref()
            .is_some_and(|path| camera_frame::has_active_frame(path).unwrap_or(false));
        if !frame_active {
            diagnostics::log(
                "error",
                "media_capture.virtual_output",
                "session.became_inactive",
                "El productor de vídeo dejó de publicar fotogramas.",
                json!({}),
            );
            stop_native_virtual_output(&app, &mut session);
        }
        Ok(session.active)
    })
    .await
}

#[tauri::command]
fn write_frontend_log(entry: FrontendLogEntry) {
    diagnostics::log(
        &entry.level,
        "frontend",
        &entry.event,
        &entry.message,
        entry.context.unwrap_or(Value::Null),
    );
}

#[tauri::command]
fn get_diagnostics_info() -> Result<DiagnosticsInfo, String> {
    let directory = diagnostics::directory()
        .ok_or_else(|| "El sistema de diagnóstico todavía no está disponible.".to_string())?;
    Ok(DiagnosticsInfo {
        file: directory
            .join("webcam-control.jsonl")
            .to_string_lossy()
            .into_owned(),
        directory: directory.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
fn open_diagnostics_folder() -> Result<(), String> {
    let directory = diagnostics::directory()
        .ok_or_else(|| "El sistema de diagnóstico todavía no está disponible.".to_string())?;
    Command::new("explorer.exe")
        .arg(&directory)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| format!("No se pudo abrir la carpeta de diagnóstico: {error}"))?;
    diagnostics::log(
        "info",
        "diagnostics",
        "folder.opened",
        "El usuario abrió la carpeta de diagnóstico.",
        Value::Null,
    );
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(BridgeState::default())
        .manage(CoordinatorState::default())
        .manage(native_host::NativeHostState::default())
        .manage(PreviewState::default())
        .manage(ProcessingState::default())
        .manage(VirtualOutputState::default())
        .setup(|app| {
            let log_directory = app.path().app_log_dir()?;
            let log_file = diagnostics::init(log_directory).map_err(std::io::Error::other)?;
            diagnostics::install_panic_hook();
            load_persisted_lut_assets(app.handle()).map_err(std::io::Error::other)?;
            diagnostics::log(
                "info",
                "application",
                "session.started",
                "Aplicación iniciada.",
                json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "build": if cfg!(debug_assertions) { "debug" } else { "release" },
                    "os": env::consts::OS,
                    "arch": env::consts::ARCH,
                    "logFile": log_file.file_name().and_then(|value| value.to_str()),
                }),
            );
            let show = MenuItem::with_id(app, "show", "Mostrar CameraTuner", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Salir", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let icon = app
                .default_window_icon()
                .cloned()
                .ok_or("No se encontró el icono de la aplicación.")?;
            TrayIconBuilder::with_id("control-webcam-tray")
                .icon(icon)
                .tooltip("CameraTuner")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        diagnostics::log(
                            "info",
                            "tray",
                            "show",
                            "Ventana restaurada desde la bandeja.",
                            Value::Null,
                        );
                        show_main_window(app);
                    }
                    "quit" => {
                        diagnostics::log(
                            "info",
                            "application",
                            "exit.requested",
                            "Salida solicitada desde la bandeja.",
                            Value::Null,
                        );
                        let _ = stop_preview_locked(app);
                        if let Ok(mut session) = app.state::<VirtualOutputState>().session.lock() {
                            stop_native_virtual_output(app, &mut session);
                        }
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        diagnostics::log(
                            "debug",
                            "tray",
                            "clicked",
                            "Icono de bandeja pulsado.",
                            Value::Null,
                        );
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::Resized(_) if window.is_minimized().unwrap_or(false) => {
                diagnostics::log(
                    "debug",
                    "window",
                    "minimized",
                    "Ventana minimizada a la bandeja.",
                    Value::Null,
                );
                let _ = window.hide();
            }
            WindowEvent::CloseRequested { api, .. } => {
                diagnostics::log(
                    "debug",
                    "window",
                    "close_intercepted",
                    "La ventana se ocultó en la bandeja.",
                    Value::Null,
                );
                api.prevent_close();
                let _ = window.hide();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            list_cameras,
            get_controls,
            set_control,
            open_driver_property_page,
            start_preview,
            get_preview_frame,
            stop_preview,
            get_filter_graph,
            set_filter_graph,
            set_filter_lut_asset,
            list_filter_plugins,
            install_filter_plugin,
            get_virtual_camera_status,
            install_virtual_camera,
            remove_virtual_camera,
            start_virtual_output,
            stop_virtual_output,
            get_virtual_output_running,
            write_frontend_log,
            get_diagnostics_info,
            open_diagnostics_folder,
            native_host::list_native_cameras,
            native_host::list_native_formats,
            native_host::probe_source_reader,
            native_host::probe_media_frame_reader
        ])
        .run(tauri::generate_context!())
        .expect("No se pudo iniciar CameraTuner");
}

#[cfg(test)]
mod preview_tests {
    use super::*;
    use camera_frame::{FrameMetadata, PIXEL_FORMAT_BGRA};

    #[test]
    fn chooses_smallest_preferred_resolution_for_interactive_preview() {
        let formats = [
            format(1920, 1080, PixelFormat::Nv12),
            format(1280, 720, PixelFormat::Mjpeg),
            format(1280, 720, PixelFormat::Nv12),
            format(640, 360, PixelFormat::Nv12),
        ];
        assert_eq!(preferred_preview_format(&formats), Some(formats[3].clone()));
    }

    #[test]
    fn accepts_only_a_mode_currently_advertised_by_the_camera() {
        let formats = [
            format(1920, 1080, PixelFormat::Mjpeg),
            format(1280, 720, PixelFormat::Nv12),
        ];
        assert_eq!(
            requested_capture_format(&formats, &formats[0]).unwrap(),
            formats[0]
        );
        assert!(requested_capture_format(&formats, &format(640, 360, PixelFormat::Nv12)).is_err());
    }

    #[test]
    fn preview_transport_is_capped_without_changing_aspect_ratio() {
        assert_eq!(
            preview_output_dimensions(&format(1920, 1080, PixelFormat::Nv12)),
            (960, 540)
        );
        assert_eq!(
            preview_output_dimensions(&format(640, 480, PixelFormat::Nv12)),
            (640, 480)
        );
    }

    #[test]
    fn encodes_bgra_snapshot_as_jpeg() {
        let jpeg = encode_preview_jpeg(FrameSnapshot {
            metadata: FrameMetadata {
                width: 2,
                height: 1,
                stride: 8,
                pixel_format: PIXEL_FORMAT_BGRA,
                frame_size: 8,
                sequence: 2,
                timestamp_micros: 1,
                active: true,
            },
            bytes: vec![0, 0, 255, 255, 0, 255, 0, 255],
        })
        .unwrap();
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xff, 0xd9]);
    }

    fn format(width: u32, height: u32, pixel_format: PixelFormat) -> VideoFormat {
        VideoFormat {
            width,
            height,
            fps_numerator: 30,
            fps_denominator: 1,
            pixel_format,
            subtype_guid: None,
        }
    }
}
