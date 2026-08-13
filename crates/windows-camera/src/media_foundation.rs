use camera_protocol::{
    canonical_device_id, BackendKind, CameraDescriptor, CaptureProbeResult, PixelFormat,
    VideoFormat,
};
use std::{ptr, slice, time::Instant};
use windows::{
    core::GUID,
    Win32::{
        Media::MediaFoundation::{
            IMFActivate, IMFAttributes, IMFMediaSource, MFCreateAttributes,
            MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources, MFShutdown, MFStartup,
            MFVideoFormat_H264, MFVideoFormat_MJPG, MFVideoFormat_NV12, MFVideoFormat_RGB32,
            MFVideoFormat_YUY2, MFSTARTUP_LITE, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
            MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_MT_FRAME_RATE,
            MF_MT_FRAME_SIZE, MF_MT_SUBTYPE, MF_SOURCE_READERF_ENDOFSTREAM,
            MF_SOURCE_READERF_ERROR, MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION,
        },
        System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED},
    },
};

/// Owns COM and Media Foundation startup on the creating thread.
pub struct MediaFoundationRuntime {
    com_initialized: bool,
    media_foundation_started: bool,
}

impl MediaFoundationRuntime {
    pub fn start() -> Result<Self, String> {
        // SAFETY: camera-host owns this thread for its entire lifetime. COM is
        // initialized and uninitialized on the same thread, and MFShutdown is
        // called before CoUninitialize in Drop.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|error| format!("CoInitializeEx failed: {error}"))?;
            if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_LITE) {
                CoUninitialize();
                return Err(format!("MFStartup failed: {error}"));
            }
        }
        Ok(Self {
            com_initialized: true,
            media_foundation_started: true,
        })
    }
}

impl Drop for MediaFoundationRuntime {
    fn drop(&mut self) {
        // SAFETY: these calls balance successful startup on this same thread.
        unsafe {
            if self.media_foundation_started {
                let _ = MFShutdown();
            }
            if self.com_initialized {
                CoUninitialize();
            }
        }
    }
}

struct ActivateArray {
    pointer: *mut Option<IMFActivate>,
    count: usize,
}

impl ActivateArray {
    fn as_slice(&self) -> &[Option<IMFActivate>] {
        if self.pointer.is_null() || self.count == 0 {
            &[]
        } else {
            // SAFETY: MFEnumDeviceSources allocated `count` initialized COM
            // interface slots. This guard owns the array until Drop.
            unsafe { slice::from_raw_parts(self.pointer, self.count) }
        }
    }
}

impl Drop for ActivateArray {
    fn drop(&mut self) {
        // SAFETY: each Option<IMFActivate> must release its COM reference before
        // freeing the CoTaskMem array returned by MFEnumDeviceSources.
        unsafe {
            for index in 0..self.count {
                ptr::drop_in_place(self.pointer.add(index));
            }
            if !self.pointer.is_null() {
                CoTaskMemFree(Some(self.pointer.cast()));
            }
        }
    }
}

pub fn enumerate_devices(
    _runtime: &MediaFoundationRuntime,
) -> Result<Vec<CameraDescriptor>, String> {
    // SAFETY: all COM pointers are scoped by RAII guards and cloned before the
    // activation array is freed. Returned values contain no native pointers.
    unsafe {
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 1)
            .map_err(|error| format!("MFCreateAttributes failed: {error}"))?;
        let attributes = attributes.ok_or("MFCreateAttributes returned no object")?;
        attributes
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .map_err(|error| format!("Selecting video capture devices failed: {error}"))?;

        let activations = device_activations(&attributes)?;

        let mut devices = Vec::with_capacity(activations.count);
        for activation in activations.as_slice().iter().flatten() {
            let name = attribute_string(activation, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)?;
            let symbolic_link = attribute_string(
                activation,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
            )?;
            let (vendor_id, product_id) = usb_ids(&symbolic_link);
            devices.push(CameraDescriptor {
                id: canonical_device_id(&symbolic_link),
                name,
                formats: Vec::new(),
                vendor_id,
                product_id,
            });
        }
        Ok(devices)
    }
}

pub fn enumerate_formats_for_device(
    _runtime: &MediaFoundationRuntime,
    device_id: &str,
) -> Result<Vec<VideoFormat>, String> {
    // SAFETY: activation objects and the allocation returned by MF are owned by
    // the local RAII guard. Only the selected device is activated.
    unsafe {
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 1)
            .map_err(|error| format!("MFCreateAttributes failed: {error}"))?;
        let attributes = attributes.ok_or("MFCreateAttributes returned no object")?;
        attributes
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .map_err(|error| format!("Selecting video capture devices failed: {error}"))?;
        let activations = device_activations(&attributes)?;
        for activation in activations.as_slice().iter().flatten() {
            let symbolic_link = attribute_string(
                activation,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
            )?;
            if canonical_device_id(&symbolic_link) == canonical_device_id(device_id) {
                return enumerate_formats(activation);
            }
        }
        Err("The selected Media Foundation camera is no longer available".into())
    }
}

