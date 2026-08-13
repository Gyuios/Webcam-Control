use camera_frame::{FrameWriter, PIXEL_FORMAT_BGRA, PIXEL_FORMAT_NV12};
use camera_processing::{resize_bgra, ProcessingPipeline};
use camera_protocol::{
    BackendKind, ErrorCode, FilterGraph, FilterPluginManifest, HostCommand, HostError,
    HostResponse, PixelFormat, RequestEnvelope, ResponseEnvelope, ScalingMode, VideoFormat,
    PROTOCOL_VERSION,
};
use shiguredo_libyuv::{
    argb_scale, argb_to_nv12, ArgbImage, ArgbImageMut, FilterMode, ImageSize, Nv12ImageMut,
};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{sync_channel, RecvTimeoutError},
        Arc, RwLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows_camera::{
    enumerate_devices, enumerate_media_frame_formats, probe_media_frame_reader,
    probe_source_reader, stream_media_frame_reader, CaptureStreamSummary, MediaFoundationRuntime,
};

const VIRTUAL_NV12_FRAME_CAPACITY: usize = 3840 * 2160 * 3 / 2;

struct CameraHost {
    runtime: MediaFoundationRuntime,
    session: Option<CaptureSession>,
}

struct CaptureSession {
    stop: Arc<AtomicBool>,
    filter_graph: Arc<RwLock<FilterGraphState>>,
    lut_assets: Arc<RwLock<LutAssetState>>,
    plugins: Vec<FilterPluginManifest>,
    thread: Option<JoinHandle<Result<CaptureStreamSummary, String>>>,
}

#[derive(Clone, Default)]
struct FilterGraphState {
    revision: u64,
    graph: FilterGraph,
}

impl FilterGraphState {
    fn replace(&mut self, graph: FilterGraph) {
        if self.graph != graph {
            self.graph = graph;
            self.revision = self.revision.wrapping_add(1);
        }
    }
}

#[derive(Clone, Default)]
struct LutAssetState {
    revision: u64,
    assets: BTreeMap<String, String>,
}

struct OpenSessionRequest {
    device_id: String,
    backend: BackendKind,
    format: VideoFormat,
    output_width: u32,
    output_height: u32,
    output_pixel_format: PixelFormat,
    scaling: ScalingMode,
    frame_path: PathBuf,
    filter_graph: FilterGraph,
    lut_assets: BTreeMap<String, String>,
    plugins: Vec<FilterPluginManifest>,
}

struct PipelineMetrics {
    interval_started: Instant,
    frames: u64,
    capture_copy_micros: u128,
    filter_micros: u128,
    resize_micros: u128,
    conversion_micros: u128,
    publish_micros: u128,
    total_micros: u128,
    max_total_micros: u64,
    output_pixel_format: PixelFormat,
}

impl PipelineMetrics {
    fn new(output_pixel_format: PixelFormat) -> Self {
        Self {
            interval_started: Instant::now(),
            frames: 0,
            capture_copy_micros: 0,
            filter_micros: 0,
            resize_micros: 0,
            conversion_micros: 0,
            publish_micros: 0,
            total_micros: 0,
            max_total_micros: 0,
            output_pixel_format,
        }
    }

    fn observe(
        &mut self,
        capture_copy_micros: u64,
        filter_micros: u64,
        resize_micros: u64,
        conversion_micros: u64,
        publish_micros: u64,
        total_micros: u64,
    ) {
        self.frames = self.frames.saturating_add(1);
        self.capture_copy_micros += u128::from(capture_copy_micros);
        self.filter_micros += u128::from(filter_micros);
        self.resize_micros += u128::from(resize_micros);
        self.conversion_micros += u128::from(conversion_micros);
        self.publish_micros += u128::from(publish_micros);
        self.total_micros += u128::from(total_micros);
        self.max_total_micros = self.max_total_micros.max(total_micros);
        if self.interval_started.elapsed() >= Duration::from_secs(10) {
            self.emit();
            *self = Self::new(self.output_pixel_format);
        }
    }

