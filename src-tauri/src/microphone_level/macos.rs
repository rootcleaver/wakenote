use std::ffi::c_void;
use std::mem::{MaybeUninit, offset_of, size_of};
use std::ptr::NonNull;
use std::sync::Arc;

use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectHasProperty,
    AudioObjectID, AudioObjectIsPropertySettable, AudioObjectPropertyAddress,
    AudioObjectSetPropertyData, kAudioDevicePropertyDeviceUID,
    kAudioDevicePropertyStreamConfiguration, kAudioDevicePropertyVolumeScalar,
    kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDevices,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeInput, kAudioObjectSystemObject,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList};
use objc2_core_foundation::{CFRetained, CFString};

use super::{DeviceVolume, MicrophoneLevelError, MicrophoneVolumeBackend, SystemInputDevice};
use crate::live_capture::stable_input_device_id;

pub const MAIN_ELEMENT: u32 = kAudioObjectPropertyElementMain;

trait AudioObjectApi: Send + Sync {
    fn input_devices(&self) -> Result<Vec<SystemInputDevice>, MicrophoneLevelError>;
    fn default_input_uid(&self) -> Result<Option<String>, MicrophoneLevelError>;
    fn input_channel_count(&self, uid: &str) -> Result<u32, MicrophoneLevelError>;
    fn has_volume_property(&self, uid: &str, element: u32) -> Result<bool, MicrophoneLevelError>;
    fn volume_is_settable(&self, uid: &str, element: u32) -> Result<bool, MicrophoneLevelError>;
    fn read_volume_scalar(&self, uid: &str, element: u32) -> Result<f32, MicrophoneLevelError>;
    fn write_volume_scalar(
        &self,
        uid: &str,
        element: u32,
        value: f32,
    ) -> Result<(), MicrophoneLevelError>;
}

#[derive(Debug, Clone, Copy, Default)]
struct SystemAudioObjectApi;

#[derive(Clone)]
pub struct CoreAudioInputVolumeBackend {
    api: Arc<dyn AudioObjectApi>,
}

pub type PlatformVolumeBackend = CoreAudioInputVolumeBackend;

impl CoreAudioInputVolumeBackend {
    pub fn new() -> Self {
        Self {
            api: Arc::new(SystemAudioObjectApi),
        }
    }
}

impl Default for CoreAudioInputVolumeBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CoreAudioInputVolumeBackend {
    #[cfg(test)]
    fn with_api(api: impl AudioObjectApi + 'static) -> Self {
        Self { api: Arc::new(api) }
    }

    fn volume_elements(&self, uid: &str) -> Result<Vec<u32>, MicrophoneLevelError> {
        if self.api.has_volume_property(uid, MAIN_ELEMENT)? {
            return Ok(vec![MAIN_ELEMENT]);
        }

        let channel_count = self.api.input_channel_count(uid)?;
        let mut elements = Vec::new();
        for element in 1..=channel_count {
            if self.api.has_volume_property(uid, element)? {
                elements.push(element);
            }
        }
        if elements.is_empty() {
            return Err(MicrophoneLevelError::VolumeUnavailable(uid.to_string()));
        }
        Ok(elements)
    }
}

impl MicrophoneVolumeBackend for CoreAudioInputVolumeBackend {
    fn input_devices(&self) -> Result<Vec<SystemInputDevice>, MicrophoneLevelError> {
        self.api.input_devices()
    }

    fn default_input_uid(&self) -> Result<Option<String>, MicrophoneLevelError> {
        self.api.default_input_uid()
    }

    fn read_volume(&self, uid: &str) -> Result<DeviceVolume, MicrophoneLevelError> {
        let elements = self.volume_elements(uid)?;
        let mut sum = 0.0_f32;
        let mut writable = false;
        for element in &elements {
            let scalar = self.api.read_volume_scalar(uid, *element)?;
            if !scalar.is_finite() {
                return Err(MicrophoneLevelError::Backend(format!(
                    "non-finite volume for {uid} element {element}"
                )));
            }
            sum += scalar.clamp(0.0, 1.0);
            writable |= self.api.volume_is_settable(uid, *element)?;
        }
        let mean = sum / elements.len() as f32;
        Ok(DeviceVolume {
            volume_percent: (mean * 100.0).round().clamp(0.0, 100.0) as u8,
            writable,
        })
    }

