//! Versioned, transport-independent contracts shared by Webcam-Control.
//!
//! This crate intentionally contains no Tauri, COM, Media Foundation or D3D
//! types. A camera host can therefore change implementation without changing
//! the UI-facing domain model or leaking native handles into serialized data.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

// Version 4 adds an explicit output pixel format to persistent capture. Keep
// this in lockstep with camera-host so a partially updated installation fails
// before it can publish a frame with the wrong layout.
pub const PROTOCOL_VERSION: u16 = 4;
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

pub type DeviceId = String;
pub type RequestId = u64;

/// Normalizes Windows camera symbolic links across API-specific interface GUIDs.
///
/// DirectShow and Media Foundation commonly return the same USB device prefix
/// followed by different `#{interface-class-guid}` suffixes. The prefix is the
/// cross-backend identity used by leases and profiles; the full symbolic link
/// remains private to the backend that opens the device.
pub fn canonical_device_id(value: &str) -> DeviceId {
    let normalized = value.trim().to_ascii_lowercase();
    normalized
        .find("#{")
        .map_or(normalized.clone(), |index| normalized[..index].to_string())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestEnvelope<T> {
    pub protocol_version: u16,
    pub request_id: RequestId,
    pub deadline_unix_ms: Option<u64>,
    pub payload: T,
}

impl<T> RequestEnvelope<T> {
    pub fn new(request_id: RequestId, payload: T) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            deadline_unix_ms: None,
            payload,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseEnvelope<T> {
    pub protocol_version: u16,
    pub request_id: RequestId,
    pub result: Result<T, HostError>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    InvalidRequest,
    ProtocolMismatch,
    DeadlineExceeded,
    DeviceAbsent,
    DeviceBusy,
    PrivacyDenied,
    UnsupportedFormat,
    UnsupportedControl,
    DriverRejected,
    DeviceLost,
    BackendUnavailable,
    Internal,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    MediaCapture,
    MfCaptureEngine,
    MfSourceReader,
    GStreamer,
    LegacyFfmpeg,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PixelFormat {
    Nv12,
    Yuy2,
    Mjpeg,
    H264,
    Bgra,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScalingMode {
    FastBilinear,
    QualityLanczos3,
    Ai,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoFormat {
    pub width: u32,
    pub height: u32,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
    pub pixel_format: PixelFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtype_guid: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureProbeResult {
    pub backend: BackendKind,
    pub format: VideoFormat,
    pub requested_frames: u32,
    pub received_frames: u32,
    pub first_frame_millis: u64,
    pub elapsed_millis: u64,
    pub first_timestamp_100ns: Option<i64>,
    pub last_timestamp_100ns: Option<i64>,
    pub stream_flags: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterSettings {
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
    pub gamma: f32,
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
    pub lens: LensCorrection,
    pub lut_strength: f32,
}

impl Default for FilterSettings {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            tint: 0.0,
            gamma: 1.0,
            flip_horizontal: false,
            flip_vertical: false,
            lens: LensCorrection::default(),
            lut_strength: 1.0,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LensCorrection {
    pub enabled: bool,
    pub k1: f32,
    pub k2: f32,
    pub k3: f32,
    pub p1: f32,
    pub p2: f32,
    pub scale: f32,
}

/// Ordered, repeatable software processing graph. Empty means true bypass.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterGraph {
    #[serde(default)]
    pub nodes: Vec<FilterNode>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterNode {
    pub id: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub effect: FilterEffect,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FilterEffect {
    Brightness {
        amount: f32,
    },
    Contrast {
        amount: f32,
    },
    Saturation {
        amount: f32,
    },
    Gamma {
        amount: f32,
    },
    Temperature {
        amount: f32,
    },
    Tint {
        amount: f32,
    },
    Flip {
        horizontal: bool,
        vertical: bool,
    },
    LensCorrection {
        k1: f32,
        k2: f32,
        k3: f32,
        p1: f32,
        p2: f32,
        scale: f32,
    },
    Lut3d {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        asset_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        strength: f32,
    },
    Plugin {
        plugin_id: String,
        #[serde(default)]
        parameters: BTreeMap<String, f32>,
    },
}

/// Safe v1 extension format. Plugins are data, never native DLLs. A plugin may
/// expose custom sliders and use them to modulate a 3x4 RGB color matrix.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterPluginManifest {
    pub schema_version: u16,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub parameters: Vec<PluginParameter>,
    pub processor: PluginProcessor,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginParameter {
    pub id: String,
    pub label: String,
    pub minimum: f32,
    pub maximum: f32,
    pub step: f32,
    pub default_value: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PluginProcessor {
    ColorMatrix {
        /// Row-major 3x4 matrix: RGB coefficients plus an offset per output.
        base: [f32; 12],
        #[serde(default)]
        modulations: Vec<MatrixModulation>,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixModulation {
    pub parameter: String,
    pub coefficient: u8,
    pub scale: f32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraDescriptor {
    pub id: DeviceId,
    pub name: String,
    #[serde(default)]
    pub formats: Vec<VideoFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_id: Option<u16>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ControlSource {
    VideoDeviceController,
    Ks,
    IamCameraControl,
    IamVideoProcAmp,
    VendorPlugin,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationPhase {
    PreStart,
    PostStart,
    Either,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlDescriptor {
    pub id: String,
    pub label: String,
    pub source: ControlSource,
    pub minimum: i64,
    pub maximum: i64,
    pub step: i64,
    pub default_value: i64,
    pub value: i64,
    pub automatic: bool,
    pub supports_auto: bool,
    pub supports_manual: bool,
    pub configuration_phase: ConfigurationPhase,
    pub supports_readback: bool,
    pub externally_mutable: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "command",
    content = "arguments",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HostCommand {
    Ping,
    EnumerateDevices,
    EnumerateFormats {
        device_id: DeviceId,
    },
    ProbeSourceReader {
        device_id: DeviceId,
        format: VideoFormat,
        frames: u32,
    },
    ProbeMediaFrameReader {
        device_id: DeviceId,
        format: VideoFormat,
        frames: u32,
    },
    Open {
        device_id: DeviceId,
        backend: BackendKind,
        format: VideoFormat,
        output_width: u32,
        output_height: u32,
        output_pixel_format: PixelFormat,
        scaling: ScalingMode,
        frame_path: String,
        filter_graph: FilterGraph,
        lut_assets: BTreeMap<String, String>,
        plugins: Vec<FilterPluginManifest>,
    },
    Close,
    SetFilterGraph {
        filter_graph: FilterGraph,
    },
    SetLutAsset {
        asset_id: String,
        cube: Option<String>,
    },
    EnumerateControls,
    SetControl {
        control_id: String,
        value: i64,
        automatic: bool,
    },
    OpenPropertyPage {
        parent_window: isize,
    },
    Shutdown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "response", content = "data", rename_all = "camelCase")]
pub enum HostResponse {
    Pong,
    Acknowledged,
    Devices(Vec<CameraDescriptor>),
    Formats(Vec<VideoFormat>),
    CaptureProbe(CaptureProbeResult),
    Controls(Vec<ControlDescriptor>),
    Diagnostics(Value),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trip_preserves_version_and_request() {
        let request = RequestEnvelope::new(42, HostCommand::Ping);
        let encoded = serde_json::to_vec(&request).unwrap();
        let decoded: RequestEnvelope<HostCommand> = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(decoded.protocol_version, PROTOCOL_VERSION);
        assert_eq!(decoded.request_id, 42);
        assert_eq!(decoded.payload, HostCommand::Ping);
        assert!(encoded.len() < MAX_MESSAGE_BYTES);
    }

    #[test]
    fn backend_names_are_stable_for_ipc() {
        assert_eq!(
            serde_json::to_string(&BackendKind::LegacyFfmpeg).unwrap(),
            "\"legacy-ffmpeg\""
        );
        assert_eq!(
            serde_json::to_string(&BackendKind::MfCaptureEngine).unwrap(),
            "\"mf-capture-engine\""
        );
    }

    #[test]
    fn command_fields_use_camel_case() {
        let command = HostCommand::EnumerateFormats {
            device_id: "camera-1".into(),
        };
        assert_eq!(
            serde_json::to_string(&command).unwrap(),
            r#"{"command":"enumerateFormats","arguments":{"deviceId":"camera-1"}}"#
        );
    }

    #[test]
    fn canonical_id_ignores_api_interface_guid() {
        let direct_show =
            r"\\?\usb#vid_1234&pid_abcd#instance#{65e8773d-8f56-11d0-a3b9-00a0c9223196}\global";
        let media_foundation =
            r"\\?\usb#vid_1234&pid_abcd#instance#{e5323777-f976-4f5b-9b55-b94699c46e44}\global";
        assert_eq!(
            canonical_device_id(direct_show),
            canonical_device_id(media_foundation)
        );
        assert_eq!(
            canonical_device_id(direct_show),
            r"\\?\usb#vid_1234&pid_abcd#instance"
        );
    }

    #[test]
    fn errors_carry_machine_readable_codes() {
        let response = ResponseEnvelope::<HostResponse> {
            protocol_version: PROTOCOL_VERSION,
            request_id: 7,
            result: Err(HostError {
                code: ErrorCode::DeviceBusy,
                message: "camera is already leased".into(),
                native_code: Some(0x8007_00AA_u32 as i64),
                retryable: Some(true),
            }),
        };
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(encoded.contains("deviceBusy"));
        assert!(encoded.contains("nativeCode"));
    }

    #[test]
    fn filter_graph_round_trip_preserves_order_and_repeated_types() {
        let graph = FilterGraph {
            nodes: vec![
                FilterNode {
                    id: "brightness-a".into(),
                    enabled: true,
                    label: None,
                    effect: FilterEffect::Brightness { amount: 0.1 },
                },
                FilterNode {
                    id: "brightness-b".into(),
                    enabled: false,
                    label: Some("Second pass".into()),
                    effect: FilterEffect::Brightness { amount: -0.2 },
                },
            ],
        };
        let encoded = serde_json::to_string(&graph).unwrap();
        assert!(encoded.contains(r#""type":"brightness""#));
        let decoded: FilterGraph = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, graph);
        assert_eq!(decoded.nodes[0].id, "brightness-a");
        assert_eq!(decoded.nodes[1].id, "brightness-b");
    }
}
