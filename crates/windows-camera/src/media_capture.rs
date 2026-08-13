use camera_protocol::{
    canonical_device_id, BackendKind, CaptureProbeResult, PixelFormat, VideoFormat,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{sync_channel, Receiver, RecvTimeoutError},
    },
    time::{Duration, Instant},
};
use windows::{
    core::{Interface, HSTRING},
    Foundation::TypedEventHandler,
    Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap},
    Media::Capture::{
        Frames::{
            MediaFrameArrivedEventArgs, MediaFrameFormat, MediaFrameReader,
            MediaFrameReaderAcquisitionMode, MediaFrameReaderStartStatus, MediaFrameSource,
            MediaFrameSourceGroup, MediaFrameSourceInfo, MediaFrameSourceKind,
        },
        MediaCapture, MediaCaptureInitializationSettings, MediaCaptureMemoryPreference,
        MediaCaptureSharingMode, StreamingCaptureMode,
    },
    Storage::Streams::Buffer,
    Win32::System::WinRT::IBufferByteAccess,
};

use crate::MediaFoundationRuntime;

pub struct BgraFrame<'a> {
    pub width: u32,
    pub height: u32,
    pub timestamp_100ns: Option<i64>,
    pub copy_micros: u64,
    pub pixels: &'a mut Vec<u8>,
}

struct BgraCopyBuffer {
    buffer: Buffer,
    access: IBufferByteAccess,
    byte_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureStreamSummary {
    pub frames: u64,
    pub first_frame_millis: u64,
    pub elapsed_millis: u64,
}

/// Enumerates the modes that the production MediaFrameReader path can actually
/// select. Media Foundation SourceReader may advertise compressed modes (for
/// example H.264) that are not present in a MediaFrameSourceGroup.
pub fn enumerate_media_frame_formats(
    _runtime: &MediaFoundationRuntime,
    device_id: &str,
) -> Result<Vec<VideoFormat>, String> {
    let (group, source_infos) = find_source_group(device_id)?;
    let settings = capture_settings(&group)?;
    let capture = MediaCapture::new()
        .map_err(|error| winrt_error("Creating the MediaCapture session", error))?;
    let result = (|| {
        capture
            .InitializeWithSettingsAsync(&settings)
            .and_then(|operation| operation.join())
            .map_err(|error| winrt_error("Initializing MediaCapture", error))?;
        let sources = capture
            .FrameSources()
            .map_err(|error| winrt_error("Reading initialized MediaFrameSource objects", error))?;
        let mut result = Vec::new();
        for info in &source_infos {
            let id = info
                .Id()
                .map_err(|error| winrt_error("Reading MediaFrameSource id", error))?;
            let Ok(source) = sources.Lookup(&id) else {
                continue;
            };
            let formats = source
                .SupportedFormats()
                .map_err(|error| winrt_error("Reading MediaFrameSource formats", error))?;
            for format in &formats {
                if let Some(candidate) = winrt_video_format(&format) {
                    if candidate.pixel_format != PixelFormat::Unknown
                        && !result.iter().any(|known| formats_match(known, &candidate))
                    {
                        result.push(candidate);
                    }
                }
            }
        }
        result.sort_by_key(|format| {
            (
                std::cmp::Reverse(format.width.saturating_mul(format.height)),
                std::cmp::Reverse(
                    format.fps_numerator.saturating_mul(1000) / format.fps_denominator.max(1),
                ),
                format.pixel_format as u8,
            )
        });
        Ok(result)
    })();
    let _ = capture.Close();
    result
}

/// Opens a camera through the modern Windows MediaCapture/MediaFrameReader API
/// and proves that real frames can be acquired in the requested native mode.
pub fn probe_media_frame_reader(
    _runtime: &MediaFoundationRuntime,
    device_id: &str,
    requested: &VideoFormat,
    requested_frames: u32,
) -> Result<CaptureProbeResult, String> {
    if requested_frames == 0 || requested_frames > 300 {
        return Err("MediaFrameReader probe frames must be between 1 and 300".into());
    }

    let (capture, reader) = create_reader(device_id, requested)?;
    let result = probe_reader(&reader, requested, requested_frames);
    let _ = capture.Close();
    result
}

/// Streams CPU-accessible BGRA frames until `stop` is set or the device fails.
/// The callback runs on the caller's thread and must not retain native objects.
pub fn stream_media_frame_reader<F>(
    _runtime: &MediaFoundationRuntime,
    device_id: &str,
    requested: &VideoFormat,
    stop: &AtomicBool,
    mut on_frame: F,
) -> Result<CaptureStreamSummary, String>
where
    F: for<'a> FnMut(BgraFrame<'a>) -> Result<(), String>,
{
    let (capture, reader) = create_reader(device_id, requested)?;
    let (receiver, token) = start_reader(&reader)?;
    let started = Instant::now();
    let mut first_frame_millis = None;
    let mut frames = 0_u64;
    let mut last_frame = Instant::now();
    let mut copy_buffer = None;
    let mut pixels = Vec::new();
    let result = (|| {
        while !stop.load(Ordering::Acquire) {
            match receiver.recv_timeout(Duration::from_millis(250)) {
                Ok(()) => {}
                Err(RecvTimeoutError::Timeout) if last_frame.elapsed() < Duration::from_secs(3) => {
                    continue;
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "MediaFrameReader stalled after {frames} captured frames"
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("MediaFrameReader event channel disconnected".into());
                }
            }
            let frame = reader
                .TryAcquireLatestFrame()
                .map_err(|error| winrt_error("Acquiring the latest video frame", error))?;
            if Interface::as_raw(&frame).is_null() {
                continue;
            }
            let timestamp_100ns = frame
                .SystemRelativeTime()
                .ok()
                .filter(|reference| !Interface::as_raw(reference).is_null())
                .and_then(|reference| reference.Value().ok())
                .map(|timestamp| timestamp.Duration);
            let copy_started = Instant::now();
            copy_bgra_pixels(&frame, &mut copy_buffer, &mut pixels)?;
            let copy_micros = elapsed_micros(copy_started);
            let _ = frame.Close();
            frames = frames.saturating_add(1);
            first_frame_millis.get_or_insert(started.elapsed().as_millis() as u64);
            last_frame = Instant::now();
            on_frame(BgraFrame {
                width: requested.width,
                height: requested.height,
                timestamp_100ns,
                copy_micros,
                pixels: &mut pixels,
            })?;
        }
        Ok(CaptureStreamSummary {
            frames,
            first_frame_millis: first_frame_millis.unwrap_or_default(),
            elapsed_millis: started.elapsed().as_millis() as u64,
        })
    })();
    stop_reader(&reader, token);
    let _ = capture.Close();
    result
}

