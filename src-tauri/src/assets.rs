use std::path::PathBuf;
use tauri::Manager;

pub(crate) const BRIDGE_BINARY: &str = "control-webcam-bridge-x86_64-pc-windows-msvc.exe";
pub(crate) const CAMERA_HOST_BINARY: &str = "camera-tuner-camera-host-x86_64-pc-windows-msvc.exe";
pub(crate) const MEDIA_SOURCE_BINARY: &str = "camera-tuner-media-source.dll";
pub(crate) const VIRTUAL_CAMERA_CONTROL_BINARY: &str =
    "camera-tuner-virtual-camera-x86_64-pc-windows-msvc.exe";

pub(crate) fn binary_path(app: &tauri::AppHandle, name: &str) -> Result<PathBuf, String> {
    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(name);
    if development.is_file() {
        return Ok(development);
    }

    let packaged = app
        .path()
        .resource_dir()
        .map_err(|error| format!("No se pudo localizar un componente de CameraTuner: {error}"))?
        .join("binaries")
        .join(name);
    if packaged.is_file() {
        Ok(packaged)
    } else {
        Err(format!(
            "Falta el componente requerido '{name}'. Ejecuta scripts\\prepare-assets.ps1."
        ))
    }
}