    fn emit(&self) {
        if self.frames == 0 {
            return;
        }
        let divisor = u128::from(self.frames);
        eprintln!(
            "{}",
            serde_json::json!({
                "level": "info",
                "event": "pipeline.metrics",
                "message": "Métricas agregadas del pipeline de vídeo.",
                "intervalMillis": self.interval_started.elapsed().as_millis(),
                "frames": self.frames,
                "captureCopyAvgMicros": self.capture_copy_micros / divisor,
                "filterAvgMicros": self.filter_micros / divisor,
                "resizeAvgMicros": self.resize_micros / divisor,
                "conversionAvgMicros": self.conversion_micros / divisor,
                "publishAvgMicros": self.publish_micros / divisor,
                "totalAvgMicros": self.total_micros / divisor,
                "totalMaxMicros": self.max_total_micros,
                "transport": "CTFRAME2",
                "slots": camera_frame::SLOT_COUNT,
                "pixelFormat": match self.output_pixel_format {
                    PixelFormat::Bgra => "BGRA",
                    PixelFormat::Nv12 => "NV12",
                    _ => "UNSUPPORTED",
                }
            })
        );
    }
}

fn convert_bgra_to_nv12(
    source: &[u8],
    width: u32,
    height: u32,
    destination: &mut Vec<u8>,
) -> Result<(), String> {
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err("NV12 output dimensions must be even".into());
    }
    let width = usize::try_from(width).map_err(|_| "NV12 width is too large")?;
    let height = usize::try_from(height).map_err(|_| "NV12 height is too large")?;
    let luma_len = width
        .checked_mul(height)
        .ok_or_else(|| "NV12 luma size overflowed".to_string())?;
    let frame_len = luma_len
        .checked_add(luma_len / 2)
        .ok_or_else(|| "NV12 frame size overflowed".to_string())?;
    destination.resize(frame_len, 0);
    let (y, uv) = destination.split_at_mut(luma_len);
    let source = ArgbImage {
        data: source,
        stride: width * 4,
    };
    let mut destination = Nv12ImageMut {
        y,
        y_stride: width,
        uv,
        uv_stride: width,
    };
    argb_to_nv12(&source, &mut destination, ImageSize::new(width, height))
        .map_err(|error| format!("libyuv BGRA-to-NV12 conversion failed: {error}"))
}

fn resize_bgra_fast(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    destination_width: u32,
    destination_height: u32,
    destination: &mut Vec<u8>,
) -> Result<(), String> {
    let source_width = usize::try_from(source_width).map_err(|_| "source width is too large")?;
    let source_height = usize::try_from(source_height).map_err(|_| "source height is too large")?;
    let destination_width =
        usize::try_from(destination_width).map_err(|_| "destination width is too large")?;
    let destination_height =
        usize::try_from(destination_height).map_err(|_| "destination height is too large")?;
    let destination_len = destination_width
        .checked_mul(destination_height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "destination BGRA size overflowed".to_string())?;
    destination.resize(destination_len, 0);
    let source = ArgbImage {
        data: source,
        stride: source_width * 4,
    };
    let mut destination = ArgbImageMut {
        data: destination,
        stride: destination_width * 4,
    };
    argb_scale(
        &source,
        ImageSize::new(source_width, source_height),
        &mut destination,
        ImageSize::new(destination_width, destination_height),
        FilterMode::Bilinear,
    )
    .map_err(|error| format!("libyuv BGRA scaling failed: {error}"))
}

