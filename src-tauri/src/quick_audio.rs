//! A lightweight menu-bar meter and an on-demand control surface. It consumes
//! the existing capture state; it never opens another audio stream.
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::image::Image;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

use crate::commands::{AppStatus, MicrophoneCaptureStatus};
use crate::settings::{AppSettings, MicrophoneSlot};

pub const WINDOW: &str = "quick-audio";
const TRAYS: [&str; 2] = ["audio-meter", "audio-meter-secondary"];
const WIDTH: f64 = 420.0;
const HEIGHT: f64 = 600.0;
const SAMPLES: usize = 32;
const INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Meter {
    active: bool,
    level: i32,
    threshold: i32,
    visible: bool,
    slot: MicrophoneSlot,
    configured: bool,
    identity: String,
    label: String,
}

impl Meter {
    fn from_slot(
        settings: &AppSettings,
        slot: MicrophoneSlot,
        runtime: Option<&MicrophoneCaptureStatus>,
    ) -> Self {
        let index = if slot == MicrophoneSlot::Primary {
            0
        } else {
            1
        };
        let configured = settings.capture_microphones.get(index);
        Self {
            active: configured.is_some()
                && runtime.is_some_and(|r| r.active && !r.reconnecting)
                && !settings.pause_all,
            level: finite_db(runtime.map_or(-120.0, |r| r.level.current_dbfs), -120),
            threshold: finite_db(settings.threshold_dbfs, -40),
            visible: settings.show_tray_icon,
            slot,
            configured: configured.is_some(),
            identity: format!(
                "{}:{}",
                configured
                    .and_then(|c| c.core_audio_uid.as_deref())
                    .or_else(|| configured.map(|c| c.id.as_str()))
                    .unwrap_or(""),
                runtime.map_or("", |r| r.device_id.as_str())
            ),
            label: runtime
                .map(|r| r.label.clone())
                .or_else(|| configured.map(|c| c.label.clone()))
                .unwrap_or_else(|| "Not configured".into()),
        }
    }

    fn title(&self) -> String {
        let slot = if self.slot == MicrophoneSlot::Primary {
            "P"
        } else {
            "S"
        };
        if !self.configured {
            format!("{slot} —")
        } else if self.active {
            format!("{slot} T{}", self.threshold)
        } else {
            format!("{slot} Off / T{}", self.threshold)
        }
    }

    fn tooltip(&self) -> String {
        format!(
            "{}: {} · {} · last 8 seconds · gold: threshold · click for controls",
            if self.slot == MicrophoneSlot::Primary {
                "Primary"
            } else {
                "Secondary"
            },
            self.label,
            if !self.configured {
                "not configured"
            } else if self.active {
                "live"
            } else {
                "unavailable"
            }
        )
    }

    fn image(&self, history: &History) -> Image<'static> {
        const W: usize = 64;
        const H: usize = 18;
        let mut rgba = vec![0; W * H * 4];
        let y_for = |db: f32| 1.0 + (1.0 - ((db + 90.0) / 90.0).clamp(0.0, 1.0)) * 15.0;
        let mut paint = |x: usize, y: usize, color: [u8; 4]| {
            rgba[(y * W + x) * 4..(y * W + x) * 4 + 4].copy_from_slice(&color);
        };
        for x in 0..W {
            if !self.configured {
                continue;
            }
            paint(
                x,
                y_for(self.threshold as f32).round() as usize,
                [235, 177, 77, 180],
            );
        }
        if self.active {
            let color = if self.slot == MicrophoneSlot::Primary {
                [88, 170, 250]
            } else {
                [111, 215, 159]
            };
            let points = history.snapshot();
            for x in 0..W {
                let at = x as f32 / (W - 1) as f32 * (SAMPLES - 1) as f32;
                let left = at.floor() as usize;
                let right = at.ceil() as usize;
                if let (Some(a), Some(b)) = (points[left], points[right]) {
                    let y = y_for(a + (b - a) * at.fract());
                    let top = y.floor() as usize;
                    // Two pixels with weighted alpha keep a small line legible.
                    paint(
                        x,
                        top,
                        [
                            color[0],
                            color[1],
                            color[2],
                            ((1.0 - y.fract()) * 255.0) as u8,
                        ],
                    );
                    paint(
                        x,
                        top + 1,
                        [color[0], color[1], color[2], (y.fract() * 255.0) as u8],
                    );
                }
            }
        }
        Image::new_owned(rgba, W as u32, H as u32)
    }
}

