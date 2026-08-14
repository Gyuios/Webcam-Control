//! Pure ownership and lifecycle rules for physical cameras.
//!
//! The lease manager is transport and backend agnostic. Holding a `CameraLease`
//! means that no other Webcam-Control component can open or configure the same
//! physical device. Dropping it releases the device, including during unwinding.

use camera_protocol::{BackendKind, DeviceId, VideoFormat};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LeasePurpose {
    Preview,
    VirtualOutput,
    ReadControls,
    WriteControl,
    PropertyPage,
    Diagnostics,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeaseSnapshot {
    pub device_id: DeviceId,
    pub token: u64,
    pub purpose: LeasePurpose,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeaseConflict {
    pub device_id: DeviceId,
    pub current_purpose: LeasePurpose,
}

impl fmt::Display for LeaseConflict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "device '{}' is already leased for {:?}",
            self.device_id, self.current_purpose
        )
    }
}

#[derive(Default)]
struct LeaseState {
    next_token: u64,
    by_device: HashMap<DeviceId, LeaseSnapshot>,
}

#[derive(Clone, Default)]
pub struct LeaseManager {
    state: Arc<Mutex<LeaseState>>,
}

impl LeaseManager {
    pub fn acquire(
        &self,
        device_id: impl Into<DeviceId>,
        purpose: LeasePurpose,
    ) -> Result<CameraLease, LeaseConflict> {
        let device_id = device_id.into();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = state.by_device.get(&device_id) {
            return Err(LeaseConflict {
                device_id,
                current_purpose: existing.purpose,
            });
        }
        state.next_token = state.next_token.wrapping_add(1).max(1);
        let snapshot = LeaseSnapshot {
            device_id: device_id.clone(),
            token: state.next_token,
            purpose,
        };
        state.by_device.insert(device_id, snapshot.clone());
        Ok(CameraLease {
            manager: self.clone(),
            snapshot: Some(snapshot),
        })
    }

    pub fn snapshot(&self) -> Vec<LeaseSnapshot> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut leases: Vec<_> = state.by_device.values().cloned().collect();
        leases.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        leases
    }

    pub fn is_leased(&self, device_id: &str) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.by_device.contains_key(device_id)
    }

    fn release(&self, snapshot: &LeaseSnapshot) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state
            .by_device
            .get(&snapshot.device_id)
            .is_some_and(|active| active.token == snapshot.token)
        {
            state.by_device.remove(&snapshot.device_id);
        }
    }
}

pub struct CameraLease {
    manager: LeaseManager,
    snapshot: Option<LeaseSnapshot>,
}

impl CameraLease {
    pub fn snapshot(&self) -> &LeaseSnapshot {
        self.snapshot.as_ref().expect("active lease has a snapshot")
    }
}