    fn write_volume(
        &self,
        uid: &str,
        volume_percent: u8,
    ) -> Result<DeviceVolume, MicrophoneLevelError> {
        let elements = self.volume_elements(uid)?;
        let scalar = f32::from(volume_percent.min(100)) / 100.0;
        let mut wrote = false;
        for element in elements {
            if self.api.volume_is_settable(uid, element)? {
                self.api.write_volume_scalar(uid, element, scalar)?;
                wrote = true;
            }
        }
        if !wrote {
            return Err(MicrophoneLevelError::ReadOnly(uid.to_string()));
        }
        self.read_volume(uid)
    }
}

impl AudioObjectApi for SystemAudioObjectApi {
    fn input_devices(&self) -> Result<Vec<SystemInputDevice>, MicrophoneLevelError> {
        let ids = audio_object_ids(
            kAudioObjectSystemObject as AudioObjectID,
            global_address(kAudioHardwarePropertyDevices),
            "enumerate devices",
        )?;
        let mut devices = Vec::new();
        for object_id in ids {
            if input_channel_count_for_object(object_id)? == 0 {
                continue;
            }
            let uid = string_property(
                object_id,
                global_address(kAudioDevicePropertyDeviceUID),
                "read device UID",
            )?;
            let label = string_property(
                object_id,
                global_address(kAudioObjectPropertyName),
                "read device name",
            )?;
            let legacy_cpal_id = Some(stable_input_device_id(devices.len(), &label));
            devices.push(SystemInputDevice {
                uid,
                label,
                legacy_cpal_id,
            });
        }
        Ok(devices)
    }

    fn default_input_uid(&self) -> Result<Option<String>, MicrophoneLevelError> {
        let object_id: AudioObjectID = property_value(
            kAudioObjectSystemObject as AudioObjectID,
            global_address(kAudioHardwarePropertyDefaultInputDevice),
            "read default input device",
        )?;
        if object_id == 0 {
            return Ok(None);
        }
        string_property(
            object_id,
            global_address(kAudioDevicePropertyDeviceUID),
            "read default input UID",
        )
        .map(Some)
    }

    fn input_channel_count(&self, uid: &str) -> Result<u32, MicrophoneLevelError> {
        input_channel_count_for_object(object_id_for_uid(uid)?)
    }

    fn has_volume_property(&self, uid: &str, element: u32) -> Result<bool, MicrophoneLevelError> {
        has_property(object_id_for_uid(uid)?, volume_address(element))
    }

    fn volume_is_settable(&self, uid: &str, element: u32) -> Result<bool, MicrophoneLevelError> {
        property_is_settable(object_id_for_uid(uid)?, volume_address(element))
    }

    fn read_volume_scalar(&self, uid: &str, element: u32) -> Result<f32, MicrophoneLevelError> {
        property_value(
            object_id_for_uid(uid)?,
            volume_address(element),
            "read input volume",
        )
    }

    fn write_volume_scalar(
        &self,
        uid: &str,
        element: u32,
        value: f32,
    ) -> Result<(), MicrophoneLevelError> {
        set_property_value(
            object_id_for_uid(uid)?,
            volume_address(element),
            &value,
            "write input volume",
        )
    }
}

/// Core Audio's transport survives renamed Continuity microphones.
pub fn input_transport_type(uid: &str) -> Result<u32, MicrophoneLevelError> {
    property_value(
        object_id_for_uid(uid)?,
        global_address(u32::from_be_bytes(*b"tran")),
        "read device transport",
    )
}

fn global_address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn input_address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeInput,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn volume_address(element: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyVolumeScalar,
        mScope: kAudioObjectPropertyScopeInput,
        mElement: element,
    }
}

fn object_id_for_uid(uid: &str) -> Result<AudioObjectID, MicrophoneLevelError> {
    let ids = audio_object_ids(
        kAudioObjectSystemObject as AudioObjectID,
        global_address(kAudioHardwarePropertyDevices),
        "enumerate devices",
    )?;
    for object_id in ids {
        let candidate = string_property(
            object_id,
            global_address(kAudioDevicePropertyDeviceUID),
            "read device UID",
        )?;
        if candidate == uid {
            return Ok(object_id);
        }
    }
    Err(MicrophoneLevelError::DeviceUnavailable(uid.to_string()))
}