pub fn probe_source_reader(
    _runtime: &MediaFoundationRuntime,
    device_id: &str,
    requested: &VideoFormat,
    requested_frames: u32,
) -> Result<CaptureProbeResult, String> {
    if requested_frames == 0 || requested_frames > 300 {
        return Err("Source Reader probe frames must be between 1 and 300".into());
    }
    // SAFETY: activation, source, reader, media type and samples use COM smart
    // pointers. The source is shut down on every path after activation.
    unsafe {
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 1)
            .map_err(|error| format!("MFCreateAttributes failed: {error}"))?;
        let attributes = attributes.ok_or("MFCreateAttributes returned no object")?;
        attributes
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .map_err(|error| format!("Selecting video capture devices failed: {error}"))?;
        let activations = device_activations(&attributes)?;
        let activation = activations
            .as_slice()
            .iter()
            .flatten()
            .find(|activation| {
                attribute_string(
                    activation,
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                )
                .is_ok_and(|value| canonical_device_id(&value) == canonical_device_id(device_id))
            })
            .ok_or("The selected Media Foundation camera is no longer available")?;
        let source: IMFMediaSource = activation
            .ActivateObject()
            .map_err(|error| format!("Activating camera source failed: {error}"))?;
        let result = probe_active_source(&source, requested, requested_frames);
        let _ = source.Shutdown();
        result
    }
}

unsafe fn probe_active_source(
    source: &IMFMediaSource,
    requested: &VideoFormat,
    requested_frames: u32,
) -> Result<CaptureProbeResult, String> {
    // SAFETY: caller owns an active MF media source. All returned COM objects
    // are scoped to this function and no pointers cross the crate boundary.
    unsafe {
        let reader = MFCreateSourceReaderFromMediaSource(source, None::<&IMFAttributes>)
            .map_err(|error| format!("Creating MF Source Reader failed: {error}"))?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        reader
            .SetStreamSelection(stream, true)
            .map_err(|error| format!("Selecting the video stream failed: {error}"))?;
        let mut selected_type = None;
        for index in 0..4096_u32 {
            let media_type = match reader.GetNativeMediaType(stream, index) {
                Ok(media_type) => media_type,
                Err(_) => break,
            };
            if video_format(&media_type).as_ref() == Some(requested) {
                selected_type = Some(media_type);
                break;
            }
        }
        let selected_type =
            selected_type.ok_or("The requested native format is no longer available")?;
        reader
            .SetCurrentMediaType(stream, None, &selected_type)
            .map_err(|error| format!("Selecting the native format failed: {error}"))?;

        let started = Instant::now();
        let mut received_frames = 0_u32;
        let mut first_frame_millis = None;
        let mut first_timestamp = None;
        let mut last_timestamp = None;
        let mut combined_flags = 0_u32;
        let mut attempts = 0_u32;
        while received_frames < requested_frames && attempts < requested_frames.saturating_mul(8) {
            attempts += 1;
            let mut actual_stream = 0_u32;
            let mut flags = 0_u32;
            let mut timestamp = 0_i64;
            let mut sample = None;
            reader
                .ReadSample(
                    stream,
                    0,
                    Some(&mut actual_stream),
                    Some(&mut flags),
                    Some(&mut timestamp),
                    Some(&mut sample),
                )
                .map_err(|error| format!("Reading a Source Reader sample failed: {error}"))?;
            combined_flags |= flags;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err(format!(
                    "Source Reader reported stream error flags 0x{flags:08X}"
                ));
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            if sample.is_some() {
                received_frames += 1;
                first_frame_millis.get_or_insert(started.elapsed().as_millis() as u64);
                first_timestamp.get_or_insert(timestamp);
                last_timestamp = Some(timestamp);
            }
        }
        if received_frames == 0 {
            return Err("Source Reader returned no video frames".into());
        }
        Ok(CaptureProbeResult {
            backend: BackendKind::MfSourceReader,
            format: requested.clone(),
            requested_frames,
            received_frames,
            first_frame_millis: first_frame_millis.unwrap_or_default(),
            elapsed_millis: started.elapsed().as_millis() as u64,
            first_timestamp_100ns: first_timestamp,
            last_timestamp_100ns: last_timestamp,
            stream_flags: combined_flags,
        })
    }
}