fn create_reader(
    device_id: &str,
    requested: &VideoFormat,
) -> Result<(MediaCapture, MediaFrameReader), String> {
    let (group, source_infos) = find_source_group(device_id)?;
    let settings = capture_settings(&group)?;

    let capture = MediaCapture::new()
        .map_err(|error| winrt_error("Creating the MediaCapture session", error))?;
    match initialize_reader(&capture, &settings, &source_infos, requested) {
        Ok(reader) => Ok((capture, reader)),
        Err(error) => {
            let _ = capture.Close();
            Err(error)
        }
    }
}

fn capture_settings(
    group: &MediaFrameSourceGroup,
) -> Result<MediaCaptureInitializationSettings, String> {
    let settings = MediaCaptureInitializationSettings::new()
        .map_err(|error| winrt_error("Creating MediaCapture settings", error))?;
    settings
        .SetSourceGroup(group)
        .map_err(|error| winrt_error("Selecting the MediaFrameSourceGroup", error))?;
    settings
        .SetStreamingCaptureMode(StreamingCaptureMode::Video)
        .map_err(|error| winrt_error("Selecting video-only capture", error))?;
    settings
        .SetSharingMode(MediaCaptureSharingMode::ExclusiveControl)
        .map_err(|error| winrt_error("Selecting exclusive camera control", error))?;
    settings
        .SetMemoryPreference(MediaCaptureMemoryPreference::Cpu)
        .map_err(|error| winrt_error("Selecting CPU-backed frames", error))?;

    Ok(settings)
}

fn initialize_reader(
    capture: &MediaCapture,
    settings: &MediaCaptureInitializationSettings,
    source_infos: &[MediaFrameSourceInfo],
    requested: &VideoFormat,
) -> Result<MediaFrameReader, String> {
    (|| {
        capture
            .InitializeWithSettingsAsync(settings)
            .and_then(|operation| operation.join())
            .map_err(|error| winrt_error("Initializing MediaCapture", error))?;
        let (source, format) = find_requested_source(capture, source_infos, requested)?;
        source
            .SetFormatAsync(&format)
            .and_then(|action| action.join())
            .map_err(|error| winrt_error("Selecting the MediaFrameReader format", error))?;
        capture
            .CreateFrameReaderWithSubtypeAsync(&source, &HSTRING::from("ARGB32"))
            .and_then(|operation| operation.join())
            .map_err(|error| winrt_error("Creating MediaFrameReader", error))
    })()
}