fn finite_db(value: f32, fallback: i32) -> i32 {
    if value.is_finite() {
        value.round().clamp(-120.0, 0.0) as i32
    } else {
        fallback
    }
}

#[derive(Default)]
struct History {
    values: VecDeque<Option<f32>>,
    last: Option<Instant>,
    smoothed: Option<f32>,
}

impl History {
    fn sample(&mut self, now: Instant, level: i32) -> bool {
        let steps = self.last.map_or(1, |last| {
            now.duration_since(last).as_millis() / INTERVAL.as_millis()
        });
        if steps == 0 {
            return false;
        }
        if steps > 1 {
            self.smoothed = None;
            for _ in 0..steps.saturating_sub(1).min(SAMPLES as u128) {
                self.push(None);
            }
        }
        self.last = Some(self.last.map_or(now, |last| {
            last + INTERVAL * steps.min(u32::MAX as u128) as u32
        }));
        let value = self.smoothed.map_or(level as f32, |previous| {
            previous * 0.65 + level as f32 * 0.35
        });
        self.smoothed = Some(value);
        self.push(Some(value));
        true
    }

    fn push(&mut self, value: Option<f32>) {
        self.values.push_back(value);
        while self.values.len() > SAMPLES {
            self.values.pop_front();
        }
    }

    fn snapshot(&self) -> Vec<Option<f32>> {
        std::iter::repeat_n(None, SAMPLES - self.values.len())
            .chain(self.values.iter().copied())
            .collect()
    }
}

#[derive(Default)]
struct SlotCache {
    meter: Option<Meter>,
    history: History,
}

impl SlotCache {
    fn update(&mut self, meter: Meter, now: Instant) -> Option<(Meter, Image<'static>)> {
        let changed = self.meter.as_ref().is_none_or(|old| {
            old.visible != meter.visible
                || old.active != meter.active
                || old.threshold != meter.threshold
                || old.identity != meter.identity
                || old.label != meter.label
                || old.configured != meter.configured
        });
        if self
            .meter
            .as_ref()
            .is_some_and(|old| old.active != meter.active || old.identity != meter.identity)
        {
            self.history = History::default();
        }
        let sampled = meter.active && self.history.sample(now, meter.level);
        self.meter = Some(meter.clone());
        (changed || sampled).then(|| {
            let image = meter.image(&self.history);
            (meter, image)
        })
    }
}

#[derive(Default)]
struct MeterCache {
    slots: [SlotCache; 2],
}

#[derive(serde::Serialize)]
pub struct SlotHistories {
    primary: Vec<Option<f32>>,
    secondary: Vec<Option<f32>>,
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    app.manage(Mutex::new(MeterCache::default()));
    // A separate meter keeps the existing tray's configured click actions.
    for (index, tray_id) in TRAYS.iter().enumerate() {
        TrayIconBuilder::with_id(*tray_id)
            .icon(
                Meter {
                    active: false,
                    level: -120,
                    threshold: -40,
                    visible: true,
                    slot: if index == 0 {
                        MicrophoneSlot::Primary
                    } else {
                        MicrophoneSlot::Secondary
                    },
                    configured: false,
                    identity: String::new(),
                    label: "Not configured".into(),
                }
                .image(&History::default()),
            )
            .tooltip("WakeNote audio · click to adjust threshold and microphones")
            .show_menu_on_left_click(false)
            .on_tray_icon_event(|tray, event| {
                #[cfg(debug_assertions)]
                if matches!(event, TrayIconEvent::Click { .. }) {
                    eprintln!("[quick-audio] tray event {event:?}");
                }
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    position,
                    ..
                } = event
                {
                    if let Err(error) = toggle(tray.app_handle(), position) {
                        eprintln!("[quick-audio] {error}");
                    }
                }
            })
            .build(app)?;
    }
    Ok(())
}