fn elapsed_micros(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

impl CaptureSession {
    fn stop(mut self) -> Result<CaptureStreamSummary, String> {
        self.stop.store(true, Ordering::Release);
        self.thread
            .take()
            .ok_or_else(|| "capture session has no worker thread".to_string())?
            .join()
            .map_err(|_| "capture worker panicked".to_string())?
    }

    fn update_filter_graph(&self, filter_graph: FilterGraph) -> Result<(), HostError> {
        let assets = self
            .lut_assets
            .read()
            .map_err(|_| internal_error("capture LUT asset state is poisoned"))?
            .assets
            .clone();
        camera_processing::validate_filter_graph_config(&filter_graph, &assets, &self.plugins)
            .map_err(invalid_request)?;
        self.filter_graph
            .write()
            .map_err(|_| internal_error("capture filter graph is poisoned"))?
            .replace(filter_graph);
        Ok(())
    }

    fn update_lut_asset(&self, asset_id: String, cube: Option<String>) -> Result<(), HostError> {
        let mut state = self
            .lut_assets
            .write()
            .map_err(|_| internal_error("capture LUT asset state is poisoned"))?;
        let mut next = state.assets.clone();
        match cube {
            Some(cube) => {
                next.insert(asset_id, cube);
            }
            None => {
                next.remove(&asset_id);
            }
        }
        let graph = self
            .filter_graph
            .read()
            .map_err(|_| internal_error("capture filter graph is poisoned"))?
            .graph
            .clone();
        ProcessingPipeline::new(graph, next.clone(), self.plugins.clone())
            .map_err(invalid_request)?;
        state.revision = state.revision.wrapping_add(1);
        state.assets = next;
        Ok(())
    }
}

impl Drop for CameraHost {
    fn drop(&mut self) {
        self.close_session();
    }
}

impl CameraHost {
    fn new() -> Result<Self, String> {
        Ok(Self {
            runtime: MediaFoundationRuntime::start()?,
            session: None,
        })
    }

    fn execute(&mut self, command: HostCommand) -> Result<HostResponse, HostError> {
        self.reap_finished_session();
        match command {
            HostCommand::Ping => Ok(HostResponse::Pong),
            HostCommand::EnumerateDevices => enumerate_devices(&self.runtime)
                .map(HostResponse::Devices)
                .map_err(backend_error),
            HostCommand::EnumerateFormats { device_id } => {
                enumerate_media_frame_formats(&self.runtime, &device_id)
                    .map(HostResponse::Formats)
                    .map_err(device_error)
            }
            HostCommand::ProbeSourceReader {
                device_id,
                format,
                frames,
            } => probe_source_reader(&self.runtime, &device_id, &format, frames)
                .map(HostResponse::CaptureProbe)
                .map_err(backend_error),
            HostCommand::ProbeMediaFrameReader {
                device_id,
                format,
                frames,
            } => probe_media_frame_reader(&self.runtime, &device_id, &format, frames)
                .map(HostResponse::CaptureProbe)
                .map_err(backend_error),
            HostCommand::Open {
                device_id,
                backend,
                format,
                output_width,
                output_height,
                output_pixel_format,
                scaling,
                frame_path,
                filter_graph,
                lut_assets,
                plugins,
            } => {
                self.open_session(OpenSessionRequest {
                    device_id,
                    backend,
                    format,
                    output_width,
                    output_height,
                    output_pixel_format,
                    scaling,
                    frame_path: PathBuf::from(frame_path),
                    filter_graph,
                    lut_assets,
                    plugins,
                })?;
                Ok(HostResponse::Acknowledged)
            }
            HostCommand::SetFilterGraph { filter_graph } => {
                self.session
                    .as_ref()
                    .ok_or_else(|| invalid_request("no capture session is open"))?
                    .update_filter_graph(filter_graph)?;
                Ok(HostResponse::Acknowledged)
            }
            HostCommand::SetLutAsset { asset_id, cube } => {
                self.session
                    .as_ref()
                    .ok_or_else(|| invalid_request("no capture session is open"))?
                    .update_lut_asset(asset_id, cube)?;
                Ok(HostResponse::Acknowledged)
            }
            HostCommand::Close => {
                self.close_session();
                Ok(HostResponse::Acknowledged)
            }
            HostCommand::Shutdown => {
                self.close_session();
                Ok(HostResponse::Acknowledged)
            }
            _ => Err(invalid_request(
                "command is not implemented by the native camera host",
            )),
        }
    }

    fn open_session(&mut self, request: OpenSessionRequest) -> Result<(), HostError> {
        let OpenSessionRequest {
            device_id,
            backend,
            format,
            output_width,
            output_height,
            output_pixel_format,
            scaling,
            frame_path,
            filter_graph,
            lut_assets,
            plugins,
        } = request;
        if self.session.is_some() {
            return Err(HostError {
                code: ErrorCode::DeviceBusy,
                message: "a capture session is already open in this host".into(),
                native_code: None,
                retryable: Some(false),
            });
        }
        if backend != BackendKind::MediaCapture {
            return Err(invalid_request(
                "persistent capture currently requires the media-capture backend",
            ));
        }
        if !frame_path.is_absolute()
            || frame_path.extension().and_then(|value| value.to_str()) != Some("bin")
        {
            return Err(invalid_request(
                "frame exchange path must be an absolute .bin file",
            ));
        }
        ProcessingPipeline::new(filter_graph.clone(), lut_assets.clone(), plugins.clone())
            .map_err(invalid_request)?;
        if output_width == 0 || output_height == 0 {
            return Err(invalid_request(
                "output dimensions must be greater than zero",
            ));
        }
        let frame_pixel_format = match output_pixel_format {
            PixelFormat::Bgra => PIXEL_FORMAT_BGRA,
            PixelFormat::Nv12
                if output_width.is_multiple_of(2) && output_height.is_multiple_of(2) =>
            {
                PIXEL_FORMAT_NV12
            }
            PixelFormat::Nv12 => {
                return Err(invalid_request("NV12 output dimensions must be even"));
            }
            _ => {
                return Err(invalid_request(
                    "persistent capture output must be BGRA or NV12",
                ));
            }
        };
        if scaling == ScalingMode::Ai {
            return Err(invalid_request("AI scaling requires a loaded ONNX backend"));
        }

        let stop = Arc::new(AtomicBool::new(false));
        let shared_filter_graph = Arc::new(RwLock::new(FilterGraphState {
            revision: 1,
            graph: filter_graph,
        }));
        let shared_lut_assets = Arc::new(RwLock::new(LutAssetState {
            revision: 1,
            assets: lut_assets,
        }));
        let worker_stop = Arc::clone(&stop);
        let worker_filter_graph = Arc::clone(&shared_filter_graph);
        let worker_lut_assets = Arc::clone(&shared_lut_assets);
        let worker_plugins = plugins.clone();
        let (ready_sender, ready_receiver) = sync_channel::<Result<(), String>>(1);
        let worker = thread::Builder::new()
            .name("media-frame-reader-capture".into())
            .spawn(move || {
                let runtime = match MediaFoundationRuntime::start() {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_sender.send(Err(error.clone()));
                        return Err(error);
                    }
                };
                let writer_result = if frame_pixel_format == PIXEL_FORMAT_NV12 {
                    FrameWriter::create_with_capacity(
                        &frame_path,
                        output_width,
                        output_height,
                        frame_pixel_format,
                        VIRTUAL_NV12_FRAME_CAPACITY,
                    )
                } else {
                    FrameWriter::create_with_format(
                        &frame_path,
                        output_width,
                        output_height,
                        frame_pixel_format,
                    )
                };
                let mut writer = match writer_result {
                    Ok(writer) => writer,
                    Err(error) => {
                        let message = format!("creating frame exchange failed: {error}");
                        let _ = ready_sender.send(Err(message.clone()));
                        return Err(message);
                    }
                };
                let (mut graph_revision, initial_graph) = {
                    let state = worker_filter_graph
                        .read()
                        .map_err(|_| "capture filter graph is poisoned".to_string())?;
                    (state.revision, state.graph.clone())
                };
                let initial_assets = worker_lut_assets
                    .read()
                    .map_err(|_| "capture LUT asset state is poisoned".to_string())?
                    .assets
                    .clone();
                let mut pipeline =
                    ProcessingPipeline::new(initial_graph, initial_assets, worker_plugins.clone())?;
                let mut lut_revision = 1;
                let mut metrics = PipelineMetrics::new(output_pixel_format);
                let mut converted_frame = Vec::new();
                let mut resized_frame = Vec::new();
                let mut ready_sender = Some(ready_sender);
                let mut restart_delay = Duration::from_millis(100);
                let result = loop {
                    let mut wrote_frame_this_run = false;
                    let attempt = stream_media_frame_reader(
                        &runtime,
                        &device_id,
                        &format,
                        &worker_stop,
                        |frame| {
                            let frame_started = Instant::now();
                            let capture_copy_micros = frame.copy_micros;
                            let next_graph = {
                                let state = worker_filter_graph
                                    .read()
                                    .map_err(|_| "capture filter graph is poisoned".to_string())?;
                                (state.revision != graph_revision)
                                    .then(|| (state.revision, state.graph.clone()))
                            };
                            if let Some((revision, graph)) = next_graph {
                                pipeline.set_graph(graph)?;
                                graph_revision = revision;
                            }
                            let next_assets = {
                                let state = worker_lut_assets.read().map_err(|_| {
                                    "capture LUT asset state is poisoned".to_string()
                                })?;
                                (state.revision != lut_revision)
                                    .then(|| (state.revision, state.assets.clone()))
                            };
                            if let Some((revision, assets)) = next_assets {
                                let current_graph = worker_filter_graph
                                    .read()
                                    .map_err(|_| "capture filter graph is poisoned".to_string())?
                                    .graph
                                    .clone();
                                pipeline = ProcessingPipeline::new(
                                    current_graph,
                                    assets,
                                    worker_plugins.clone(),
                                )?;
                                lut_revision = revision;
                            }
                            let filter_started = Instant::now();
                            pipeline.process_bgra(frame.pixels, frame.width, frame.height)?;
                            let filter_micros = elapsed_micros(filter_started);
                            let resize_started = Instant::now();
                            let output =
                                if frame.width == output_width && frame.height == output_height {
                                    frame.pixels.as_slice()
                                } else {
                                    if scaling == ScalingMode::FastBilinear {
                                        resize_bgra_fast(
                                            frame.pixels,
                                            frame.width,
                                            frame.height,
                                            output_width,
                                            output_height,
                                            &mut resized_frame,
                                        )?;
                                    } else {
                                        resized_frame = resize_bgra(
                                            frame.pixels,
                                            frame.width,
                                            frame.height,
                                            output_width,
                                            output_height,
                                            scaling,
                                        )?;
                                    }
                                    resized_frame.as_slice()
                                };
                            let resize_micros = elapsed_micros(resize_started);
                            let conversion_started = Instant::now();
                            let publish_frame = match output_pixel_format {
                                PixelFormat::Bgra => output,
                                PixelFormat::Nv12 => {
                                    convert_bgra_to_nv12(
                                        output,
                                        output_width,
                                        output_height,
                                        &mut converted_frame,
                                    )?;
                                    converted_frame.as_slice()
                                }
                                _ => unreachable!("output format was validated before capture"),
                            };
                            let conversion_micros = elapsed_micros(conversion_started);
                            let publish_started = Instant::now();
                            writer.write_now(publish_frame).map_err(|error| {
                                format!("writing frame exchange failed: {error}")
                            })?;
                            let publish_micros = elapsed_micros(publish_started);
                            metrics.observe(
                                capture_copy_micros,
                                filter_micros,
                                resize_micros,
                                conversion_micros,
                                publish_micros,
                                elapsed_micros(frame_started),
                            );
                            wrote_frame_this_run = true;
                            if let Some(sender) = ready_sender.take() {
                                let _ = sender.send(Ok(()));
                            }
                            Ok(())
                        },
                    );

                    match attempt {
                        Ok(summary) => break Ok(summary),
                        Err(error)
                            if ready_sender.is_none()
                                && !worker_stop.load(Ordering::Acquire)
                                && error.contains("MediaFrameReader stalled") =>
                        {
                            eprintln!(
                                "capture stream stalled; restarting in {} ms: {error}",
                                restart_delay.as_millis()
                            );
                            thread::sleep(restart_delay);
                            restart_delay = if wrote_frame_this_run {
                                Duration::from_millis(100)
                            } else {
                                (restart_delay * 2).min(Duration::from_secs(2))
                            };
                        }
                        Err(error) => break Err(error),
                    }
                };
                metrics.emit();
                if let Err(error) = &result {
                    if let Some(sender) = ready_sender.take() {
                        let _ = sender.send(Err(error.clone()));
                    }
                }
                result
            })
            .map_err(|error| internal_error(format!("starting capture worker failed: {error}")))?;

        match ready_receiver.recv_timeout(Duration::from_secs(12)) {
            Ok(Ok(())) => {
                self.session = Some(CaptureSession {
                    stop,
                    filter_graph: shared_filter_graph,
                    lut_assets: shared_lut_assets,
                    plugins,
                    thread: Some(worker),
                });
                Ok(())
            }
            Ok(Err(error)) => {
                stop.store(true, Ordering::Release);
                let _ = worker.join();
                Err(backend_error(error))
            }
            Err(RecvTimeoutError::Timeout) => {
                stop.store(true, Ordering::Release);
                let _ = worker.join();
                Err(backend_error(
                    "native capture did not produce its first frame within 12 seconds",
                ))
            }
            Err(RecvTimeoutError::Disconnected) => {
                stop.store(true, Ordering::Release);
                let result = worker.join();
                Err(backend_error(format!(
                    "native capture exited before its first frame: {result:?}"
                )))
            }
        }
    }

    fn close_session(&mut self) {
        if let Some(session) = self.session.take() {
            match session.stop() {
                Ok(summary) => eprintln!(
                    "capture stopped: frames={}, first_frame_ms={}, elapsed_ms={}",
                    summary.frames, summary.first_frame_millis, summary.elapsed_millis
                ),
                Err(error) => eprintln!("capture stopped with error: {error}"),
            }
        }
    }

    fn reap_finished_session(&mut self) {
        let finished = self
            .session
            .as_ref()
            .and_then(|session| session.thread.as_ref())
            .is_some_and(JoinHandle::is_finished);
        if finished {
            self.close_session();
        }
    }
}