fn input_channel_count_for_object(object_id: AudioObjectID) -> Result<u32, MicrophoneLevelError> {
    let address = input_address(kAudioDevicePropertyStreamConfiguration);
    let byte_size = property_data_size(object_id, address, "read input stream size")? as usize;
    if byte_size < size_of::<AudioBufferList>() {
        return Ok(0);
    }

    let word_count = byte_size.div_ceil(size_of::<usize>());
    let mut storage = vec![MaybeUninit::<usize>::uninit(); word_count];
    let returned = get_property_bytes(
        object_id,
        address,
        storage.as_mut_ptr().cast::<c_void>(),
        byte_size,
        "read input streams",
    )?;
    let list = storage.as_ptr().cast::<AudioBufferList>();
    // SAFETY: Core Audio initialized `returned` bytes and the size was checked above.
    let buffer_count = unsafe { (*list).mNumberBuffers as usize };
    let buffers_offset = offset_of!(AudioBufferList, mBuffers);
    let required =
        buffers_offset.saturating_add(buffer_count.saturating_mul(size_of::<AudioBuffer>()));
    if required > returned {
        return Err(MicrophoneLevelError::Backend(
            "invalid Core Audio input stream configuration".to_string(),
        ));
    }
    // SAFETY: `required <= returned` proves every indexed AudioBuffer is initialized.
    let buffers = unsafe {
        std::slice::from_raw_parts(
            storage
                .as_ptr()
                .cast::<u8>()
                .add(buffers_offset)
                .cast::<AudioBuffer>(),
            buffer_count,
        )
    };
    Ok(buffers.iter().map(|buffer| buffer.mNumberChannels).sum())
}

fn audio_object_ids(
    object_id: AudioObjectID,
    address: AudioObjectPropertyAddress,
    operation: &'static str,
) -> Result<Vec<AudioObjectID>, MicrophoneLevelError> {
    let byte_size = property_data_size(object_id, address, operation)? as usize;
    if !byte_size.is_multiple_of(size_of::<AudioObjectID>()) {
        return Err(MicrophoneLevelError::Backend(format!(
            "invalid Core Audio device list size: {byte_size}"
        )));
    }
    let mut values = vec![0; byte_size / size_of::<AudioObjectID>()];
    let returned = get_property_bytes(
        object_id,
        address,
        values.as_mut_ptr().cast::<c_void>(),
        byte_size,
        operation,
    )?;
    values.truncate(returned / size_of::<AudioObjectID>());
    Ok(values)
}

fn string_property(
    object_id: AudioObjectID,
    address: AudioObjectPropertyAddress,
    operation: &'static str,
) -> Result<String, MicrophoneLevelError> {
    let pointer: *mut CFString = property_value(object_id, address, operation)?;
    let pointer = NonNull::new(pointer).ok_or_else(|| {
        MicrophoneLevelError::Backend(format!("{operation} returned a null CFString"))
    })?;
    // SAFETY: Core Audio string properties follow the Create rule, so this +1 reference is
    // ours to release; `CFRetained` releases it on drop.
    let value = unsafe { CFRetained::from_raw(pointer) };
    Ok(value.to_string())
}

fn has_property(
    object_id: AudioObjectID,
    mut address: AudioObjectPropertyAddress,
) -> Result<bool, MicrophoneLevelError> {
    // SAFETY: `address` is a live property address for the duration of the call.
    Ok(unsafe { AudioObjectHasProperty(object_id, NonNull::from(&mut address)) })
}

fn property_is_settable(
    object_id: AudioObjectID,
    mut address: AudioObjectPropertyAddress,
) -> Result<bool, MicrophoneLevelError> {
    let mut settable = 0_u8;
    // SAFETY: both pointers refer to initialized, correctly sized values.
    let status = unsafe {
        AudioObjectIsPropertySettable(
            object_id,
            NonNull::from(&mut address),
            NonNull::from(&mut settable),
        )
    };
    check_status(status, "check input volume writability")?;
    Ok(settable != 0)
}

