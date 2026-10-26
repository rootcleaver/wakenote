//! Primary input switching uses the same persisted settings/runtime path as
//! the Audio settings UI. The small notice window never takes keyboard focus.
use super::*;
use wakenote::dictation::dictation_shortcuts_overlap;

const NOTICE_WINDOW: &str = "input-notice";
const NOTICE_EVENT: &str = "microphone-cycle-notice";

#[derive(Default)]
pub struct Runtime {
    gate: tokio::sync::Mutex<()>,
    registered: Mutex<Option<String>>,
    notice: Mutex<Notice>,
    generation: AtomicU64,
}

#[derive(Clone, Default, Serialize)]
pub struct Notice {
    sequence: u64,
    label: String,
    detail: String,
    error: bool,
}

pub fn normalize(raw: &str) -> Result<String, String> {
    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    let value = validate_dictation_shortcut(raw)?;
    if modifier_shortcut(&value)?.is_some() || !value.contains('+') {
        return Err(
            "Cycle microphone requires a modifier and a main key, such as Ctrl+Shift+M".into(),
        );
    }
    Ok(value)
}

pub fn validate(settings: &AppSettings) -> Result<(), String> {
    let value = normalize(&settings.cycle_microphone_shortcut)?;
    if !value.is_empty() {
        for (other, _) in active_dictation_shortcuts(settings) {
            if dictation_shortcuts_overlap(&value, &other)? {
                return Err("Cycle Primary microphone shortcut overlaps a Dictation shortcut. Choose a different key combination.".into());
            }
        }
    }
    Ok(())
}

pub fn register(app: &AppHandle, raw: &str) -> Result<(), String> {
    let desired = normalize(raw)?;
    let runtime = app.state::<Runtime>();
    let mut current = runtime.registered.lock().map_err(|e| e.to_string())?;
    if current.as_deref().unwrap_or("") == desired {
        return Ok(());
    }
    let old = current.clone();
    if let Some(old) = &old {
        app.global_shortcut()
            .unregister(old.parse::<Shortcut>().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        *current = None;
    }
    if desired.is_empty() {
        return Ok(());
    }
    let shortcut = desired.parse::<Shortcut>().map_err(|e| e.to_string())?;
    let held = AtomicBool::new(false);
    let result = app
        .global_shortcut()
        .on_shortcut(shortcut, move |app, _, event| match event.state {
            ShortcutState::Released => {
                held.store(false, Ordering::SeqCst);
            }
            ShortcutState::Pressed => {
                if held.swap(true, Ordering::SeqCst)
                    || app
                        .state::<DictationShortcutSuspension>()
                        .0
                        .load(Ordering::SeqCst)
                {
                    return;
                }
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = cycle_primary_microphone(app).await {
                        eprintln!("[microphone-cycle] {error}");
                    }
                });
            }
        })
        .map_err(|e| {
            format!("Could not register Cycle Primary microphone shortcut '{desired}': {e}")
        });
    drop(current);
    if let Err(error) = result {
        if let Some(old) = old {
            if let Err(restore) = register(app, &old) {
                return Err(format!("{error}; shortcut restore failed: {restore}"));
            }
        }
        return Err(error);
    }
    *runtime.registered.lock().map_err(|e| e.to_string())? = Some(desired);
    Ok(())
}

/// Free the old cycle chord before replacing dictation chords, so swapping
/// assignments cannot collide with the app's own previous registration.
pub fn reconcile(
    app: &AppHandle,
    previous: &AppSettings,
    next: &AppSettings,
) -> Result<(), String> {
    validate(next)?;
    let old = app
        .state::<Runtime>()
        .registered
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
    let changing = old.as_deref().unwrap_or("") != next.cycle_microphone_shortcut;
    if changing {
        register(app, "")?;
    }
    if let Err(error) = reconcile_dictation_shortcut_registration(app, previous, next) {
        if changing {
            register(app, old.as_deref().unwrap_or(""))?;
        }
        return Err(error);
    }
    if let Err(error) = register(app, &next.cycle_microphone_shortcut) {
        let dictation_restore = reconcile_dictation_shortcut_registration(app, next, previous);
        let cycle_restore = register(app, old.as_deref().unwrap_or(""));
        return Err(format!(
            "{error}; restore: dictation={dictation_restore:?}, cycle={cycle_restore:?}"
        ));
    }
    Ok(())
}

