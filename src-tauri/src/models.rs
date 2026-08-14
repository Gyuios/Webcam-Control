use camera_protocol::{FilterPluginManifest, VideoFormat};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Camera {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) device_index: u32,
    #[serde(default, skip_serializing)]
    pub(crate) backend_id: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CameraControl {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) property: i32,
    pub(crate) minimum: i32,
    pub(crate) maximum: i32,
    pub(crate) step: i32,
    pub(crate) default_value: i32,
    pub(crate) value: i32,
    pub(crate) automatic: bool,
    pub(crate) supports_auto: bool,
    pub(crate) supports_manual: bool,
    pub(crate) default_automatic: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VirtualOutputOptions {
    pub(crate) camera_id: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) quality: String,
    #[serde(default)]
    pub(crate) input_format: Option<VideoFormat>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewStartResult {
    pub(crate) format: VideoFormat,
    pub(crate) preview_width: u32,
    pub(crate) preview_height: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VirtualCameraStatus {
    pub(crate) supported: bool,
    pub(crate) installed: bool,
    pub(crate) running: bool,
    pub(crate) name: &'static str,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) detail: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FrontendLogEntry {
    pub(crate) level: String,
    pub(crate) event: String,
    pub(crate) message: String,
    pub(crate) context: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticsInfo {
    pub(crate) directory: String,
    pub(crate) file: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FilterPluginCatalog {
    pub(crate) directory: String,
    pub(crate) plugins: Vec<FilterPluginManifest>,
    pub(crate) warnings: Vec<String>,
}