fn property_data_size(
    object_id: AudioObjectID,
    mut address: AudioObjectPropertyAddress,
    operation: &'static str,
) -> Result<u32, MicrophoneLevelError> {
    let mut size = 0_u32;
    // SAFETY: the property address and output-size pointer remain valid for the call.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            object_id,
            NonNull::from(&mut address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    check_status(status, operation)?;
    Ok(size)
}

fn property_value<T: Copy>(
    object_id: AudioObjectID,
    address: AudioObjectPropertyAddress,
    operation: &'static str,
) -> Result<T, MicrophoneLevelError> {
    let mut value = MaybeUninit::<T>::uninit();
    let returned = get_property_bytes(
        object_id,
        address,
        value.as_mut_ptr().cast::<c_void>(),
        size_of::<T>(),
        operation,
    )?;
    if returned != size_of::<T>() {
        return Err(MicrophoneLevelError::Backend(format!(
            "{operation} returned {returned} bytes, expected {}",
            size_of::<T>()
        )));
    }
    // SAFETY: the successful call initialized exactly `size_of::<T>()` bytes.
    Ok(unsafe { value.assume_init() })
}

fn get_property_bytes(
    object_id: AudioObjectID,
    mut address: AudioObjectPropertyAddress,
    output: *mut c_void,
    capacity: usize,
    operation: &'static str,
) -> Result<usize, MicrophoneLevelError> {
    let mut size = u32::try_from(capacity)
        .map_err(|_| MicrophoneLevelError::Backend("Core Audio buffer is too large".into()))?;
    let output = NonNull::new(output)
        .ok_or_else(|| MicrophoneLevelError::Backend("null Core Audio output buffer".into()))?;
    // SAFETY: `output` points to at least `capacity` writable bytes; all other pointers live
    // through the call and no qualifier data is required for these properties.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object_id,
            NonNull::from(&mut address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            output,
        )
    };
    check_status(status, operation)?;
    Ok(size as usize)
}

fn set_property_value<T>(
    object_id: AudioObjectID,
    mut address: AudioObjectPropertyAddress,
    value: &T,
    operation: &'static str,
) -> Result<(), MicrophoneLevelError> {
    let size = u32::try_from(size_of::<T>())
        .map_err(|_| MicrophoneLevelError::Backend("Core Audio value is too large".into()))?;
    let pointer = NonNull::from(value).cast::<c_void>();
    // SAFETY: `pointer` references an initialized `T` of `size` bytes for the call.
    let status = unsafe {
        AudioObjectSetPropertyData(
            object_id,
            NonNull::from(&mut address),
            0,
            std::ptr::null(),
            size,
            pointer,
        )
    };
    check_status(status, operation)
}

