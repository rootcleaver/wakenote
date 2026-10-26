use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::settings::CaptureMicrophoneEntry;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(not(target_os = "macos"))]
pub mod macos {
    use super::{DeviceVolume, MicrophoneLevelError, MicrophoneVolumeBackend, SystemInputDevice};

    pub fn input_transport_type(_uid: &str) -> Result<u32, MicrophoneLevelError> {
        Err(MicrophoneLevelError::UnsupportedPlatform)
    }

    #[derive(Debug, Clone, Copy, Default)]
    pub struct CoreAudioInputVolumeBackend;

    pub type PlatformVolumeBackend = CoreAudioInputVolumeBackend;

    impl CoreAudioInputVolumeBackend {
        pub fn new() -> Self {
            Self
        }
    }

    impl MicrophoneVolumeBackend for CoreAudioInputVolumeBackend {
        fn input_devices(&self) -> Result<Vec<SystemInputDevice>, MicrophoneLevelError> {
            Err(MicrophoneLevelError::UnsupportedPlatform)
        }

        fn default_input_uid(&self) -> Result<Option<String>, MicrophoneLevelError> {
            Err(MicrophoneLevelError::UnsupportedPlatform)
        }

        fn read_volume(&self, _uid: &str) -> Result<DeviceVolume, MicrophoneLevelError> {
            Err(MicrophoneLevelError::UnsupportedPlatform)
        }

