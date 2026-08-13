//! Windows camera backend boundary.
//!
//! Native handles and COM objects stay inside this crate. The public API uses
//! only `camera-protocol` data structures so the rest of the application can be
//! tested without Windows hardware.

#[cfg(not(target_os = "windows"))]
compile_error!("windows-camera only supports Windows");

mod media_capture;
mod media_foundation;

pub use media_capture::{
    enumerate_media_frame_formats, probe_media_frame_reader, stream_media_frame_reader, BgraFrame,
    CaptureStreamSummary,
};
pub use media_foundation::{
    enumerate_devices, enumerate_formats_for_device, probe_source_reader, MediaFoundationRuntime,
};