fn check_status(status: i32, operation: &'static str) -> Result<(), MicrophoneLevelError> {
    if status == 0 {
        Ok(())
    } else {
        Err(MicrophoneLevelError::CoreAudio { operation, status })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct FakeAudioObjectApi {
        state: Arc<Mutex<FakeAudioObjectState>>,
    }

    #[derive(Default)]
    struct FakeAudioObjectState {
        channel_count: u32,
        scalars: HashMap<u32, (f32, bool)>,
        writes: Vec<(u32, f32)>,
        failure: Option<MicrophoneLevelError>,
    }

    impl FakeAudioObjectApi {
        fn new() -> Self {
            Self::default()
        }

        fn with_channels(self, channels: impl IntoIterator<Item = u32>) -> Self {
            self.state.lock().unwrap().channel_count = channels.into_iter().max().unwrap_or(0);
            self
        }

        fn with_scalar(self, element: u32, value: f32, writable: bool) -> Self {
            self.state
                .lock()
                .unwrap()
                .scalars
                .insert(element, (value, writable));
            self
        }

        fn with_failure(self, error: MicrophoneLevelError) -> Self {
            self.state.lock().unwrap().failure = Some(error);
            self
        }

        fn writes(&self) -> Vec<(u32, f32)> {
            self.state.lock().unwrap().writes.clone()
        }
    }

    impl AudioObjectApi for FakeAudioObjectApi {
        fn input_devices(&self) -> Result<Vec<SystemInputDevice>, MicrophoneLevelError> {
            Ok(vec![SystemInputDevice {
                uid: "uid-a".into(),
                label: "USB Mic".into(),
                legacy_cpal_id: Some("input-0-usb-mic".into()),
            }])
        }

        fn default_input_uid(&self) -> Result<Option<String>, MicrophoneLevelError> {
            Ok(Some("uid-a".into()))
        }

        fn input_channel_count(&self, _uid: &str) -> Result<u32, MicrophoneLevelError> {
            let state = self.state.lock().unwrap();
            if let Some(error) = &state.failure {
                return Err(error.clone());
            }
            Ok(state.channel_count)
        }

        fn has_volume_property(
            &self,
            _uid: &str,
            element: u32,
        ) -> Result<bool, MicrophoneLevelError> {
            Ok(self.state.lock().unwrap().scalars.contains_key(&element))
        }

        fn volume_is_settable(
            &self,
            _uid: &str,
            element: u32,
        ) -> Result<bool, MicrophoneLevelError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .scalars
                .get(&element)
                .is_some_and(|(_, writable)| *writable))
        }

        fn read_volume_scalar(
            &self,
            _uid: &str,
            element: u32,
        ) -> Result<f32, MicrophoneLevelError> {
            self.state
                .lock()
                .unwrap()
                .scalars
                .get(&element)
                .map(|(value, _)| *value)
                .ok_or_else(|| MicrophoneLevelError::VolumeUnavailable(element.to_string()))
        }

        fn write_volume_scalar(
            &self,
            _uid: &str,
            element: u32,
            value: f32,
        ) -> Result<(), MicrophoneLevelError> {
            let mut state = self.state.lock().unwrap();
            let Some((stored, writable)) = state.scalars.get_mut(&element) else {
                return Err(MicrophoneLevelError::VolumeUnavailable(element.to_string()));
            };
            if !*writable {
                return Err(MicrophoneLevelError::ReadOnly(element.to_string()));
            }
            *stored = value;
            state.writes.push((element, value));
            Ok(())
        }
    }

    #[test]
    fn writable_main_volume_has_priority_over_channels() {
        let api = FakeAudioObjectApi::new()
            .with_channels([1, 2])
            .with_scalar(MAIN_ELEMENT, 0.42, true)
            .with_scalar(1, 0.20, true)
            .with_scalar(2, 0.80, true);
        let backend = CoreAudioInputVolumeBackend::with_api(api);
        let value = backend.read_volume("uid-a").unwrap();
        assert_eq!(
            value,
            DeviceVolume {
                volume_percent: 42,
                writable: true
            }
        );
    }

    #[test]
    fn channel_only_write_updates_every_writable_channel_and_reads_mean() {
        let api = FakeAudioObjectApi::new()
            .with_channels([1, 2])
            .with_scalar(1, 0.25, true)
            .with_scalar(2, 0.75, true);
        let backend = CoreAudioInputVolumeBackend::with_api(api.clone());
        let value = backend.write_volume("uid-a", 60).unwrap();
        assert_eq!(api.writes(), vec![(1, 0.60), (2, 0.60)]);
        assert_eq!(value.volume_percent, 60);
    }

    #[test]
    fn read_only_main_does_not_write_independent_channels() {
        let api = FakeAudioObjectApi::new()
            .with_channels([1, 2])
            .with_scalar(MAIN_ELEMENT, 0.55, false)
            .with_scalar(1, 0.20, true)
            .with_scalar(2, 0.80, true);
        let backend = CoreAudioInputVolumeBackend::with_api(api.clone());
        let value = backend.read_volume("uid-a").unwrap();
        assert_eq!(value.volume_percent, 55);
        assert!(!value.writable);
        assert!(matches!(
            backend.write_volume("uid-a", 60),
            Err(MicrophoneLevelError::ReadOnly(_))
        ));
        assert!(api.writes().is_empty());
    }

    #[test]
    fn mixed_channels_write_only_settable_elements_and_read_all_channels() {
        let api = FakeAudioObjectApi::new()
            .with_channels([1, 2])
            .with_scalar(1, 0.20, true)
            .with_scalar(2, 0.80, false);
        let backend = CoreAudioInputVolumeBackend::with_api(api.clone());
        let value = backend.write_volume("uid-a", 60).unwrap();
        assert_eq!(api.writes(), vec![(1, 0.60)]);
        assert_eq!(value.volume_percent, 70);
        assert!(value.writable);
    }

    #[test]
    fn missing_properties_and_nonzero_status_are_preserved() {
        let backend =
            CoreAudioInputVolumeBackend::with_api(FakeAudioObjectApi::new().with_channels([1, 2]));
        assert!(matches!(
            backend.read_volume("uid-a"),
            Err(MicrophoneLevelError::VolumeUnavailable(_))
        ));

        let expected = MicrophoneLevelError::CoreAudio {
            operation: "read test property",
            status: -50,
        };
        let backend = CoreAudioInputVolumeBackend::with_api(
            FakeAudioObjectApi::new().with_failure(expected.clone()),
        );
        assert_eq!(backend.read_volume("uid-a"), Err(expected));
    }

    #[test]
    fn device_uid_and_input_only_inventory_are_exposed_unchanged() {
        let backend = CoreAudioInputVolumeBackend::with_api(FakeAudioObjectApi::new());
        assert_eq!(
            backend.input_devices().unwrap(),
            vec![SystemInputDevice {
                uid: "uid-a".into(),
                label: "USB Mic".into(),
                legacy_cpal_id: Some("input-0-usb-mic".into()),
            }]
        );
        assert_eq!(
            backend.default_input_uid().unwrap().as_deref(),
            Some("uid-a")
        );
    }

    struct VolumeRestoreGuard<'a> {
        backend: &'a CoreAudioInputVolumeBackend,
        uid: String,
        volume_percent: u8,
    }

    impl Drop for VolumeRestoreGuard<'_> {
        fn drop(&mut self) {
            let _ = self.backend.write_volume(&self.uid, self.volume_percent);
        }
    }

    fn live_malloc_blocks() -> usize {
        let mut stats = MaybeUninit::<libc::malloc_statistics_t>::zeroed();
        // SAFETY: a null zone asks malloc for statistics across all zones into `stats`.
        unsafe { libc::malloc_zone_statistics(std::ptr::null_mut(), stats.as_mut_ptr()) };
        // SAFETY: the call above initialized every field.
        unsafe { stats.assume_init() }.blocks_in_use as usize
    }

    #[test]
    #[ignore = "reads real macOS Core Audio devices; run alone with --test-threads=1"]
    fn core_audio_string_properties_are_released() {
        const READS: usize = 200;
        let backend = CoreAudioInputVolumeBackend::new();
        let uid = backend
            .default_input_uid()
            .unwrap()
            .expect("the retention check needs a default input device");
        let read_all = || {
            backend.input_devices().unwrap();
            backend.default_input_uid().unwrap();
            let _ = backend.read_volume(&uid);
            crate::audio::list_input_devices();
        };
        for _ in 0..10 {
            read_all();
        }
        let before = live_malloc_blocks();
        for _ in 0..READS {
            read_all();
        }
        let retained = live_malloc_blocks().saturating_sub(before);
        assert!(
            retained < READS / 4,
            "{retained} malloc blocks retained across {READS} Core Audio reads"
        );
    }

    #[test]
    #[ignore = "mutates and restores the default macOS input volume"]
    fn core_audio_default_input_volume_round_trip() {
        if std::env::var("WAKENOTE_TEST_CORE_AUDIO_VOLUME").as_deref() != Ok("1") {
            return;
        }
        let backend = CoreAudioInputVolumeBackend::new();
        let uid = backend.default_input_uid().unwrap().expect("default input");
        let before = backend.read_volume(&uid).unwrap();
        assert!(before.writable, "default input volume is read-only");
        let _restore = VolumeRestoreGuard {
            backend: &backend,
            uid: uid.clone(),
            volume_percent: before.volume_percent,
        };
        let requested = if before.volume_percent >= 95 {
            before.volume_percent - 5
        } else {
            before.volume_percent + 5
        };
        let after = backend.write_volume(&uid, requested).unwrap();
        assert!(after.volume_percent.abs_diff(requested) <= 1);
    }
}