impl Drop for CameraLease {
    fn drop(&mut self) {
        if let Some(snapshot) = self.snapshot.take() {
            self.manager.release(&snapshot);
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CameraPhase {
    Absent,
    Idle,
    Opening,
    Streaming,
    Reconfiguring,
    Closing,
    Busy,
    PrivacyDenied,
    DeviceLost,
    Faulted,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraState {
    pub device_id: DeviceId,
    pub phase: CameraPhase,
    pub backend: Option<BackendKind>,
    pub format: Option<VideoFormat>,
    pub generation: u64,
    pub detail: Option<String>,
}

impl CameraState {
    pub fn new(device_id: impl Into<DeviceId>) -> Self {
        Self {
            device_id: device_id.into(),
            phase: CameraPhase::Idle,
            backend: None,
            format: None,
            generation: 0,
            detail: None,
        }
    }

    pub fn transition(
        &mut self,
        phase: CameraPhase,
        detail: Option<String>,
    ) -> Result<(), InvalidTransition> {
        if !valid_transition(self.phase, phase) {
            return Err(InvalidTransition {
                from: self.phase,
                to: phase,
            });
        }
        self.phase = phase;
        self.generation = self.generation.wrapping_add(1);
        self.detail = detail;
        if matches!(phase, CameraPhase::Idle | CameraPhase::Absent) {
            self.backend = None;
            self.format = None;
        }
        Ok(())
    }

    pub fn begin_open(
        &mut self,
        backend: BackendKind,
        format: VideoFormat,
    ) -> Result<(), InvalidTransition> {
        self.transition(CameraPhase::Opening, None)?;
        self.backend = Some(backend);
        self.format = Some(format);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidTransition {
    pub from: CameraPhase,
    pub to: CameraPhase,
}

impl fmt::Display for InvalidTransition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid camera transition {:?} -> {:?}",
            self.from, self.to
        )
    }
}

fn valid_transition(from: CameraPhase, to: CameraPhase) -> bool {
    use CameraPhase::*;
    matches!(
        (from, to),
        (Absent, Idle)
            | (Idle, Absent | Opening | Busy | PrivacyDenied | Faulted)
            | (
                Opening,
                Streaming | Closing | Busy | PrivacyDenied | DeviceLost | Faulted
            )
            | (Streaming, Reconfiguring | Closing | DeviceLost | Faulted)
            | (Reconfiguring, Streaming | Closing | DeviceLost | Faulted)
            | (Closing, Idle | Absent | DeviceLost | Faulted)
            | (Busy, Idle | Opening | Absent)
            | (PrivacyDenied, Idle | Opening | Absent)
            | (DeviceLost, Absent | Idle | Opening)
            | (Faulted, Closing | Idle | Absent | Opening)
    ) || from == to
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format_720p() -> VideoFormat {
        VideoFormat {
            width: 1280,
            height: 720,
            fps_numerator: 30,
            fps_denominator: 1,
            pixel_format: camera_protocol::PixelFormat::Nv12,
            subtype_guid: None,
        }
    }

    #[test]
    fn only_one_owner_can_lease_a_device() {
        let manager = LeaseManager::default();
        let preview = manager.acquire("uvc-1", LeasePurpose::Preview).unwrap();

        let conflict = manager
            .acquire("uvc-1", LeasePurpose::WriteControl)
            .err()
            .expect("the second owner must be rejected");
        assert_eq!(conflict.current_purpose, LeasePurpose::Preview);
        assert!(manager.is_leased("uvc-1"));

        drop(preview);
        assert!(!manager.is_leased("uvc-1"));
        assert!(manager.acquire("uvc-1", LeasePurpose::WriteControl).is_ok());
    }

    #[test]
    fn different_devices_can_be_used_concurrently() {
        let manager = LeaseManager::default();
        let _left = manager.acquire("uvc-1", LeasePurpose::Preview).unwrap();
        let _right = manager
            .acquire("capture-card", LeasePurpose::VirtualOutput)
            .unwrap();
        assert_eq!(manager.snapshot().len(), 2);
    }

    #[test]
    fn state_machine_accepts_normal_capture_lifecycle() {
        let mut state = CameraState::new("uvc-1");
        state
            .begin_open(BackendKind::MediaCapture, format_720p())
            .unwrap();
        state.transition(CameraPhase::Streaming, None).unwrap();
        state.transition(CameraPhase::Reconfiguring, None).unwrap();
        state.transition(CameraPhase::Streaming, None).unwrap();
        state.transition(CameraPhase::Closing, None).unwrap();
        state.transition(CameraPhase::Idle, None).unwrap();
        assert_eq!(state.generation, 6);
        assert_eq!(state.backend, None);
    }

    #[test]
    fn state_machine_rejects_streaming_without_opening() {
        let mut state = CameraState::new("uvc-1");
        let error = state
            .transition(CameraPhase::Streaming, None)
            .expect_err("idle cannot jump directly to streaming");
        assert_eq!(error.from, CameraPhase::Idle);
        assert_eq!(error.to, CameraPhase::Streaming);
    }
}