fn toggle(app: &AppHandle, position: PhysicalPosition<f64>) -> tauri::Result<()> {
    let window = if let Some(window) = app.get_webview_window(WINDOW) {
        window
    } else {
        let window =
            WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("quick-audio.html".into()))
                .title("WakeNote Audio")
                .inner_size(WIDTH, HEIGHT)
                .resizable(false)
                .decorations(false)
                .visible(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .build()?;
        let handle = window.clone();
        window.on_window_event(move |event| match event {
            tauri::WindowEvent::Focused(false) => {
                #[cfg(debug_assertions)]
                eprintln!(
                    "[quick-audio] blur visible={:?} focused={:?}",
                    handle.is_visible(),
                    handle.is_focused()
                );
                let _ = handle.hide();
            }
            #[cfg(debug_assertions)]
            tauri::WindowEvent::Focused(true) => eprintln!("[quick-audio] focused"),
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = handle.hide();
            }
            _ => {}
        });
        window
    };
    if window.is_visible()? {
        #[cfg(debug_assertions)]
        eprintln!("[quick-audio] toggle hides visible panel");
        return window.hide();
    }
    let monitors = window.available_monitors()?;
    let monitor = monitors.iter().find(|m| {
        let p = m.position();
        let s = m.size();
        position.x >= p.x as f64
            && position.y >= p.y as f64
            && position.x < p.x as f64 + s.width as f64
            && position.y < p.y as f64 + s.height as f64
    });
    let scale = monitor.map(|m| m.scale_factor()).unwrap_or(1.0);
    let (x, y) = panel_position(
        position.x,
        position.y,
        scale,
        monitor.map(|m| {
            (
                m.position().x as f64,
                m.position().y as f64,
                m.size().width as f64,
                m.size().height as f64,
            )
        }),
    );
    window.set_position(PhysicalPosition::new(x as i32, y as i32))?;
    window.show()?;
    #[cfg(debug_assertions)]
    eprintln!("[quick-audio] shown; requesting focus");
    window.set_focus()
}

fn panel_position(x: f64, y: f64, scale: f64, monitor: Option<(f64, f64, f64, f64)>) -> (f64, f64) {
    let mut left = x - WIDTH * scale / 2.0;
    let mut top = y + 14.0 * scale;
    if let Some((mx, my, mw, mh)) = monitor {
        left = left.clamp(mx, (mx + mw - WIDTH * scale).max(mx));
        top = top.clamp(my, (my + mh - HEIGHT * scale).max(my));
    }
    (left, top)
}