fn find_source_group(
    device_id: &str,
) -> Result<(MediaFrameSourceGroup, Vec<MediaFrameSourceInfo>), String> {
    let groups = MediaFrameSourceGroup::FindAllAsync()
        .and_then(|operation| operation.join())
        .map_err(|error| winrt_error("Enumerating MediaFrameSourceGroup objects", error))?;
    let requested_id = canonical_device_id(device_id);
    for group in &groups {
        let infos = group
            .SourceInfos()
            .map_err(|error| winrt_error("Reading source-group information", error))?;
        let mut color_sources = Vec::new();
        let mut device_matches = false;
        for info in &infos {
            if info.SourceKind().ok() != Some(MediaFrameSourceKind::Color) {
                continue;
            }
            if info
                .DeviceInformation()
                .and_then(|device| device.Id())
                .is_ok_and(|id| canonical_device_id(&id.to_string()) == requested_id)
            {
                device_matches = true;
            }
            color_sources.push(info);
        }
        if device_matches && !color_sources.is_empty() {
            return Ok((group, color_sources));
        }
    }
    Err("The selected camera has no matching MediaFrameSourceGroup".into())
}

fn find_requested_source(
    capture: &MediaCapture,
    source_infos: &[MediaFrameSourceInfo],
    requested: &VideoFormat,
) -> Result<(MediaFrameSource, MediaFrameFormat), String> {
    let sources = capture
        .FrameSources()
        .map_err(|error| winrt_error("Reading initialized MediaFrameSource objects", error))?;
    for info in source_infos {
        let id = info
            .Id()
            .map_err(|error| winrt_error("Reading MediaFrameSource id", error))?;
        let source = match sources.Lookup(&id) {
            Ok(source) => source,
            Err(_) => continue,
        };
        let formats = source
            .SupportedFormats()
            .map_err(|error| winrt_error("Reading MediaFrameSource formats", error))?;
        for format in &formats {
            if winrt_video_format(&format)
                .is_some_and(|candidate| formats_match(&candidate, requested))
            {
                return Ok((source, format));
            }
        }
    }
    Err("The requested mode is not exposed by MediaFrameReader".into())
}

fn probe_reader(
    reader: &MediaFrameReader,
    requested: &VideoFormat,
    requested_frames: u32,
) -> Result<CaptureProbeResult, String> {
    let started = Instant::now();
    let (receiver, token) = start_reader(reader)?;
    let result = collect_frames(reader, &receiver, requested, requested_frames, started);
    stop_reader(reader, token);
    result
}

fn start_reader(reader: &MediaFrameReader) -> Result<(Receiver<()>, i64), String> {
    reader
        .SetAcquisitionMode(MediaFrameReaderAcquisitionMode::Realtime)
        .map_err(|error| winrt_error("Selecting realtime frame acquisition", error))?;
    let (sender, receiver) = sync_channel(1);
    let handler =
        TypedEventHandler::<MediaFrameReader, MediaFrameArrivedEventArgs>::new(move |_, _| {
            let _ = sender.try_send(());
            Ok(())
        });
    let token = reader
        .FrameArrived(&handler)
        .map_err(|error| winrt_error("Subscribing to MediaFrameReader frames", error))?;
    let status = reader
        .StartAsync()
        .and_then(|operation| operation.join())
        .map_err(|error| winrt_error("Starting MediaFrameReader", error))?;
    if status != MediaFrameReaderStartStatus::Success {
        let _ = reader.RemoveFrameArrived(token);
        let _ = reader.Close();
        return Err(format!(
            "MediaFrameReader failed to start with status {}",
            status.0
        ));
    }
    Ok((receiver, token))
}

fn stop_reader(reader: &MediaFrameReader, token: i64) {
    let _ = reader.StopAsync().and_then(|action| action.join());
    let _ = reader.RemoveFrameArrived(token);
    let _ = reader.Close();
}