struct Target {
    entry: CaptureMicrophoneEntry,
    changed: bool,
    alternatives: Vec<CaptureMicrophoneEntry>,
}

fn cycle_eligible(device: &MicrophoneDevice) -> bool {
    // macOS SDK: Continuity Capture wired, wireless, and the deprecated form.
    // Do not infer the device kind from a name such as “yoon's microphone”.
    let continuity = [*b"ccwd", *b"ccwl", *b"ccap"]
        .iter()
        .any(|kind| device.transport_type == Some(u32::from_be_bytes(*kind)));
    let identified = !cfg!(target_os = "macos") || device.transport_type.is_some();
    device.available && !device.fallback && identified && !continuity
}

fn next_input(
    current: &CaptureMicrophoneEntry,
    devices: &[MicrophoneDevice],
) -> Result<Target, String> {
    let mut candidates: Vec<&MicrophoneDevice> = Vec::new();
    for device in devices
        .iter()
        .filter(|d| cycle_eligible(d) && d.id != "default")
    {
        if !candidates.iter().any(|other| same_device(other, device)) {
            candidates.push(device);
        }
    }
    if candidates.is_empty() {
        candidates.extend(
            devices
                .iter()
                .filter(|d| cycle_eligible(d) && d.id == "default")
                .take(1),
        );
    }
    if candidates.is_empty() {
        return Err("No non-iPhone microphone is ready. Connect a Mac or external microphone and try again.".into());
    }
    let current_uid = current.core_audio_uid.as_deref().or_else(|| {
        devices
            .iter()
            .find(|d| d.id == current.id && d.available)
            .and_then(|d| d.core_audio_uid.as_deref())
    });
    let index = candidates.iter().position(|d| {
        d.id == current.id
            || current_uid.is_some_and(|uid| d.core_audio_uid.as_deref() == Some(uid))
    });
    let next = index.map_or(0, |index| (index + 1) % candidates.len());
    let device = candidates[next];
    Ok(Target {
        entry: CaptureMicrophoneEntry {
            id: device.id.clone(),
            label: device.label.clone(),
            core_audio_uid: device.core_audio_uid.clone(),
        },
        changed: index != Some(next),
        alternatives: (0..candidates.len())
            .map(|offset| (next + offset) % candidates.len())
            .filter(|candidate| Some(*candidate) != index)
            .map(|candidate| CaptureMicrophoneEntry {
                id: candidates[candidate].id.clone(),
                label: candidates[candidate].label.clone(),
                core_audio_uid: candidates[candidate].core_audio_uid.clone(),
            })
            .collect(),
    })
}

fn same_device(a: &MicrophoneDevice, b: &MicrophoneDevice) -> bool {
    a.id == b.id
        || a.core_audio_uid
            .as_ref()
            .is_some_and(|uid| b.core_audio_uid.as_ref() == Some(uid))
}

fn with_primary(
    settings: &AppSettings,
    primary: CaptureMicrophoneEntry,
) -> Vec<CaptureMicrophoneEntry> {
    std::iter::once(primary)
        .chain(settings.capture_microphones.iter().skip(1).cloned())
        .collect()
}

fn apply(app: &AppHandle, entries: Vec<CaptureMicrophoneEntry>) -> Result<AppSettings, String> {
    update_settings(
        app.clone(),
        app.state::<BackendState>(),
        app.state::<DictionaryFileState>(),
        app.state::<LiveCaptureState>(),
        app.state::<AutoTranscriptionState>(),
        app.state::<LiveTranscriberState>(),
        app.state::<SystemCaptureState>(),
        app.state::<SourceCaptureLifecycleState>(),
        app.state::<DetectedSourceState>(),
        app.state::<MeetingState>(),
        app.state::<VoiceLevelingPolicy>(),
        SettingsPatch {
            capture_microphones: Some(entries),
            ..Default::default()
        },
    )
}