        fn write_volume(
            &self,
            _uid: &str,
            _volume_percent: u8,
        ) -> Result<DeviceVolume, MicrophoneLevelError> {
            Err(MicrophoneLevelError::UnsupportedPlatform)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInputDevice {
    pub uid: String,
    pub label: String,
    pub legacy_cpal_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceVolume {
    pub volume_percent: u8,
    pub writable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrophoneInputLevel {
    pub device_id: String,
    pub label: String,
    pub volume_percent: Option<u8>,
    pub writable: bool,
    pub available: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MicrophoneLevelError {
    #[error("microphone input-volume control is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("microphone device is unavailable: {0}")]
    DeviceUnavailable(String),
    #[error("Multiple devices are named {0}")]
    AmbiguousLabel(String),
    #[error("microphone input volume is read-only: {0}")]
    ReadOnly(String),
    #[error("microphone input volume is unavailable: {0}")]
    VolumeUnavailable(String),
    #[error("Core Audio {operation} failed with OSStatus {status}")]
    CoreAudio {
        operation: &'static str,
        status: i32,
    },
    #[error("microphone volume backend failed: {0}")]
    Backend(String),
}

impl MicrophoneLevelError {
    pub fn user_message(&self) -> String {
        match self {
            Self::UnsupportedPlatform => {
                "Microphone input-volume control is unavailable on this platform".to_string()
            }
            Self::DeviceUnavailable(_) => "Microphone is unavailable".to_string(),
            Self::AmbiguousLabel(label) => format!("Multiple devices are named {label}"),
            Self::ReadOnly(_) => "Microphone input volume is read-only".to_string(),
            Self::VolumeUnavailable(_) => {
                "This microphone does not expose macOS input volume".to_string()
            }
            Self::CoreAudio { operation, status } => {
                format!("Core Audio {operation} failed with OSStatus {status}")
            }
            Self::Backend(_) => "Microphone input-volume control failed".to_string(),
        }
    }
}

pub trait MicrophoneVolumeBackend: Send + Sync {
    fn input_devices(&self) -> Result<Vec<SystemInputDevice>, MicrophoneLevelError>;
    fn default_input_uid(&self) -> Result<Option<String>, MicrophoneLevelError>;
    fn read_volume(&self, uid: &str) -> Result<DeviceVolume, MicrophoneLevelError>;
    fn write_volume(
        &self,
        uid: &str,
        volume_percent: u8,
    ) -> Result<DeviceVolume, MicrophoneLevelError>;
}

pub struct MicrophoneLevelService<B> {
    backend: B,
}

impl<B> MicrophoneLevelService<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }
}

impl<B: MicrophoneVolumeBackend> MicrophoneLevelService<B> {
    pub fn resolved_uid_for(
        &self,
        configured: &CaptureMicrophoneEntry,
    ) -> Result<String, MicrophoneLevelError> {
        let devices = self.backend.input_devices()?;
        self.resolve_uid_from_devices(configured, &devices)
    }

    pub fn level_for(
        &self,
        configured: &CaptureMicrophoneEntry,
    ) -> Result<MicrophoneInputLevel, MicrophoneLevelError> {
        let devices = self.backend.input_devices()?;
        let uid = match self.resolve_uid_from_devices(configured, &devices) {
            Ok(uid) => uid,
            Err(error @ MicrophoneLevelError::DeviceUnavailable(_))
            | Err(error @ MicrophoneLevelError::AmbiguousLabel(_)) => {
                return Ok(unavailable_level(configured, error.user_message()));
            }
            Err(error) => return Err(error),
        };

        match self.backend.read_volume(&uid) {
            Ok(volume) => Ok(level_from_volume(configured, volume)),
            Err(error) => Ok(MicrophoneInputLevel {
                device_id: configured.id.clone(),
                label: configured.label.clone(),
                volume_percent: None,
                writable: false,
                available: true,
                error: Some(error.user_message()),
            }),
        }
    }

    pub fn set_volume(
        &self,
        configured: &CaptureMicrophoneEntry,
        volume_percent: u8,
    ) -> Result<MicrophoneInputLevel, MicrophoneLevelError> {
        let uid = self.resolved_uid_for(configured)?;
        let current = self.backend.read_volume(&uid)?;
        if !current.writable {
            return Err(MicrophoneLevelError::ReadOnly(configured.label.clone()));
        }
        self.backend.write_volume(&uid, volume_percent.min(100))?;
        let read_back = self.backend.read_volume(&uid)?;
        Ok(level_from_volume(configured, read_back))
    }

    fn resolve_uid_from_devices(
        &self,
        configured: &CaptureMicrophoneEntry,
        devices: &[SystemInputDevice],
    ) -> Result<String, MicrophoneLevelError> {
        if let Some(uid) = configured
            .core_audio_uid
            .as_deref()
            .map(str::trim)
            .filter(|uid| !uid.is_empty())
        {
            return devices
                .iter()
                .any(|device| device.uid == uid)
                .then(|| uid.to_string())
                .ok_or_else(|| MicrophoneLevelError::DeviceUnavailable(configured.label.clone()));
        }

        if configured.id == "default" {
            let uid = self
                .backend
                .default_input_uid()?
                .ok_or_else(|| MicrophoneLevelError::DeviceUnavailable(configured.label.clone()))?;
            return devices
                .iter()
                .any(|device| device.uid == uid)
                .then_some(uid)
                .ok_or_else(|| MicrophoneLevelError::DeviceUnavailable(configured.label.clone()));
        }

        if let Some(device) = devices.iter().find(|device| {
            device.legacy_cpal_id.as_deref() == Some(configured.id.as_str())
                && device.label == configured.label
        }) {
            return Ok(device.uid.clone());
        }

        let mut label_matches = devices
            .iter()
            .filter(|device| device.label == configured.label);
        let Some(first) = label_matches.next() else {
            return Err(MicrophoneLevelError::DeviceUnavailable(
                configured.label.clone(),
            ));
        };
        if label_matches.next().is_some() {
            return Err(MicrophoneLevelError::AmbiguousLabel(
                configured.label.clone(),
            ));
        }
        Ok(first.uid.clone())
    }
}

fn level_from_volume(
    configured: &CaptureMicrophoneEntry,
    volume: DeviceVolume,
) -> MicrophoneInputLevel {
    MicrophoneInputLevel {
        device_id: configured.id.clone(),
        label: configured.label.clone(),
        volume_percent: Some(volume.volume_percent.min(100)),
        writable: volume.writable,
        available: true,
        error: None,
    }
}

fn unavailable_level(configured: &CaptureMicrophoneEntry, error: String) -> MicrophoneInputLevel {
    MicrophoneInputLevel {
        device_id: configured.id.clone(),
        label: configured.label.clone(),
        volume_percent: None,
        writable: false,
        available: false,
        error: Some(error),
    }
}

pub fn levels_changed(previous: &[MicrophoneInputLevel], next: &[MicrophoneInputLevel]) -> bool {
    previous != next
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::settings::CaptureMicrophoneEntry;

    #[derive(Clone)]
    struct FakeVolumeBackend {
        state: Arc<Mutex<FakeVolumeState>>,
    }

    struct FakeVolumeState {
        devices: Vec<SystemInputDevice>,
        default_uid: Option<String>,
        volumes: HashMap<String, DeviceVolume>,
        quantize_to: Option<u8>,
        writes: Vec<(String, u8)>,
    }

    impl FakeVolumeBackend {
        fn with_devices(devices: impl IntoIterator<Item = FakeSystemInputDevice>) -> Self {
            let mut system_devices = Vec::new();
            let mut volumes = HashMap::new();
            for device in devices {
                volumes.insert(device.uid.clone(), device.volume);
                system_devices.push(SystemInputDevice {
                    uid: device.uid,
                    label: device.label,
                    legacy_cpal_id: device.legacy_cpal_id,
                });
            }
            Self {
                state: Arc::new(Mutex::new(FakeVolumeState {
                    default_uid: system_devices.first().map(|device| device.uid.clone()),
                    devices: system_devices,
                    volumes,
                    quantize_to: None,
                    writes: Vec::new(),
                })),
            }
        }

        fn with_default_uid(self, uid: &str) -> Self {
            self.state.lock().unwrap().default_uid = Some(uid.to_string());
            self
        }

        fn with_quantization(self, step: u8) -> Self {
            self.state.lock().unwrap().quantize_to = Some(step);
            self
        }

        fn writes(&self) -> Vec<(String, u8)> {
            self.state.lock().unwrap().writes.clone()
        }
    }

    impl MicrophoneVolumeBackend for FakeVolumeBackend {
        fn input_devices(&self) -> Result<Vec<SystemInputDevice>, MicrophoneLevelError> {
            Ok(self.state.lock().unwrap().devices.clone())
        }

        fn default_input_uid(&self) -> Result<Option<String>, MicrophoneLevelError> {
            Ok(self.state.lock().unwrap().default_uid.clone())
        }

        fn read_volume(&self, uid: &str) -> Result<DeviceVolume, MicrophoneLevelError> {
            self.state
                .lock()
                .unwrap()
                .volumes
                .get(uid)
                .copied()
                .ok_or_else(|| MicrophoneLevelError::DeviceUnavailable(uid.to_string()))
        }

        fn write_volume(
            &self,
            uid: &str,
            volume_percent: u8,
        ) -> Result<DeviceVolume, MicrophoneLevelError> {
            let mut state = self.state.lock().unwrap();
            let current = state
                .volumes
                .get(uid)
                .copied()
                .ok_or_else(|| MicrophoneLevelError::DeviceUnavailable(uid.to_string()))?;
            if !current.writable {
                return Err(MicrophoneLevelError::ReadOnly(uid.to_string()));
            }
            let requested = volume_percent.min(100);
            state.writes.push((uid.to_string(), requested));
            let applied = state
                .quantize_to
                .filter(|step| *step > 0)
                .map(|step| ((requested + step / 2) / step) * step)
                .unwrap_or(requested)
                .min(100);
            let volume = DeviceVolume {
                volume_percent: applied,
                writable: true,
            };
            state.volumes.insert(uid.to_string(), volume);
            Ok(volume)
        }
    }

    struct FakeSystemInputDevice {
        uid: String,
        label: String,
        legacy_cpal_id: Option<String>,
        volume: DeviceVolume,
    }

    fn device(uid: &str, label: &str, volume_percent: u8, writable: bool) -> FakeSystemInputDevice {
        FakeSystemInputDevice {
            uid: uid.to_string(),
            label: label.to_string(),
            legacy_cpal_id: None,
            volume: DeviceVolume {
                volume_percent,
                writable,
            },
        }
    }

    fn legacy_device(
        uid: &str,
        id: &str,
        label: &str,
        volume_percent: u8,
    ) -> FakeSystemInputDevice {
        FakeSystemInputDevice {
            legacy_cpal_id: Some(id.to_string()),
            ..device(uid, label, volume_percent, true)
        }
    }

    fn legacy_entry(id: &str, label: &str) -> CaptureMicrophoneEntry {
        CaptureMicrophoneEntry {
            id: id.to_string(),
            label: label.to_string(),
            core_audio_uid: None,
        }
    }

    #[test]
    fn explicit_uid_wins_over_duplicate_labels() {
        let backend = FakeVolumeBackend::with_devices([
            device("uid-a", "USB Mic", 30, true),
            device("uid-b", "USB Mic", 70, true),
        ]);
        let service = MicrophoneLevelService::new(backend);
        let configured = CaptureMicrophoneEntry {
            id: "input-1-usb-mic".into(),
            label: "USB Mic".into(),
            core_audio_uid: Some("uid-b".into()),
        };
        let level = service.level_for(&configured).unwrap();
        assert_eq!(level.volume_percent, Some(70));
        assert!(level.available);
    }

    #[test]
    fn ambiguous_legacy_label_refuses_hardware_control() {
        let backend = FakeVolumeBackend::with_devices([
            device("uid-a", "USB Mic", 30, true),
            device("uid-b", "USB Mic", 70, true),
        ]);
        let service = MicrophoneLevelService::new(backend);
        let level = service
            .level_for(&legacy_entry("input-9-usb-mic", "USB Mic"))
            .unwrap();
        assert_eq!(level.volume_percent, None);
        assert!(!level.writable);
        assert_eq!(
            level.error.as_deref(),
            Some("Multiple devices are named USB Mic")
        );
    }

    #[test]
    fn system_default_rebinds_to_the_current_default_uid() {
        let backend = FakeVolumeBackend::with_devices([
            device("uid-a", "Built-in Mic", 30, true),
            device("uid-b", "USB Mic", 65, true),
        ])
        .with_default_uid("uid-b");
        let service = MicrophoneLevelService::new(backend);
        let level = service
            .level_for(&legacy_entry("default", "System Default"))
            .unwrap();
        assert_eq!(level.volume_percent, Some(65));
    }

    #[test]
    fn resolved_default_uid_stays_pinned_for_an_active_capture() {
        let backend = FakeVolumeBackend::with_devices([
            device("uid-a", "Built-in Mic", 30, true),
            device("uid-b", "USB Mic", 65, true),
        ])
        .with_default_uid("uid-b");
        let service = MicrophoneLevelService::new(backend);
        let configured = CaptureMicrophoneEntry {
            id: "default".into(),
            label: "System Default".into(),
            core_audio_uid: Some("uid-a".into()),
        };

        assert_eq!(service.resolved_uid_for(&configured).unwrap(), "uid-a");
    }

    #[test]
    fn exact_legacy_id_and_label_resolve_before_a_duplicate_label() {
        let backend = FakeVolumeBackend::with_devices([
            legacy_device("uid-a", "input-0-usb-mic", "USB Mic", 40),
            legacy_device("uid-b", "input-1-usb-mic", "USB Mic", 80),
        ]);
        let service = MicrophoneLevelService::new(backend);
        let configured = legacy_entry("input-1-usb-mic", "USB Mic");
        assert_eq!(service.resolved_uid_for(&configured).unwrap(), "uid-b");
    }

    #[test]
    fn unavailable_explicit_uid_never_falls_back_by_label() {
        let backend = FakeVolumeBackend::with_devices([device("uid-present", "USB Mic", 50, true)]);
        let service = MicrophoneLevelService::new(backend);
        let configured = CaptureMicrophoneEntry {
            id: "input-0-usb-mic".into(),
            label: "USB Mic".into(),
            core_audio_uid: Some("uid-missing".into()),
        };
        let level = service.level_for(&configured).unwrap();
        assert!(!level.available);
        assert_eq!(level.volume_percent, None);
    }

    #[test]
    fn set_volume_clamps_and_returns_quantized_read_back() {
        let backend = FakeVolumeBackend::with_devices([device("uid-a", "USB Mic", 30, true)])
            .with_quantization(10);
        let writes = backend.clone();
        let service = MicrophoneLevelService::new(backend);
        let configured = legacy_entry("input-0-usb-mic", "USB Mic");
        let level = service.set_volume(&configured, 106).unwrap();
        assert_eq!(writes.writes(), vec![("uid-a".to_string(), 100)]);
        assert_eq!(level.volume_percent, Some(100));

        let level = service.set_volume(&configured, 3).unwrap();
        assert_eq!(level.volume_percent, Some(0));
    }

    #[test]
    fn set_volume_refuses_read_only_devices_without_writing() {
        let backend = FakeVolumeBackend::with_devices([device("uid-a", "USB Mic", 30, false)]);
        let writes = backend.clone();
        let service = MicrophoneLevelService::new(backend);
        let error = service
            .set_volume(&legacy_entry("input-0-usb-mic", "USB Mic"), 50)
            .unwrap_err();
        assert!(matches!(error, MicrophoneLevelError::ReadOnly(_)));
        assert!(writes.writes().is_empty());
    }

    #[test]
    fn observer_comparison_emits_only_for_real_changes() {
        let first = MicrophoneInputLevel {
            device_id: "default".into(),
            label: "System Default".into(),
            volume_percent: Some(50),
            writable: true,
            available: true,
            error: None,
        };
        assert!(!levels_changed(
            std::slice::from_ref(&first),
            std::slice::from_ref(&first)
        ));
        assert!(levels_changed(
            std::slice::from_ref(&first),
            &[MicrophoneInputLevel {
                volume_percent: Some(51),
                ..first.clone()
            }]
        ));
    }
}