unsafe fn device_activations(attributes: &IMFAttributes) -> Result<ActivateArray, String> {
    // SAFETY: MFEnumDeviceSources initializes the returned allocation and count.
    unsafe {
        let mut pointer: *mut Option<IMFActivate> = ptr::null_mut();
        let mut count = 0_u32;
        MFEnumDeviceSources(attributes, &mut pointer, &mut count)
            .map_err(|error| format!("MFEnumDeviceSources failed: {error}"))?;
        Ok(ActivateArray {
            pointer,
            count: count as usize,
        })
    }
}

unsafe fn attribute_string(attributes: &IMFAttributes, key: &GUID) -> Result<String, String> {
    // SAFETY: the buffer has the exact capacity requested by IMFAttributes and
    // remains alive for the duration of GetString.
    unsafe {
        let length = attributes
            .GetStringLength(key)
            .map_err(|error| format!("GetStringLength failed: {error}"))?;
        let mut buffer = vec![0_u16; length as usize + 1];
        attributes
            .GetString(key, &mut buffer, None)
            .map_err(|error| format!("GetString failed: {error}"))?;
        Ok(String::from_utf16_lossy(&buffer[..length as usize]))
    }
}

unsafe fn enumerate_formats(activation: &IMFActivate) -> Result<Vec<VideoFormat>, String> {
    // SAFETY: source and reader are COM smart pointers. Shutdown is issued
    // before the source is released, including when native type enumeration ends.
    unsafe {
        let source: IMFMediaSource = activation
            .ActivateObject()
            .map_err(|error| format!("Activating camera source failed: {error}"))?;
        let reader = match MFCreateSourceReaderFromMediaSource(&source, None::<&IMFAttributes>) {
            Ok(reader) => reader,
            Err(error) => {
                let _ = source.Shutdown();
                return Err(format!("Creating MF Source Reader failed: {error}"));
            }
        };
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let mut formats = Vec::new();
        for index in 0..4096_u32 {
            let media_type = match reader.GetNativeMediaType(stream, index) {
                Ok(media_type) => media_type,
                Err(_) => break,
            };
            if let Some(format) = video_format(&media_type) {
                formats.push(format);
            }
        }
        let _ = source.Shutdown();
        formats.sort_by_key(|format| {
            (
                format.width,
                format.height,
                format.fps_numerator,
                format.fps_denominator,
                format.pixel_format as u8,
            )
        });
        formats.dedup();
        Ok(formats)
    }
}

unsafe fn video_format(
    media_type: &windows::Win32::Media::MediaFoundation::IMFMediaType,
) -> Option<VideoFormat> {
    // SAFETY: IMFMediaType is a valid COM smart pointer and attribute getters do
    // not retain references to the keys.
    unsafe {
        let size = media_type.GetUINT64(&MF_MT_FRAME_SIZE).ok()?;
        let rate = media_type.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0);
        let subtype = media_type.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
        let width = (size >> 32) as u32;
        let height = size as u32;
        if width == 0 || height == 0 {
            return None;
        }
        Some(VideoFormat {
            width,
            height,
            fps_numerator: (rate >> 32) as u32,
            fps_denominator: (rate as u32).max(1),
            pixel_format: pixel_format(&subtype),
            subtype_guid: Some(format!("{subtype:?}")),
        })
    }
}

fn pixel_format(subtype: &GUID) -> PixelFormat {
    if *subtype == MFVideoFormat_NV12 {
        PixelFormat::Nv12
    } else if *subtype == MFVideoFormat_YUY2 {
        PixelFormat::Yuy2
    } else if *subtype == MFVideoFormat_MJPG {
        PixelFormat::Mjpeg
    } else if *subtype == MFVideoFormat_H264 {
        PixelFormat::H264
    } else if *subtype == MFVideoFormat_RGB32 {
        PixelFormat::Bgra
    } else {
        PixelFormat::Unknown
    }
}

fn usb_ids(symbolic_link: &str) -> (Option<u16>, Option<u16>) {
    let normalized = symbolic_link.to_ascii_lowercase();
    (
        hex_marker(&normalized, "vid_"),
        hex_marker(&normalized, "pid_"),
    )
}

fn hex_marker(value: &str, marker: &str) -> Option<u16> {
    let start = value.find(marker)? + marker.len();
    let digits = value.get(start..start + 4)?;
    u16::from_str_radix(digits, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_usb_ids_from_symbolic_link() {
        assert_eq!(
            usb_ids(r"\\?\usb#vid_046d&pid_0825#camera"),
            (Some(0x046d), Some(0x0825))
        );
        assert_eq!(usb_ids(r"\\?\swd#mmdevapi#camera"), (None, None));
    }

    #[test]
    fn maps_known_media_foundation_subtypes() {
        assert_eq!(pixel_format(&MFVideoFormat_NV12), PixelFormat::Nv12);
        assert_eq!(pixel_format(&MFVideoFormat_YUY2), PixelFormat::Yuy2);
        assert_eq!(pixel_format(&GUID::zeroed()), PixelFormat::Unknown);
    }
}