#[tauri::command]
pub fn hide_quick_audio(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(WINDOW) {
        window.hide().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn quick_audio_history(app: AppHandle) -> SlotHistories {
    app.try_state::<Mutex<MeterCache>>()
        .and_then(|cache| {
            cache.lock().ok().map(|cache| SlotHistories {
                primary: cache.slots[0].history.snapshot(),
                secondary: cache.slots[1].history.snapshot(),
            })
        })
        .unwrap_or_else(|| SlotHistories {
            primary: vec![None; SAMPLES],
            secondary: vec![None; SAMPLES],
        })
}

pub fn update(app: &AppHandle, settings: &AppSettings, status: &AppStatus) {
    let Some(cache) = app.try_state::<Mutex<MeterCache>>() else {
        return;
    };
    let Ok(mut cache) = cache.try_lock() else {
        return;
    };
    let now = Instant::now();
    let mut updates = Vec::new();
    for (index, slot) in [MicrophoneSlot::Primary, MicrophoneSlot::Secondary]
        .iter()
        .enumerate()
    {
        let meter = Meter::from_slot(
            settings,
            *slot,
            status.microphone_captures.iter().find(|r| r.slot == *slot),
        );
        if let Some((meter, image)) = cache.slots[index].update(meter, now) {
            updates.push((TRAYS[index], meter, image));
        }
    }
    drop(cache);
    if updates.is_empty() {
        return;
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        for (id, meter, image) in updates {
            if let Some(tray) = handle.tray_by_id(id) {
                let _ = tray.set_icon(Some(image));
                let _ = tray.set_title(Some(meter.title()));
                let _ = tray.set_visible(meter.visible);
                let _ = tray.set_tooltip(Some(meter.tooltip()));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_samples_constant_input_at_a_fixed_cadence_and_softens_spikes() {
        let mut history = History::default();
        let start = Instant::now();
        assert!(history.sample(start, -60));
        assert!(!history.sample(start + Duration::from_millis(100), -10));
        assert!(history.sample(start + INTERVAL, -20));
        assert_eq!(history.values.len(), 2);
        assert!((history.values[1].unwrap() + 46.0).abs() < 0.01);
        for step in 2..=40 {
            assert!(history.sample(start + INTERVAL * step, -60));
        }
        assert_eq!(history.values.len(), SAMPLES);
        assert!(history.snapshot().iter().all(Option::is_some));
    }

    #[test]
    fn history_preserves_missing_time_and_drops_stale_values_after_a_long_gap() {
        let mut history = History::default();
        let start = Instant::now();
        history.sample(start, -60);
        history.sample(start + INTERVAL * 3, -30);
        assert_eq!(
            history.values.iter().copied().collect::<Vec<_>>(),
            vec![Some(-60.0), None, None, Some(-30.0)]
        );
        history.sample(start + Duration::from_secs(20), -40);
        let points = history.snapshot();
        assert_eq!(points.iter().filter(|point| point.is_some()).count(), 1);
        assert_eq!(points.last(), Some(&Some(-40.0)));
    }

    #[test]
    fn meter_handles_invalid_and_stopped_input() {
        assert_eq!(finite_db(f32::NAN, -120), -120);
        assert_eq!(finite_db(f32::INFINITY, -40), -40);
        let meter = Meter {
            active: false,
            level: -20,
            threshold: -57,
            visible: true,
            slot: MicrophoneSlot::Primary,
            configured: true,
            identity: "a".into(),
            label: "USB".into(),
        };
        assert_eq!(meter.title(), "P Off / T-57");
        assert_eq!(meter.image(&History::default()).rgba().len(), 64 * 18 * 4);
    }

    #[test]
    fn meters_use_each_slots_level_and_clear_history_when_device_changes() {
        use crate::settings::CaptureMicrophoneEntry;
        let mut settings = AppSettings::default();
        settings.capture_microphones = vec![
            CaptureMicrophoneEntry {
                id: "usb".into(),
                label: "USB".into(),
                core_audio_uid: None,
            },
            CaptureMicrophoneEntry {
                id: "mac".into(),
                label: "Mac".into(),
                core_audio_uid: None,
            },
        ];
        let runtime = |slot, id: &str, db| MicrophoneCaptureStatus {
            slot,
            device_id: id.into(),
            label: id.into(),
            active: true,
            reconnecting: false,
            warning: None,
            level: crate::audio::LevelSnapshot {
                current_dbfs: db,
                ..Default::default()
            },
        };
        let p = Meter::from_slot(
            &settings,
            MicrophoneSlot::Primary,
            Some(&runtime(MicrophoneSlot::Primary, "usb", -20.0)),
        );
        let secondary = Meter::from_slot(
            &settings,
            MicrophoneSlot::Secondary,
            Some(&runtime(MicrophoneSlot::Secondary, "mac", -60.0)),
        );
        assert_eq!(p.level, -20);
        assert_eq!(secondary.level, -60);
        assert!(secondary.title().starts_with("S T"));
        let mut cache = SlotCache::default();
        let now = Instant::now();
        cache.update(p.clone(), now);
        let mut changed = p;
        changed.identity = "new mic".into();
        changed.level = -80;
        cache.update(changed, now + INTERVAL);
        assert_eq!(
            cache.history.values.iter().copied().collect::<Vec<_>>(),
            vec![Some(-80.0)]
        );
        let off = Meter::from_slot(&settings, MicrophoneSlot::Secondary, None);
        assert!(!off.active);
        assert!(off.title().contains("Off"));
        settings.capture_microphones.truncate(1);
        let missing = Meter::from_slot(&settings, MicrophoneSlot::Secondary, None);
        assert_eq!(missing.title(), "S —");
        assert!(!missing.active);
    }

    #[test]
    fn popover_fits_scaled_monitors_with_negative_origins() {
        let (x, y) = panel_position(-5.0, 12.0, 2.0, Some((-2560.0, 0.0, 2560.0, 1440.0)));
        assert_eq!(x, -840.0);
        assert_eq!(y, 40.0);
        assert_eq!(
            panel_position(1.0, 790.0, 1.0, Some((0.0, 0.0, 1280.0, 800.0))),
            (0.0, 200.0)
        );
    }
}
