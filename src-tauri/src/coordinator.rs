use crate::diagnostics;
use camera_domain::{CameraLease, LeaseManager, LeasePurpose};
use serde_json::json;
use tauri::Manager;

#[derive(Default)]
pub(crate) struct CoordinatorState {
    leases: LeaseManager,
}

pub(crate) fn acquire_camera_lease(
    app: &tauri::AppHandle,
    camera_id: &str,
    purpose: LeasePurpose,
) -> Result<CameraLease, String> {
    let lease = app
        .state::<CoordinatorState>()
        .leases
        .acquire(camera_id, purpose)
        .map_err(|conflict| {
            diagnostics::log(
                "warn",
                "camera.coordinator",
                "lease.conflict",
                "La cámara ya tiene un propietario activo.",
                json!({
                    "cameraId": "[redacted]",
                    "requestedPurpose": format!("{purpose:?}"),
                    "currentPurpose": format!("{:?}", conflict.current_purpose),
                }),
            );
            format!(
                "La cámara está siendo utilizada por {:?}. Detén esa operación antes de continuar.",
                conflict.current_purpose
            )
        })?;
    diagnostics::log(
        "debug",
        "camera.coordinator",
        "lease.acquired",
        "Propiedad exclusiva de la cámara adquirida.",
        json!({
            "cameraId": "[redacted]",
            "purpose": format!("{purpose:?}"),
            "token": lease.snapshot().token,
        }),
    );
    Ok(lease)
}

pub(crate) fn release_camera_lease(lease: &mut Option<CameraLease>) {
    if let Some(active) = lease.take() {
        diagnostics::log(
            "debug",
            "camera.coordinator",
            "lease.released",
            "Propiedad exclusiva de la cámara liberada.",
            json!({
                "cameraId": "[redacted]",
                "purpose": format!("{:?}", active.snapshot().purpose),
                "token": active.snapshot().token,
            }),
        );
        drop(active);
    }
}