#[tauri::command]
pub async fn cycle_primary_microphone(app: AppHandle) -> Result<AppSettings, String> {
    let runtime = app.state::<Runtime>();
    let _guard = runtime.gate.lock().await;
    let previous = app
        .state::<BackendState>()
        .lock()
        .map_err(|e| e.to_string())?
        .settings();
    if previous.recording_enabled
        && !previous.pause_all
        && permissions::permission_snapshot().microphone.status != PermissionGrantStatus::Granted
    {
        let granted =
            tauri::async_runtime::spawn_blocking(permissions::request_microphone_permission)
                .await
                .map_err(|error| error.to_string())?;
        if granted.microphone.status != PermissionGrantStatus::Granted {
            let error =
                "Allow microphone access in WakeNote Settings > Audio before switching live input"
                    .to_string();
            show_notice(
                &app,
                "Microphone permission required".into(),
                error.clone(),
                true,
            );
            return Err(error);
        }
        // Launch auto-start stops after an unavailable permission. Resume the
        // already selected input now that the user has allowed this app.
        let restart_app = app.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            start_live_capture_runtime(
                &restart_app,
                restart_app.state::<BackendState>().inner(),
                restart_app.state::<LiveCaptureState>().inner(),
                restart_app
                    .state::<AutoTranscriptionState>()
                    .inner()
                    .clone(),
            )
        })
        .await;
    }
    let devices = resolve_microphones_for_ui(previous.clone()).await;
    let current = previous
        .capture_microphones
        .first()
        .cloned()
        .unwrap_or_else(|| CaptureMicrophoneEntry {
            id: "default".into(),
            label: "System Default".into(),
            core_audio_uid: None,
        });
    let target = match next_input(&current, &devices) {
        Ok(target) => target,
        Err(error) => {
            show_notice(&app, "Microphone unavailable".into(), error.clone(), true);
            return Err(error);
        }
    };
    if !target.changed {
        show_notice(
            &app,
            target.entry.label,
            "Only one non-iPhone microphone is available".into(),
            false,
        );
        return Ok(previous);
    }
    let label = target.entry.label.clone();
    let app_for_work = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let was_active = app_for_work
            .state::<BackendState>()
            .lock()
            .map_err(|e| e.to_string())?
            .app_status()
            .live_input_active;
        let mut failures = Vec::new();
        for entry in target.alternatives {
            let attempted_label = entry.label.clone();
            let entries = with_primary(&previous, entry);
            let result = apply(&app_for_work, entries.clone()).and_then(|settings| {
                if was_active {
                    let status = app_for_work
                        .state::<BackendState>()
                        .lock()
                        .map_err(|e| e.to_string())?
                        .app_status();
                    if !status
                        .microphone_captures
                        .iter()
                        .any(|s| s.slot == MicrophoneSlot::Primary && s.active)
                    {
                        return Err(status
                            .microphone_warning
                            .unwrap_or_else(|| "New Primary microphone did not start".into()));
                    }
                }
                Ok(settings)
            });
            match result {
                Ok(settings) => return Ok((settings, failures.len())),
                Err(error) => {
                    // Persistence/registration failures or a newer manual
                    // selection must not be overwritten by another attempt.
                    let still_ours = app_for_work
                        .state::<BackendState>()
                        .lock()
                        .map(|b| b.settings().capture_microphones == entries)
                        .unwrap_or(false);
                    if !still_ours {
                        return Err(error);
                    }
                    eprintln!("[microphone-cycle] skipping {attempted_label}: {error}");
                    failures.push(format!("{attempted_label}: {error}"));
                }
            }
        }
        let restore = apply(&app_for_work, previous.capture_microphones).map(|_| ());
        Err(format!(
            "No other input became ready: {}{}",
            failures.join("; "),
            restore
                .err()
                .map(|e| format!("; previous input restore failed: {e}"))
                .unwrap_or_default()
        ))
    })
    .await
    .map_err(|e| e.to_string())?;
    let result = result.map_err(|error| format!("Could not switch to {label}: {error}"));
    match &result {
        Ok((settings, skipped)) => show_notice(
            &app,
            settings
                .capture_microphones
                .first()
                .map(|entry| entry.label.clone())
                .unwrap_or_else(|| label.clone()),
            if *skipped > 0 {
                "Primary microphone · skipped an unavailable input".into()
            } else {
                "Primary microphone switched".into()
            },
            false,
        ),
        Err(error) => {
            let active = app.state::<BackendState>().lock().ok().and_then(|backend| {
                backend
                    .app_status()
                    .microphone_captures
                    .into_iter()
                    .find(|slot| slot.slot == MicrophoneSlot::Primary && slot.active)
            });
            if let Some(active) = active {
                show_notice(
                    &app,
                    format!("Primary: {}", active.label),
                    format!("Could not switch to {label}"),
                    true,
                );
            } else {
                show_notice(
                    &app,
                    "Microphone needs attention".into(),
                    error.clone(),
                    true,
                );
            }
        }
    }
    result.map(|(settings, _)| settings)
}