fn collect_frames(
    reader: &MediaFrameReader,
    receiver: &std::sync::mpsc::Receiver<()>,
    requested: &VideoFormat,
    requested_frames: u32,
    started: Instant,
) -> Result<CaptureProbeResult, String> {
    let deadline = Instant::now() + probe_timeout(requested, requested_frames);
    let mut received_frames = 0;
    let mut first_frame_millis = None;
    let mut first_timestamp = None;
    let mut last_timestamp = None;
    let mut pixels_validated = false;
    let mut copy_buffer = None;
    let mut pixels = Vec::new();
    while received_frames < requested_frames {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "MediaFrameReader timed out after receiving {received_frames}/{requested_frames} frames"
            ));
        }
        match receiver.recv_timeout(remaining) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => {
                return Err(format!(
                    "MediaFrameReader timed out after receiving {received_frames}/{requested_frames} frames"
                ));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err("MediaFrameReader event channel disconnected".into());
            }
        }
        let frame = reader
            .TryAcquireLatestFrame()
            .map_err(|error| winrt_error("Acquiring the latest video frame", error))?;
        if Interface::as_raw(&frame).is_null() {
            continue;
        }
        received_frames += 1;
        first_frame_millis.get_or_insert(started.elapsed().as_millis() as u64);
        if !pixels_validated {
            copy_bgra_pixels(&frame, &mut copy_buffer, &mut pixels)?;
            let expected = requested
                .width
                .checked_mul(requested.height)
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or("Invalid requested frame size")? as usize;
            if pixels.len() != expected {
                return Err(format!(
                    "MediaFrameReader returned {} BGRA bytes; expected {expected}",
                    pixels.len()
                ));
            }
            pixels_validated = true;
        }
        if let Ok(reference) = frame.SystemRelativeTime() {
            if !Interface::as_raw(&reference).is_null() {
                if let Ok(timestamp) = reference.Value() {
                    first_timestamp.get_or_insert(timestamp.Duration);
                    last_timestamp = Some(timestamp.Duration);
                }
            }
        }
        let _ = frame.Close();
    }

    Ok(CaptureProbeResult {
        backend: BackendKind::MediaCapture,
        format: requested.clone(),
        requested_frames,
        received_frames,
        first_frame_millis: first_frame_millis.unwrap_or_default(),
        elapsed_millis: started.elapsed().as_millis() as u64,
        first_timestamp_100ns: first_timestamp,
        last_timestamp_100ns: last_timestamp,
        stream_flags: 0,
    })
}

fn copy_bgra_pixels(
    frame: &windows::Media::Capture::Frames::MediaFrameReference,
    copy_buffer: &mut Option<BgraCopyBuffer>,
    pixels: &mut Vec<u8>,
) -> Result<(), String> {
    let video = frame
        .VideoMediaFrame()
        .map_err(|error| winrt_error("Reading the video frame", error))?;
    if Interface::as_raw(&video).is_null() {
        return Err("MediaFrameReader returned a frame without video data".into());
    }
    let source = video
        .SoftwareBitmap()
        .map_err(|error| winrt_error("Reading the frame SoftwareBitmap", error))?;
    if Interface::as_raw(&source).is_null() {
        return Err("MediaFrameReader returned no CPU-accessible SoftwareBitmap".into());
    }
    let bitmap = if source.BitmapPixelFormat().ok() == Some(BitmapPixelFormat::Bgra8) {
        source.clone()
    } else {
        SoftwareBitmap::Convert(&source, BitmapPixelFormat::Bgra8)
            .map_err(|error| winrt_error("Converting the frame to BGRA8", error))?
    };
    let width = bitmap
        .PixelWidth()
        .map_err(|error| winrt_error("Reading SoftwareBitmap width", error))?;
    let height = bitmap
        .PixelHeight()
        .map_err(|error| winrt_error("Reading SoftwareBitmap height", error))?;
    let byte_count = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .and_then(|bytes| u32::try_from(bytes).ok())
        .ok_or("Invalid SoftwareBitmap dimensions")?;
    if copy_buffer
        .as_ref()
        .is_none_or(|buffer| buffer.byte_count != byte_count)
    {
        let buffer = Buffer::Create(byte_count)
            .map_err(|error| winrt_error("Allocating the BGRA frame buffer", error))?;
        let access = buffer
            .cast()
            .map_err(|error| winrt_error("Accessing the BGRA frame buffer", error))?;
        *copy_buffer = Some(BgraCopyBuffer {
            buffer,
            access,
            byte_count,
        });
        pixels.resize(byte_count as usize, 0);
    }
    let copy_buffer = copy_buffer
        .as_ref()
        .ok_or("The BGRA copy buffer was not initialized")?;
    bitmap
        .CopyToBuffer(&copy_buffer.buffer)
        .map_err(|error| winrt_error("Copying SoftwareBitmap pixels", error))?;
    // SAFETY: the WinRT Buffer owns at least `byte_count` bytes and remains
    // alive while its contents are copied into the reusable owned buffer.
    unsafe {
        let pointer = copy_buffer
            .access
            .Buffer()
            .map_err(|error| winrt_error("Reading the BGRA frame pointer", error))?;
        if pointer.is_null() {
            return Err("The BGRA frame buffer returned a null pointer".into());
        }
        pixels.copy_from_slice(std::slice::from_raw_parts(pointer, byte_count as usize));
    }
    let _ = bitmap.Close();
    if bitmap != source {
        let _ = source.Close();
    }
    Ok(())
}