fn backend_error(message: impl Into<String>) -> HostError {
    HostError {
        code: ErrorCode::BackendUnavailable,
        message: message.into(),
        native_code: None,
        retryable: Some(true),
    }
}

fn device_error(message: impl Into<String>) -> HostError {
    HostError {
        code: ErrorCode::DeviceAbsent,
        message: message.into(),
        native_code: None,
        retryable: Some(true),
    }
}

fn invalid_request(message: impl Into<String>) -> HostError {
    HostError {
        code: ErrorCode::InvalidRequest,
        message: message.into(),
        native_code: None,
        retryable: Some(false),
    }
}

fn internal_error(message: impl Into<String>) -> HostError {
    HostError {
        code: ErrorCode::Internal,
        message: message.into(),
        native_code: None,
        retryable: Some(false),
    }
}

fn serve() -> Result<(), String> {
    let mut host = CameraHost::new()?;
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| error.to_string())?;
        if line.len() > camera_protocol::MAX_MESSAGE_BYTES {
            return Err("camera host request exceeded the protocol limit".into());
        }
        let request: RequestEnvelope<HostCommand> =
            serde_json::from_str(&line).map_err(|error| error.to_string())?;
        let shutdown = matches!(request.payload, HostCommand::Shutdown);
        let result = if request.protocol_version == PROTOCOL_VERSION {
            host.execute(request.payload)
        } else {
            Err(HostError {
                code: ErrorCode::ProtocolMismatch,
                message: format!(
                    "unsupported protocol version {}; expected {}",
                    request.protocol_version, PROTOCOL_VERSION
                ),
                native_code: None,
                retryable: Some(false),
            })
        };
        let response = ResponseEnvelope {
            protocol_version: PROTOCOL_VERSION,
            request_id: request.request_id,
            result,
        };
        serde_json::to_writer(&mut stdout, &response).map_err(|error| error.to_string())?;
        stdout.write_all(b"\n").map_err(|error| error.to_string())?;
        stdout.flush().map_err(|error| error.to_string())?;
        if shutdown {
            break;
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = serve() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{convert_bgra_to_nv12, resize_bgra_fast, FilterGraphState};
    use camera_protocol::{FilterEffect, FilterGraph, FilterNode};

    fn brightness_graph(amount: f32) -> FilterGraph {
        FilterGraph {
            nodes: vec![FilterNode {
                id: "brightness".into(),
                enabled: true,
                label: None,
                effect: FilterEffect::Brightness { amount },
            }],
        }
    }

    #[test]
    fn filter_graph_revision_changes_only_for_new_content() {
        let graph = brightness_graph(0.1);
        let mut state = FilterGraphState {
            revision: 7,
            graph: graph.clone(),
        };

        state.replace(graph);
        assert_eq!(state.revision, 7);
        state.replace(brightness_graph(0.2));
        assert_eq!(state.revision, 8);
    }

    #[test]
    fn libyuv_converts_bgra_red_to_nv12_without_swapping_channels() {
        let source = [0_u8, 0, 255, 255].repeat(8);
        let mut destination = Vec::new();
        convert_bgra_to_nv12(&source, 4, 2, &mut destination).unwrap();

        assert_eq!(destination.len(), 12);
        assert!(destination[..8]
            .iter()
            .all(|value| (75..=90).contains(value)));
        for chroma in destination[8..].chunks_exact(2) {
            assert!(
                (80..=105).contains(&chroma[0]),
                "unexpected U: {}",
                chroma[0]
            );
            assert!(
                (225..=250).contains(&chroma[1]),
                "unexpected V: {}",
                chroma[1]
            );
        }
    }

    #[test]
    fn nv12_rejects_odd_dimensions() {
        let mut destination = Vec::new();
        assert!(convert_bgra_to_nv12(&[0; 12], 3, 1, &mut destination).is_err());
    }

    #[test]
    fn libyuv_fast_scaler_preserves_bgra_channel_order() {
        let source = [0_u8, 0, 255, 255].repeat(16);
        let mut destination = Vec::new();
        resize_bgra_fast(&source, 4, 4, 2, 2, &mut destination).unwrap();
        assert_eq!(destination, [0_u8, 0, 255, 255].repeat(4));
    }
}