#[tauri::command]
pub fn microphone_cycle_notice(app: AppHandle) -> Notice {
    app.state::<Runtime>()
        .notice
        .lock()
        .map(|notice| notice.clone())
        .unwrap_or_default()
}

fn show_notice(app: &AppHandle, label: String, detail: String, error: bool) {
    let runtime = app.state::<Runtime>();
    let sequence = runtime.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let payload = Notice {
        sequence,
        label,
        detail,
        error,
    };
    if let Ok(mut notice) = runtime.notice.lock() {
        *notice = payload.clone();
    }
    let app_for_ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        let result = (|| -> tauri::Result<()> {
            let window = if let Some(window) = app_for_ui.get_webview_window(NOTICE_WINDOW) {
                window
            } else {
                tauri::WebviewWindowBuilder::new(
                    &app_for_ui,
                    NOTICE_WINDOW,
                    tauri::WebviewUrl::App("quick-audio.html?notice=1".into()),
                )
                .title("WakeNote Input")
                .inner_size(360.0, 90.0)
                .resizable(false)
                .decorations(false)
                .visible(false)
                .focused(false)
                .focusable(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .build()?
            };
            window.set_ignore_cursor_events(true)?;
            let monitors = window.available_monitors()?;
            let cursor = window.cursor_position().ok();
            let monitor = cursor
                .and_then(|cursor| {
                    monitors.iter().find(|monitor| {
                        let origin = monitor.position();
                        let size = monitor.size();
                        cursor.x >= origin.x as f64
                            && cursor.x < origin.x as f64 + size.width as f64
                            && cursor.y >= origin.y as f64
                            && cursor.y < origin.y as f64 + size.height as f64
                    })
                })
                .cloned()
                .or(window.primary_monitor()?);
            if let Some(monitor) = monitor {
                let scale = monitor.scale_factor();
                window.set_position(tauri::PhysicalPosition::new(
                    monitor.position().x
                        + ((monitor.size().width as f64 - 360.0 * scale) / 2.0) as i32,
                    monitor.position().y + (64.0 * scale) as i32,
                ))?;
            }
            window.show()?;
            window.emit(NOTICE_EVENT, &payload)?;
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("[microphone-cycle] notice: {error}");
        }
    });
    let app_for_timer = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(2500));
        let ui = app_for_timer.clone();
        let _ = app_for_timer.run_on_main_thread(move || {
            if ui.state::<Runtime>().generation.load(Ordering::SeqCst) == sequence {
                if let Some(window) = ui.get_webview_window(NOTICE_WINDOW) {
                    let _ = window.hide();
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device(id: &str, uid: &str, available: bool) -> MicrophoneDevice {
        MicrophoneDevice {
            transport_type: Some(u32::from_be_bytes(*b"bltn")),
            id: id.into(),
            label: id.into(),
            core_audio_uid: Some(uid.into()),
            available,
            fallback: false,
        }
    }
    fn entry(id: &str, uid: Option<&str>) -> CaptureMicrophoneEntry {
        CaptureMicrophoneEntry {
            id: id.into(),
            label: id.into(),
            core_audio_uid: uid.map(str::to_owned),
        }
    }
    #[test]
    fn cycles_physical_devices_and_wraps_without_default_alias_or_unavailable_entries() {
        let devices = vec![
            device("default", "a", true),
            device("a", "a", true),
            device("missing", "x", false),
            device("b", "b", true),
            device("b-alias", "b", true),
        ];
        let next = next_input(&entry("default", None), &devices).unwrap();
        assert_eq!(next.entry.id, "b");
        assert_eq!(next.alternatives.len(), 1);
        assert!(next.changed);
        assert_eq!(
            next_input(&entry("b", Some("b")), &devices)
                .unwrap()
                .entry
                .id,
            "a"
        );
    }
    #[test]
    fn one_physical_device_does_not_restart_and_missing_current_can_recover() {
        let devices = vec![device("default", "a", true), device("a", "a", true)];
        assert!(
            !next_input(&entry("default", None), &devices)
                .unwrap()
                .changed
        );
        assert!(
            next_input(&entry("unplugged", Some("x")), &devices)
                .unwrap()
                .changed
        );
        assert!(next_input(&entry("default", None), &[]).is_err());
    }
    #[test]
    fn validates_chords_and_detects_overlap_with_modifier_only_dictation() {
        assert_eq!(normalize(" Shift + Control + M ").unwrap(), "ctrl+shift+m");
        assert!(normalize("m").is_err());
        assert!(normalize("ctrl+shift").is_err());
        let mut settings = AppSettings::default();
        settings.dictation_enabled = true;
        settings.dictation_shortcut = "ctrl+shift".into();
        assert!(validate(&settings).is_err());
        settings.cycle_microphone_shortcut.clear();
        assert!(validate(&settings).is_ok());
    }

    #[test]
    fn cycling_primary_preserves_secondary_even_when_it_is_the_next_device() {
        let mut settings = AppSettings::default();
        settings.capture_microphones = vec![entry("a", Some("a")), entry("b", Some("b"))];
        let target = next_input(
            &settings.capture_microphones[0],
            &[device("a", "a", true), device("b", "b", true)],
        )
        .unwrap();
        let entries = with_primary(&settings, target.entry);
        assert_eq!(entries[0], entry("b", Some("b")));
        assert_eq!(entries[1], settings.capture_microphones[1]);
    }

    #[test]
    fn alternatives_continue_past_an_unready_device_without_returning_to_current() {
        let devices = vec![
            device("phone", "phone", true),
            device("usb", "usb", true),
            device("built-in", "built-in", true),
        ];
        let target = next_input(&entry("built-in", Some("built-in")), &devices).unwrap();
        assert_eq!(
            target
                .alternatives
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            vec!["phone", "usb"]
        );
    }

    #[test]
    fn excludes_renamed_iphones_and_their_default_alias_before_attempting_capture() {
        for transport in [*b"ccwd", *b"ccwl", *b"ccap"] {
            let mut phone = device("phone", "phone-uid", true);
            phone.label = "yoon microphone".into();
            phone.transport_type = Some(u32::from_be_bytes(transport));
            let mut alias = phone.clone();
            alias.id = "default".into();
            let devices = vec![
                alias,
                phone.clone(),
                device("usb", "usb", true),
                device("mac", "mac", true),
            ];
            let next = next_input(&entry("mac", Some("mac")), &devices).unwrap();
            assert_eq!(next.entry.id, "usb");
            assert_eq!(next.alternatives.len(), 1);
            assert!(next_input(&entry("usb", Some("usb")), &[phone]).is_err());
            assert_eq!(
                next_input(&entry("phone", Some("phone-uid")), &devices)
                    .unwrap()
                    .entry
                    .id,
                "usb"
            );
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn an_unidentified_mac_input_is_not_opened_by_the_shortcut() {
        let mut unknown = device("renamed", "unknown", true);
        unknown.transport_type = None;
        assert!(next_input(&entry("mac", Some("mac")), &[unknown]).is_err());
    }

    #[test]
    fn a_timeout_fallback_is_not_treated_as_a_verified_replacement_device() {
        let mut fallback = device("default", "", true);
        fallback.fallback = true;
        assert!(next_input(&entry("usb", Some("usb")), &[fallback]).is_err());
    }
}
