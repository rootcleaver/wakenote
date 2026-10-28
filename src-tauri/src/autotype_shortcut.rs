//! Global shortcut that turns "Auto-type transcripts into cursor" on or off.
//! The toggle goes through the same persisted settings path as the Advanced
//! settings switch, and reuses the small Input notice window to show the state.
use super::*;
use wakenote::dictation::dictation_shortcuts_overlap;

#[derive(Default)]
pub struct Runtime {
    gate: tokio::sync::Mutex<()>,
    registered: Mutex<Option<String>>,
}

pub fn normalize(raw: &str) -> Result<String, String> {
    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    let value = validate_dictation_shortcut(raw)?;
    if modifier_shortcut(&value)?.is_some() || !value.contains('+') {
        return Err(
            "Auto-type toggle requires a modifier and a main key, such as Ctrl+Shift+A".into(),
        );
    }
    Ok(value)
}

pub fn validate(settings: &AppSettings) -> Result<(), String> {
    let value = normalize(&settings.auto_type_toggle_shortcut)?;
    if value.is_empty() {
        return Ok(());
    }
    for (other, _) in active_dictation_shortcuts(settings) {
        if dictation_shortcuts_overlap(&value, &other)? {
            return Err("Auto-type toggle shortcut overlaps a Dictation shortcut. Choose a different key combination.".into());
        }
    }
    let cycle = microphone_shortcut::normalize(&settings.cycle_microphone_shortcut)?;
    if !cycle.is_empty() && dictation_shortcuts_overlap(&value, &cycle)? {
        return Err("Auto-type toggle shortcut overlaps the Cycle Primary microphone shortcut. Choose a different key combination.".into());
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
                    if let Err(error) = toggle_auto_type(app).await {
                        eprintln!("[auto-type-toggle] {error}");
                    }
                });
            }
        })
        .map_err(|e| format!("Could not register Auto-type toggle shortcut '{desired}': {e}"));
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

/// Move the toggle chord to the next settings; on failure keep the old chord.
pub fn reconcile(app: &AppHandle, next: &AppSettings) -> Result<(), String> {
    validate(next)?;
    register(app, &next.auto_type_toggle_shortcut)
}

fn set_enabled(app: &AppHandle, enabled: bool) -> Result<AppSettings, String> {
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
            auto_transcript_input_enabled: Some(enabled),
            ..Default::default()
        },
    )
}

#[tauri::command]
pub async fn toggle_auto_type(app: AppHandle) -> Result<AppSettings, String> {
    let runtime = app.state::<Runtime>();
    let _guard = runtime.gate.lock().await;
    let enabled = !app
        .state::<BackendState>()
        .lock()
        .map_err(|e| e.to_string())?
        .settings()
        .auto_transcript_input_enabled;
    let app_for_work = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || set_enabled(&app_for_work, enabled))
        .await
        .map_err(|e| e.to_string())?;
    match &result {
        Ok(settings) => microphone_shortcut::show_notice(
            &app,
            if settings.auto_transcript_input_enabled {
                "Auto-type on".into()
            } else {
                "Auto-type off".into()
            },
            "Typing transcripts into the cursor".into(),
            false,
        ),
        Err(error) => microphone_shortcut::show_notice(
            &app,
            "Auto-type needs attention".into(),
            error.clone(),
            true,
        ),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_chords_and_rejects_overlaps() {
        assert_eq!(normalize(" Shift + Control + A ").unwrap(), "ctrl+shift+a");
        assert_eq!(normalize("  ").unwrap(), "");
        assert!(normalize("a").is_err());
        assert!(normalize("ctrl+shift").is_err());

        let mut settings = AppSettings::default();
        assert!(validate(&settings).is_ok());
        settings.auto_type_toggle_shortcut = "ctrl+shift+a".into();
        assert!(validate(&settings).is_ok());
        settings.auto_type_toggle_shortcut = settings.cycle_microphone_shortcut.clone();
        assert!(validate(&settings).is_err());
        settings.auto_type_toggle_shortcut = "ctrl+shift+a".into();
        settings.dictation_enabled = true;
        settings.dictation_shortcut = "ctrl+shift".into();
        assert!(validate(&settings).is_err());
    }
}