fn winrt_video_format(format: &MediaFrameFormat) -> Option<VideoFormat> {
    let dimensions = format.VideoFormat().ok()?;
    let rate = format.FrameRate().ok()?;
    let width = dimensions.Width().ok()?;
    let height = dimensions.Height().ok()?;
    let numerator = rate.Numerator().ok()?;
    let denominator = rate.Denominator().ok()?.max(1);
    if width == 0 || height == 0 {
        return None;
    }
    let subtype = format.Subtype().ok()?.to_string();
    Some(VideoFormat {
        width,
        height,
        fps_numerator: numerator,
        fps_denominator: denominator,
        pixel_format: pixel_format(&subtype),
        subtype_guid: None,
    })
}

fn pixel_format(subtype: &str) -> PixelFormat {
    match subtype.to_ascii_uppercase().as_str() {
        "NV12" => PixelFormat::Nv12,
        "YUY2" => PixelFormat::Yuy2,
        "MJPG" | "MJPEG" => PixelFormat::Mjpeg,
        "H264" => PixelFormat::H264,
        "ARGB32" | "BGRA8" | "RGB32" => PixelFormat::Bgra,
        _ => PixelFormat::Unknown,
    }
}

fn formats_match(candidate: &VideoFormat, requested: &VideoFormat) -> bool {
    candidate.width == requested.width
        && candidate.height == requested.height
        && candidate.fps_numerator as u64 * requested.fps_denominator as u64
            == requested.fps_numerator as u64 * candidate.fps_denominator as u64
        && candidate.pixel_format == requested.pixel_format
}

fn probe_timeout(format: &VideoFormat, frames: u32) -> Duration {
    let frame_seconds = format.fps_denominator as f64 / format.fps_numerator.max(1) as f64;
    Duration::from_secs_f64((frames as f64 * frame_seconds * 3.0 + 5.0).clamp(10.0, 90.0))
}

fn elapsed_micros(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

fn winrt_error(context: &str, error: windows::core::Error) -> String {
    format!(
        "{context} failed ({:#010X}): {error}",
        error.code().0 as u32
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_winrt_subtypes() {
        assert_eq!(pixel_format("NV12"), PixelFormat::Nv12);
        assert_eq!(pixel_format("mjpg"), PixelFormat::Mjpeg);
        assert_eq!(pixel_format("ARGB32"), PixelFormat::Bgra);
        assert_eq!(pixel_format("D16"), PixelFormat::Unknown);
    }

    #[test]
    fn probe_timeout_has_safe_bounds() {
        assert_eq!(probe_timeout(&format(30, 1), 30), Duration::from_secs(10));
        assert_eq!(probe_timeout(&format(1, 1), 300), Duration::from_secs(90));
    }

    #[test]
    fn semantic_format_match_ignores_backend_specific_metadata() {
        let candidate = format(30, 1);
        let mut requested = format(60, 2);
        requested.subtype_guid = Some("{3231564E-0000-0010-8000-00AA00389B71}".into());
        assert!(formats_match(&candidate, &requested));
        requested.pixel_format = PixelFormat::Yuy2;
        assert!(!formats_match(&candidate, &requested));
    }

    fn format(fps_numerator: u32, fps_denominator: u32) -> VideoFormat {
        VideoFormat {
            width: 1920,
            height: 1080,
            fps_numerator,
            fps_denominator,
            pixel_format: PixelFormat::Nv12,
            subtype_guid: None,
        }
    }
}
