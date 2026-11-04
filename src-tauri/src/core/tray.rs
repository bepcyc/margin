use super::{
    model::{Account, UsageWindow, WindowScope},
    notifications::{reset_clock, Clock12h},
    poller,
    ring::{ring_rgba, IconStyle},
    settings::{Settings, TrayMetric, TrayMode},
};
use crate::runtime::{AppState, Runtime};
use chrono::Utc;
use serde::Serialize;
#[cfg(any(target_os = "linux", test))]
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};
#[cfg(not(target_os = "macos"))]
use tauri_plugin_positioner::{Position, WindowExt};

#[cfg(any(target_os = "linux", test))]
#[derive(Default)]
pub(crate) struct BlurGeneration(AtomicU64);

#[cfg(any(target_os = "linux", test))]
impl BlurGeneration {
    pub(crate) fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
    }

    pub(crate) fn current(&self, generation: u64) -> bool {
        self.0.load(Ordering::Relaxed) == generation
    }
}

#[derive(Default)]
struct Presentation {
    revision: u64,
    pixels: Vec<u8>,
    tooltip: String,
    #[cfg(target_os = "macos")]
    title: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RingKey {
    size: u32,
    style: IconStyle,
    stale: bool,
    tenths: Option<i32>,
}

impl RingKey {
    fn new(size: u32, amount: Option<f64>, stale: bool, style: IconStyle) -> Self {
        Self {
            size,
            style,
            stale,
            tenths: amount
                .filter(|amount| amount.is_finite())
                .map(|amount| (amount.clamp(0.0, 100.0) * 10.0).round() as i32),
        }
    }

    fn render(self) -> Vec<u8> {
        ring_rgba(
            self.size,
            self.tenths.map(|tenths| f64::from(tenths) / 10.0),
            self.stale,
            self.style,
        )
    }
}

fn cached_ring(cache: &mut Option<(RingKey, Vec<u8>)>, key: RingKey) -> (Vec<u8>, bool) {
    if let Some((_, pixels)) = cache.as_ref().filter(|(cached, _)| *cached == key) {
        return (pixels.clone(), false);
    }
    let pixels = key.render();
    *cache = Some((key, pixels.clone()));
    (pixels, true)
}

#[derive(Default)]
pub struct Visibility {
    pub last_blur: Mutex<Option<Instant>>,
    #[cfg(target_os = "linux")]
    pub(crate) blur_generation: BlurGeneration,
    presentation: Mutex<Presentation>,
    ring: Mutex<Option<(RingKey, Vec<u8>)>>,
    pub placement: Mutex<PopoverPlacement>,
    content_height: Mutex<Option<f64>>,
    seen: Mutex<Seen>,
}

#[derive(Default)]
struct Seen {
    shown: bool,
    hidden_at: Option<Instant>,
}

impl Seen {
    fn show(&mut self, visible: bool, now: Instant) -> Option<u64> {
        let hidden_for = match (self.shown, visible, self.hidden_at) {
            (false, _, _) => None,
            (true, true, _) => Some(0),
            (true, false, hidden_at) => hidden_at.map(|hidden_at| {
                u64::try_from(now.saturating_duration_since(hidden_at).as_millis())
                    .unwrap_or(u64::MAX)
            }),
        };
        self.shown = true;
        hidden_for
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PopoverShown {
    hidden_for_ms: Option<u64>,
}

pub fn mark_hidden(app: &AppHandle) {
    if let Ok(mut seen) = app.state::<Visibility>().seen.lock() {
        seen.hidden_at = Some(Instant::now());
    }
}

const POPOVER_WIDTH: f64 = 420.0;
const POPOVER_HEIGHT: f64 = 660.0;
const POPOVER_MIN_HEIGHT: f64 = 200.0;

fn popover_height(requested: Option<f64>) -> f64 {
    requested
        .filter(|height| height.is_finite())
        .map_or(POPOVER_HEIGHT, |height| {
            height.ceil().clamp(POPOVER_MIN_HEIGHT, POPOVER_HEIGHT)
        })
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PopoverPlacement {
    pub revision: u64,
    pub arrow_x: Option<f64>,
}

#[derive(Clone, Copy)]
struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

struct Layout {
    frame: Bounds,
    arrow_x: Option<f64>,
}

fn layout(work: Bounds, anchor: Option<Bounds>, scale: f64, macos: bool, height: f64) -> Layout {
    let anchor = anchor.filter(|rect| rect.width > 0.0 && rect.height > 0.0);
    let width = (POPOVER_WIDTH * scale).round().min(work.width);
    let height = (height * scale).round().min(work.height);
    let (x, y) = match anchor {
        Some(tray) => {
            let above = tray.y - height;
            let y = if macos || above < work.y {
                tray.y + tray.height
            } else {
                above
            };
            (tray.x + tray.width / 2.0 - width / 2.0, y)
        }
        None => (
            work.x + (work.width - width) / 2.0,
            work.y + (work.height - height) / 2.0,
        ),
    };
    let x = x.clamp(work.x, work.x + work.width - width).round();
    let y = y.clamp(work.y, work.y + work.height - height).round();
    let arrow_x = anchor.filter(|_| macos).and_then(|tray| {
        let offset = (tray.x + tray.width / 2.0 - x) / scale;
        (offset >= 26.0 && offset <= width / scale - 26.0).then_some(offset)
    });
    Layout {
        frame: Bounds {
            x,
            y,
            width,
            height,
        },
        arrow_x,
    }
}

#[cfg(any(target_os = "macos", test))]
fn cocoa_layout(work: Bounds, anchor: Option<Bounds>, height: f64) -> Layout {
    let flip = |rect: Bounds| Bounds {
        y: -rect.y - rect.height,
        ..rect
    };
    let placed = layout(flip(work), anchor.map(flip), 1.0, true, height);
    Layout {
        frame: flip(placed.frame),
        arrow_x: placed.arrow_x,
    }
}

#[cfg(target_os = "macos")]
fn place_macos_window(app: &AppHandle, window: &tauri::WebviewWindow, height: f64) -> Option<f64> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSScreen, NSWindow};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let mtm = MainThreadMarker::new()?;
    let bounds = |rect: NSRect| Bounds {
        x: rect.origin.x,
        y: rect.origin.y,
        width: rect.size.width,
        height: rect.size.height,
    };
    let pointer = window.ns_window().ok()?;
    // Keep AppKit geometry in points on the main thread, including mixed-DPI moves.
    let native = unsafe { &*pointer.cast::<NSWindow>() };
    let tray_geometry = app.tray_by_id("margin").and_then(|tray| {
        tray.with_inner_tray_icon(move |tray| {
            let mtm = MainThreadMarker::new()?;
            let tray_window = tray.ns_status_item()?.button(mtm)?.window()?;
            let screen = tray_window.screen()?;
            Some((bounds(screen.visibleFrame()), bounds(tray_window.frame())))
        })
        .ok()
        .flatten()
    });
    let (work, anchor) = match tray_geometry {
        Some((work, anchor)) => (work, Some(anchor)),
        None => {
            let screen = native.screen().or_else(|| NSScreen::mainScreen(mtm))?;
            (bounds(screen.visibleFrame()), None)
        }
    };
    let placed = cocoa_layout(work, anchor, height);
    let content_rect = NSRect::new(
        NSPoint::new(placed.frame.x, placed.frame.y),
        NSSize::new(placed.frame.width, placed.frame.height),
    );
    native.setFrame_display(native.frameRectForContentRect(content_rect), true);
    placed.arrow_x
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowKind {
    Session,
    Weekly,
    Other,
}

fn window_kind(window: &UsageWindow) -> WindowKind {
    match window.id.as_str() {
        "five_hour" | "primary_window" => WindowKind::Session,
        "seven_day" | "secondary_window" => WindowKind::Weekly,
        _ => WindowKind::Other,
    }
}

fn expired(window: &UsageWindow, now: chrono::DateTime<Utc>) -> bool {
    window.resets_at.as_ref().is_some_and(|reset| {
        chrono::DateTime::parse_from_rfc3339(reset).is_ok_and(|reset| reset <= now)
    })
}

fn counts(window: &UsageWindow, metric: TrayMetric, now: chrono::DateTime<Utc>) -> bool {
    let kind = window_kind(window);
    match metric {
        TrayMetric::All => true,
        TrayMetric::Core => kind != WindowKind::Other && !expired(window, now),
        TrayMetric::Session => kind == WindowKind::Session && !expired(window, now),
        TrayMetric::Weekly => kind == WindowKind::Weekly && !expired(window, now),
    }
}

pub fn limiting_window(
    account: &Account,
    metric: TrayMetric,
    now: chrono::DateTime<Utc>,
) -> Option<&UsageWindow> {
    let windows = &account.snapshot.as_ref()?.windows;
    let most_used = |metric| {
        windows
            .iter()
            .filter(|window| window.used_percent.is_finite() && counts(window, metric, now))
            .max_by(|left, right| left.used_percent.total_cmp(&right.used_percent))
    };
    most_used(metric).or_else(|| most_used(TrayMetric::All))
}

fn remaining(account: &Account, metric: TrayMetric, now: chrono::DateTime<Utc>) -> Option<f64> {
    limiting_window(account, metric, now).map(|window| 100.0 - window.used_percent)
}

pub fn stale(account: &Account, settings: &Settings, now: chrono::DateTime<Utc>) -> bool {
    if account.error.as_ref().is_some_and(|error| {
        account.snapshot.is_none()
            || !matches!(error.kind.as_str(), "rateLimited" | "credentialsChanged")
    }) {
        return true;
    }
    account.snapshot.as_ref().is_some_and(|snapshot| {
        let old =
            chrono::DateTime::parse_from_rfc3339(&snapshot.fetched_at).map_or(true, |fetched| {
                (now - fetched.with_timezone(&Utc)).num_seconds()
                    > (settings.idle_poll_interval_seconds * 2).max(180) as i64
            });
        // A model or extra window whose reset passed outdates only itself; the
        // card shows it as pending. Core windows speak for the account.
        let has_core = snapshot
            .windows
            .iter()
            .any(|window| window.scope() == WindowScope::Core);
        old || snapshot
            .windows
            .iter()
            .filter(|window| !has_core || window.scope() == WindowScope::Core)
            .any(|window| {
                window.resets_at.as_ref().is_some_and(|reset| {
                    chrono::DateTime::parse_from_rfc3339(reset).map_or(true, |reset| reset <= now)
                })
            })
    })
}

pub fn selected<'a>(
    accounts: &'a [Account],
    settings: &Settings,
    now: chrono::DateTime<Utc>,
) -> Option<&'a Account> {
    if settings.tray_mode == TrayMode::Pinned {
        if let Some(account) = settings
            .pinned_account_id
            .as_ref()
            .and_then(|id| accounts.iter().find(|account| &account.id == id))
        {
            return Some(account);
        }
    }
    accounts
        .iter()
        .filter(|account| !stale(account, settings, now))
        .filter_map(|account| {
            remaining(account, settings.tray_metric, now).map(|value| (account, value))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(account, _)| account)
        .or_else(|| accounts.first())
}

fn icon_style() -> IconStyle {
    if cfg!(target_os = "macos") {
        IconStyle::Template
    } else {
        taskbar_style()
    }
}

#[cfg(windows)]
fn taskbar_style() -> IconStyle {
    let light = windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|key| key.get_u32("SystemUsesLightTheme"))
        .is_ok_and(|value| value == 1);
    if light {
        IconStyle::LightTaskbar
    } else {
        IconStyle::DarkTaskbar
    }
}

#[cfg(not(windows))]
fn taskbar_style() -> IconStyle {
    IconStyle::DarkTaskbar
}

fn icon_size(app: &AppHandle) -> u32 {
    if cfg!(target_os = "macos") {
        return 36;
    }
    let scale = app
        .primary_monitor()
        .ok()
        .flatten()
        .map_or(1.0, |monitor| monitor.scale_factor());
    (16.0 * scale).round().clamp(16.0, 64.0) as u32
}

pub fn percent_label(amount: f64) -> String {
    if amount > 0.0 && amount < 1.0 {
        "<1".into()
    } else {
        format!("{:.0}", amount.floor())
    }
}

#[cfg(target_os = "macos")]
fn set_tabular_title(tray: &tauri::tray::TrayIcon, title: Option<&str>) -> bool {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSFont, NSFontWeightRegular};

    if tray.set_title(title).is_err() {
        return false;
    }
    let _ = tray.with_inner_tray_icon(|tray| {
        let mtm = MainThreadMarker::new()?;
        let button = tray.ns_status_item()?.button(mtm)?;
        let size = NSFont::menuBarFontOfSize(0.0).pointSize();
        let font =
            NSFont::monospacedDigitSystemFontOfSize_weight(size, unsafe { NSFontWeightRegular });
        button.setFont(Some(&font));
        Some(())
    });
    true
}

fn tooltip(
    account: Option<&Account>,
    window: Option<&UsageWindow>,
    is_stale: bool,
    now: chrono::DateTime<Utc>,
    clock: Clock12h,
) -> String {
    let Some(account) = account else {
        return "Margin · Add an account".into();
    };
    let Some(window) = window else {
        return format!("Margin · {} · usage unavailable", account.label);
    };
    let reset = window
        .resets_at
        .as_deref()
        .and_then(|reset| chrono::DateTime::parse_from_rfc3339(reset).ok())
        .map(|reset| reset.with_timezone(&Utc))
        .filter(|reset| *reset > now)
        .map(|reset| format!(" · resets {}", reset_clock(reset, now, clock)))
        .unwrap_or_default();
    format!(
        "Margin · {} · {} {}% remaining{reset}{}",
        account.label,
        window.label.chars().take(64).collect::<String>(),
        percent_label(100.0 - window.used_percent),
        if is_stale { " · last known" } else { "" }
    )
}

pub fn update(app: &AppHandle, state: &AppState) {
    let now = Utc::now();
    let account = selected(&state.accounts, &state.settings, now);
    let window =
        account.and_then(|account| limiting_window(account, state.settings.tray_metric, now));
    let amount = window.map(|window| 100.0 - window.used_percent);
    let is_stale = account.is_some_and(|account| stale(account, &state.settings, now));
    let size = icon_size(app);
    let key = RingKey::new(size, amount, is_stale, icon_style());
    let pixels = match app.state::<Visibility>().ring.lock() {
        Ok(mut cache) => cached_ring(&mut cache, key).0,
        Err(_) => key.render(),
    };
    #[cfg(target_os = "macos")]
    let title = amount
        .filter(|_| state.settings.tray_show_percent)
        .map(|amount| format!("{}%", percent_label(amount)));
    let tooltip = tooltip(account, window, is_stale, now, Clock12h::system());
    let revision = state.revision;
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let visibility = handle.state::<Visibility>();
        let Ok(mut previous) = visibility.presentation.lock() else {
            return;
        };
        if revision < previous.revision {
            return;
        }
        previous.revision = revision;
        #[cfg(target_os = "linux")]
        if handle.state::<crate::gnome_bridge::Bridge>().active() {
            crate::gnome_bridge::update(&handle, &tooltip, size, &pixels, revision);
            previous.pixels = pixels;
            previous.tooltip = tooltip;
            return;
        }
        let Some(tray) = handle.tray_by_id("margin") else {
            return;
        };
        if previous.pixels != pixels {
            let updated = tray.set_icon_with_as_template(
                Some(Image::new_owned(pixels.clone(), size, size)),
                cfg!(target_os = "macos"),
            );
            if updated.is_ok() {
                previous.pixels = pixels;
            }
        }
        if previous.tooltip != tooltip && tray.set_tooltip(Some(&tooltip)).is_ok() {
            previous.tooltip = tooltip;
        }
        #[cfg(target_os = "macos")]
        if previous.title != title && set_tabular_title(&tray, title.as_deref()) {
            previous.title = title;
        }
    });
}

#[cfg(target_os = "macos")]
fn fit_backdrop(window: &tauri::WebviewWindow) {
    use objc2::ClassType;
    use objc2_app_kit::{NSAutoresizingMaskOptions, NSVisualEffectView, NSWindow};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};

    let window = window.clone();
    let handle = window.clone();
    let _ = window.run_on_main_thread(move || {
        let Ok(pointer) = handle.ns_window() else {
            return;
        };
        // Tauri owns this NSWindow; access stays on its main thread while the handle lives.
        let native = unsafe { &*pointer.cast::<NSWindow>() };
        let Some(content) = native.contentView() else {
            return;
        };
        let bounds = content.bounds();
        let frame = NSRect::new(
            NSPoint::new(6.0, if content.isFlipped() { 14.0 } else { 6.0 }),
            NSSize::new(
                (bounds.size.width - 12.0).max(0.0),
                (bounds.size.height - 20.0).max(0.0),
            ),
        );
        for view in content.subviews() {
            if view.isKindOfClass(NSVisualEffectView::class()) {
                view.setAutoresizingMask(
                    NSAutoresizingMaskOptions::ViewWidthSizable
                        | NSAutoresizingMaskOptions::ViewHeightSizable,
                );
                view.setFrame(frame);
            }
        }
        native.invalidateShadow();
    });
}

pub fn show(app: &AppHandle, settings: bool) {
    let handle = app.clone();
    #[cfg(target_os = "linux")]
    tauri::async_runtime::spawn(async move {
        let bridge = handle.state::<crate::gnome_bridge::Bridge>();
        if !bridge.degraded() && !bridge.wait_for_backend().await {
            return;
        }
        let window_app = handle.clone();
        let _ = handle.run_on_main_thread(move || show_on_main_thread(&window_app, settings));
    });
    #[cfg(not(target_os = "linux"))]
    let _ = app.run_on_main_thread(move || show_on_main_thread(&handle, settings));
}

pub fn fit(
    app: &AppHandle,
    height: f64,
    pixel_ratio: f64,
    fit_content: bool,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    let height = {
        let scale = app
            .get_webview_window("main")
            .and_then(|window| window.scale_factor().ok())
            .filter(|scale| scale.is_finite() && *scale > 0.0)
            .unwrap_or(1.0);
        let pixel_ratio = if pixel_ratio.is_finite() && pixel_ratio > 0.0 {
            pixel_ratio
        } else {
            scale
        };
        if fit_content {
            height * pixel_ratio / scale
        } else {
            height
        }
    };
    #[cfg(not(target_os = "linux"))]
    let _ = (pixel_ratio, fit_content);
    let height = popover_height(Some(height));
    {
        let visibility = app.state::<Visibility>();
        let mut current = visibility
            .content_height
            .lock()
            .map_err(|_| "Popover size is unavailable.")?;
        if *current == Some(height) {
            return Ok(());
        }
        *current = Some(height);
    }
    let handle = app.clone();
    app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window("main") {
            if window.is_visible().unwrap_or(false) {
                place(&handle, &window);
            }
        }
    })
    .map_err(|_| "Popover size is unavailable.".into())
}

#[cfg(target_os = "linux")]
fn set_fixed_size(window: &tauri::WebviewWindow, size: tauri::Size) -> tauri::Result<()> {
    let (width, height) = match size {
        tauri::Size::Logical(size) => (
            tauri::LogicalUnit::new(size.width).into(),
            tauri::LogicalUnit::new(size.height).into(),
        ),
        tauri::Size::Physical(size) => (
            tauri::PhysicalUnit::new(size.width as i32).into(),
            tauri::PhysicalUnit::new(size.height as i32).into(),
        ),
    };
    let constraints = tauri::WindowSizeConstraints {
        min_width: Some(width),
        max_width: Some(width),
        min_height: Some(height),
        max_height: Some(height),
    };
    window.set_resizable(true)?;
    window.set_size_constraints(constraints)?;
    window.set_size(size)
}

fn place(app: &AppHandle, window: &tauri::WebviewWindow) {
    #[cfg(target_os = "linux")]
    if app
        .state::<crate::gnome_bridge::Bridge>()
        .retains_position()
    {
        let height = popover_height(
            app.state::<Visibility>()
                .content_height
                .lock()
                .ok()
                .and_then(|value| *value),
        );
        let _ = set_fixed_size(
            window,
            tauri::LogicalSize::new(POPOVER_WIDTH, height).into(),
        );
        if app.state::<crate::gnome_bridge::Bridge>().active() {
            crate::gnome_bridge::reposition(app);
        }
        return;
    }
    let height = popover_height(
        app.state::<Visibility>()
            .content_height
            .lock()
            .ok()
            .and_then(|value| *value),
    );
    #[cfg(target_os = "macos")]
    let arrow_x = place_macos_window(app, window, height);
    #[cfg(not(target_os = "macos"))]
    let arrow_x = {
        let position = Position::TrayCenter;
        if window.move_window_constrained(position).is_err() {
            let _ = window.center();
        }
        let anchor = app
            .tray_by_id("margin")
            .and_then(|tray| tray.rect().ok().flatten())
            .map(|rect| {
                (
                    rect.position.to_physical::<f64>(1.0),
                    rect.size.to_physical::<f64>(1.0),
                )
            })
            .filter(|(_, size)| size.width > 0.0 && size.height > 0.0);
        let monitor = anchor
            .and_then(|(position, _)| {
                window
                    .monitor_from_point(position.x, position.y)
                    .ok()
                    .flatten()
            })
            .or_else(|| window.current_monitor().ok().flatten());
        let mut arrow_x = None;
        if let Some(monitor) = monitor {
            let work = monitor.work_area();
            let placed = layout(
                Bounds {
                    x: f64::from(work.position.x),
                    y: f64::from(work.position.y),
                    width: f64::from(work.size.width),
                    height: f64::from(work.size.height),
                },
                anchor.map(|(position, size)| Bounds {
                    x: position.x,
                    y: position.y,
                    width: size.width,
                    height: size.height,
                }),
                monitor.scale_factor(),
                cfg!(target_os = "macos"),
                height,
            );
            let size =
                tauri::PhysicalSize::new(placed.frame.width as u32, placed.frame.height as u32);
            #[cfg(target_os = "linux")]
            let resized = set_fixed_size(window, size.into());
            #[cfg(not(target_os = "linux"))]
            let resized = window.set_size(size);
            let positioned = window.set_position(tauri::PhysicalPosition::new(
                placed.frame.x as i32,
                placed.frame.y as i32,
            ));
            if resized.is_ok() && positioned.is_ok() {
                arrow_x = placed.arrow_x;
            }
        }
        arrow_x
    };
    if let Ok(mut placement) = app.state::<Visibility>().placement.lock() {
        placement.revision = placement.revision.saturating_add(1);
        placement.arrow_x = arrow_x;
        let _ = window.emit("popover_placement_changed", placement.clone());
    }
    #[cfg(target_os = "macos")]
    fit_backdrop(window);
}

fn show_on_main_thread(app: &AppHandle, settings: bool) {
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "linux")]
        app.state::<Visibility>().blur_generation.next();
        let visible = window.is_visible().unwrap_or(false);
        place(app, &window);
        let _ = window.show();
        let _ = window.set_focus();
        #[cfg(target_os = "linux")]
        if app.state::<crate::gnome_bridge::Bridge>().active() {
            crate::gnome_bridge::reposition(app);
        }
        if settings {
            let _ = app.emit("open_settings", ());
        }
        let hidden_for_ms = app
            .state::<Visibility>()
            .seen
            .lock()
            .ok()
            .and_then(|mut seen| seen.show(visible, Instant::now()));
        let _ = app.emit_to("main", "popover_shown", PopoverShown { hidden_for_ms });
        let runtime = app.state::<Arc<Runtime>>().inner().clone();
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let view = runtime.inner.lock().await.view.clone();
            crate::runtime::publish(&handle, &view);
            poller::refresh_on_open(&runtime).await;
        });
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn show_from_bridge(app: &AppHandle, settings: bool, generation: u64) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if handle
            .state::<crate::gnome_bridge::Bridge>()
            .is_active_generation(generation)
        {
            show_on_main_thread(&handle, settings);
        }
    });
}

#[cfg(target_os = "linux")]
pub(crate) fn toggle_from_bridge(app: &AppHandle, generation: u64) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if !handle
            .state::<crate::gnome_bridge::Bridge>()
            .is_active_generation(generation)
        {
            return;
        }
        if let Some(window) = handle.get_webview_window("main") {
            if window.is_visible().unwrap_or(false) {
                if window.hide().is_ok() {
                    mark_hidden(&handle);
                }
            } else {
                show_on_main_thread(&handle, false);
            }
        }
    });
}

#[cfg(target_os = "linux")]
pub(crate) fn initial_bridge_icon(app: &AppHandle) -> (u32, Vec<u8>, String) {
    let size = icon_size(app);
    (
        size,
        ring_rgba(size, None, false, icon_style()),
        "Margin · Checking accounts".into(),
    )
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    #[cfg(target_os = "linux")]
    {
        crate::gnome_bridge::start(app);
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    setup_native(app)
}

pub(crate) fn setup_native(app: &AppHandle) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, "show", "Open Margin", true, None::<&str>)?;
    let refresh_item = MenuItem::with_id(app, "refresh", "Refresh usage", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Margin", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &show_item,
            &refresh_item,
            &settings_item,
            &separator,
            &quit_item,
        ],
    )?;
    TrayIconBuilder::with_id("margin")
        .icon({
            let size = icon_size(app);
            Image::new_owned(ring_rgba(size, None, false, icon_style()), size, size)
        })
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Margin · Checking accounts")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show(app, false),
            "settings" => show(app, true),
            "refresh" => {
                let runtime = app.state::<Arc<Runtime>>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    poller::refresh(&runtime).await;
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            let app = tray.app_handle();
            tauri_plugin_positioner::on_tray_event(app, &event);
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let blurred = app
                    .state::<Visibility>()
                    .last_blur
                    .lock()
                    .ok()
                    .and_then(|value| *value)
                    .is_some_and(|time| time.elapsed().as_millis() < 250);
                if let Some(window) = app.get_webview_window("main") {
                    if window.is_visible().unwrap_or(false) || blurred {
                        if window.hide().is_ok() {
                            mark_hidden(app);
                        }
                    } else {
                        show(app, false);
                    }
                }
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        model::{AccountSource, Identity, Provider, Snapshot, UsageError, UsageWindow},
        settings::Settings,
    };

    #[test]
    fn blur_generation_cancels_older_checks() {
        let generation = BlurGeneration::default();
        let first_blur = generation.next();
        assert!(generation.current(first_blur));
        generation.next();
        assert!(!generation.current(first_blur));
        let second_blur = generation.next();
        assert!(generation.current(second_blur));
        assert!(!generation.current(first_blur));
    }

    fn account(id: &str, usage: &[f64]) -> Account {
        Account {
            id: id.into(),
            provider: Provider::Claude,
            label: id.into(),
            color: "#ffffff".into(),
            source: AccountSource::Keychain,
            identity: Identity::default(),
            snapshot: Some(Snapshot {
                windows: usage
                    .iter()
                    .enumerate()
                    .map(|(index, used)| UsageWindow {
                        id: format!("window_{index}"),
                        label: "Session".into(),
                        used_percent: *used,
                        resets_at: None,
                        window_seconds: None,
                    })
                    .collect(),
                fetched_at: Utc::now().to_rfc3339(),
                extra_usage: None,
                reset_grants: None,
                balance: None,
            }),
            error: None,
            refreshing: false,
        }
    }

    #[test]
    fn lowest_remaining_uses_all_windows_and_excludes_stale_accounts() {
        let mut state = AppState {
            revision: 0,
            accounts: vec![account("a", &[10.0, 98.0]), account("b", &[90.0])],
            settings: Settings {
                tray_mode: TrayMode::Lowest,
                ..Settings::default()
            },
            refreshing: false,
            notice: None,
            bridge_notice: None,
            recovery: false,
            recovered: Vec::new(),
            ignored_sources: Vec::new(),
            switching: Default::default(),
        };
        assert_eq!(
            selected(&state.accounts, &state.settings, Utc::now())
                .unwrap()
                .id,
            "a"
        );
        state.accounts[0].error = Some(UsageError::new("network", "Offline"));
        assert_eq!(
            selected(&state.accounts, &state.settings, Utc::now())
                .unwrap()
                .id,
            "b"
        );
        state.settings.tray_mode = TrayMode::Pinned;
        state.settings.pinned_account_id = Some("a".into());
        let pinned = selected(&state.accounts, &state.settings, Utc::now()).unwrap();
        assert_eq!(pinned.id, "a");
        assert!(stale(pinned, &state.settings, Utc::now()));
    }

    #[test]
    fn transient_errors_keep_fresh_readings_live() {
        let mut state = AppState {
            revision: 0,
            accounts: vec![account("a", &[10.0])],
            settings: Settings::default(),
            refreshing: false,
            notice: None,
            bridge_notice: None,
            recovery: false,
            recovered: Vec::new(),
            ignored_sources: Vec::new(),
            switching: Default::default(),
        };
        for kind in ["rateLimited", "credentialsChanged"] {
            state.accounts[0].error = Some(UsageError::new(kind, "Retrying"));
            assert!(!stale(&state.accounts[0], &state.settings, Utc::now()));
            if let Some(snapshot) = state.accounts[0].snapshot.as_mut() {
                snapshot.fetched_at = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
            }
            assert!(stale(&state.accounts[0], &state.settings, Utc::now()));
            state.accounts[0].snapshot = None;
            assert!(stale(&state.accounts[0], &state.settings, Utc::now()));
            state.accounts[0] = account("a", &[10.0]);
        }
    }

    fn windowed(id: &str, windows: &[(&str, f64, Option<i64>)]) -> Account {
        let mut account = account(id, &[]);
        if let Some(snapshot) = account.snapshot.as_mut() {
            snapshot.windows = windows
                .iter()
                .map(|(id, used, reset_hours)| UsageWindow {
                    id: (*id).into(),
                    label: (*id).into(),
                    used_percent: *used,
                    resets_at: reset_hours
                        .map(|hours| (Utc::now() + chrono::Duration::hours(hours)).to_rfc3339()),
                    window_seconds: None,
                })
                .collect();
        }
        account
    }

    #[test]
    fn only_core_resets_make_an_account_stale() {
        let settings = Settings::default();
        let now = Utc::now();
        let model_elapsed = windowed(
            "a",
            &[
                ("five_hour", 10.0, Some(2)),
                ("seven_day", 20.0, Some(48)),
                ("seven_day_opus", 0.0, Some(-1)),
            ],
        );
        assert!(!stale(&model_elapsed, &settings, now));
        let session_elapsed = windowed(
            "a",
            &[("five_hour", 10.0, Some(-1)), ("seven_day", 20.0, Some(48))],
        );
        assert!(stale(&session_elapsed, &settings, now));
        let model_only = windowed("a", &[("seven_day_opus", 10.0, Some(-1))]);
        assert!(stale(&model_only, &settings, now));
    }

    fn limiting_id(account: &Account, metric: TrayMetric) -> Option<&str> {
        limiting_window(account, metric, Utc::now()).map(|window| window.id.as_str())
    }

    #[test]
    fn window_kinds_match_provider_ids() {
        for (id, kind) in [
            ("five_hour", WindowKind::Session),
            ("primary_window", WindowKind::Session),
            ("seven_day", WindowKind::Weekly),
            ("secondary_window", WindowKind::Weekly),
            ("seven_day_opus", WindowKind::Other),
            ("seven_day_sonnet", WindowKind::Other),
            ("seven_day_routines", WindowKind::Other),
            ("seven_day_cowork", WindowKind::Other),
            ("weekly_scoped_0123456789abcdef", WindowKind::Other),
            ("extra_0123456789abcdef_primary_window", WindowKind::Other),
            ("extra_0123456789abcdef_secondary_window", WindowKind::Other),
        ] {
            let account = windowed("a", &[(id, 1.0, None)]);
            let window = &account.snapshot.as_ref().unwrap().windows[0];
            assert_eq!(window_kind(window), kind, "{id}");
        }
    }

    #[test]
    fn core_metric_ignores_model_scoped_and_expired_windows() {
        let claude = windowed(
            "a",
            &[
                ("five_hour", 40.0, Some(-1)),
                ("seven_day", 30.0, Some(48)),
                ("seven_day_opus", 99.0, Some(48)),
                ("weekly_scoped_0123456789abcdef", 97.0, Some(48)),
                ("seven_day_cowork", 95.0, Some(48)),
            ],
        );
        assert_eq!(limiting_id(&claude, TrayMetric::Core), Some("seven_day"));
        assert_eq!(
            limiting_id(&claude, TrayMetric::All),
            Some("seven_day_opus")
        );
        assert_eq!(limiting_id(&claude, TrayMetric::Weekly), Some("seven_day"));
        assert_eq!(
            limiting_id(&claude, TrayMetric::Session),
            Some("seven_day_opus")
        );

        let codex = windowed(
            "b",
            &[
                ("primary_window", 70.0, Some(2)),
                ("secondary_window", 20.0, Some(100)),
                ("extra_0123456789abcdef_primary_window", 90.0, Some(2)),
            ],
        );
        assert_eq!(
            limiting_id(&codex, TrayMetric::Core),
            Some("primary_window")
        );
        assert_eq!(
            limiting_id(&codex, TrayMetric::Session),
            Some("primary_window")
        );
        assert_eq!(
            limiting_id(&codex, TrayMetric::Weekly),
            Some("secondary_window")
        );
        assert_eq!(
            limiting_id(&codex, TrayMetric::All),
            Some("extra_0123456789abcdef_primary_window")
        );
    }

    #[test]
    fn core_metric_falls_back_to_any_window_without_core_windows() {
        let account = windowed(
            "a",
            &[
                ("seven_day_sonnet", 60.0, None),
                ("seven_day_opus", 80.0, None),
            ],
        );
        assert_eq!(
            limiting_id(&account, TrayMetric::Core),
            Some("seven_day_opus")
        );
        assert_eq!(limiting_id(&windowed("b", &[]), TrayMetric::Core), None);
    }

    #[test]
    fn lowest_account_uses_the_selected_metric() {
        let mut state = AppState {
            revision: 0,
            accounts: vec![
                windowed(
                    "a",
                    &[
                        ("five_hour", 20.0, Some(2)),
                        ("seven_day_opus", 99.0, Some(48)),
                    ],
                ),
                windowed(
                    "b",
                    &[("five_hour", 60.0, Some(2)), ("seven_day", 10.0, Some(48))],
                ),
            ],
            settings: Settings {
                tray_mode: TrayMode::Lowest,
                ..Settings::default()
            },
            refreshing: false,
            notice: None,
            bridge_notice: None,
            recovery: false,
            recovered: Vec::new(),
            ignored_sources: Vec::new(),
            switching: Default::default(),
        };
        assert_eq!(
            selected(&state.accounts, &state.settings, Utc::now())
                .unwrap()
                .id,
            "b"
        );
        state.settings.tray_metric = TrayMetric::All;
        assert_eq!(
            selected(&state.accounts, &state.settings, Utc::now())
                .unwrap()
                .id,
            "a"
        );
        state.settings.tray_metric = TrayMetric::Weekly;
        assert_eq!(
            selected(&state.accounts, &state.settings, Utc::now())
                .unwrap()
                .id,
            "a"
        );
    }

    #[test]
    fn tooltip_names_the_followed_window_and_its_reset_time() {
        let now = Utc::now();
        let mut work = windowed("Personal", &[("seven_day", 58.0, Some(30))]);
        let reset = now + chrono::Duration::hours(30);
        let window = work.snapshot.as_ref().unwrap().windows[0].clone();
        assert_eq!(
            tooltip(Some(&work), Some(&window), false, now, Clock12h(false)),
            format!(
                "Margin · Personal · seven_day 42% remaining · resets {}",
                reset_clock(reset, now, Clock12h(false))
            )
        );
        let mut expired = window.clone();
        expired.resets_at = Some((now - chrono::Duration::minutes(1)).to_rfc3339());
        assert_eq!(
            tooltip(Some(&work), Some(&expired), true, now, Clock12h(false)),
            "Margin · Personal · seven_day 42% remaining · last known"
        );
        work.snapshot = None;
        assert_eq!(
            tooltip(Some(&work), None, false, now, Clock12h(false)),
            "Margin · Personal · usage unavailable"
        );
        assert_eq!(
            tooltip(None, None, false, now, Clock12h(false)),
            "Margin · Add an account"
        );
    }

    #[test]
    fn popover_shown_reports_time_since_last_hide() {
        let start = Instant::now();
        let mut seen = Seen::default();
        assert_eq!(seen.show(false, start), None);
        assert_eq!(seen.show(true, start), Some(0));
        seen.hidden_at = Some(start);
        assert_eq!(
            seen.show(false, start + std::time::Duration::from_millis(1500)),
            Some(1500)
        );
        assert_eq!(seen.show(true, start), Some(0));
    }

    #[test]
    fn identical_ring_inputs_reuse_the_last_render() {
        let mut cache = None;
        let key = RingKey::new(36, Some(42.31), false, IconStyle::Template);
        let (first, rendered) = cached_ring(&mut cache, key);
        assert!(rendered);
        let (again, rendered) = cached_ring(
            &mut cache,
            RingKey::new(36, Some(42.34), false, IconStyle::Template),
        );
        assert!(!rendered);
        assert_eq!(first, again);
        for changed in [
            RingKey::new(32, Some(42.3), false, IconStyle::Template),
            RingKey::new(32, Some(42.3), true, IconStyle::Template),
            RingKey::new(32, Some(42.3), true, IconStyle::DarkTaskbar),
            RingKey::new(32, Some(12.0), true, IconStyle::DarkTaskbar),
            RingKey::new(32, None, true, IconStyle::DarkTaskbar),
        ] {
            assert!(cached_ring(&mut cache, changed).1);
        }
        assert_eq!(
            key.render(),
            ring_rgba(36, Some(42.3), false, IconStyle::Template)
        );
    }

    #[test]
    fn cocoa_coordinates_keep_content_size_across_retina_and_external_screens() {
        for (work, tray, expected_x, expected_y) in [
            (
                Bounds {
                    x: 0.0,
                    y: 0.0,
                    width: 1440.0,
                    height: 875.0,
                },
                Bounds {
                    x: 1368.0,
                    y: 875.0,
                    width: 24.0,
                    height: 25.0,
                },
                1020.0,
                215.0,
            ),
            (
                Bounds {
                    x: 1440.0,
                    y: -180.0,
                    width: 1920.0,
                    height: 1055.0,
                },
                Bounds {
                    x: 3288.0,
                    y: 875.0,
                    width: 24.0,
                    height: 25.0,
                },
                2940.0,
                215.0,
            ),
            (
                Bounds {
                    x: -1920.0,
                    y: -180.0,
                    width: 1920.0,
                    height: 1055.0,
                },
                Bounds {
                    x: -1872.0,
                    y: 875.0,
                    width: 24.0,
                    height: 25.0,
                },
                -1920.0,
                215.0,
            ),
            (
                Bounds {
                    x: 0.0,
                    y: 900.0,
                    width: 1920.0,
                    height: 1055.0,
                },
                Bounds {
                    x: 1848.0,
                    y: 1955.0,
                    width: 24.0,
                    height: 25.0,
                },
                1500.0,
                1295.0,
            ),
        ] {
            let placed = cocoa_layout(work, Some(tray), POPOVER_HEIGHT);
            assert_eq!(placed.frame.width, 420.0);
            assert_eq!(placed.frame.height, 660.0);
            assert_eq!(placed.frame.x, expected_x);
            assert_eq!(placed.frame.y, expected_y);
            assert_eq!(placed.frame.y + placed.frame.height, tray.y);
            assert_eq!(
                placed.frame.x + placed.arrow_x.unwrap(),
                tray.x + tray.width / 2.0
            );
        }
    }

    #[test]
    fn cocoa_small_work_area_and_missing_tray_stay_within_screen() {
        let work = Bounds {
            x: -360.0,
            y: 900.0,
            width: 360.0,
            height: 440.0,
        };
        let placed = cocoa_layout(work, None, POPOVER_HEIGHT);
        assert_eq!(placed.frame.x, work.x);
        assert_eq!(placed.frame.y, work.y);
        assert_eq!(placed.frame.width, work.width);
        assert_eq!(placed.frame.height, work.height);
        assert_eq!(placed.arrow_x, None);
    }

    #[test]
    fn retina_arrow_tracks_tray_after_right_edge_clamping() {
        let work = Bounds {
            x: 0.0,
            y: 50.0,
            width: 2880.0,
            height: 1750.0,
        };
        let tray = Bounds {
            x: 2736.0,
            y: 0.0,
            width: 48.0,
            height: 50.0,
        };
        let placed = layout(work, Some(tray), 2.0, true, POPOVER_HEIGHT);
        assert_eq!(placed.frame.x, 2040.0);
        assert_eq!(placed.frame.y, 50.0);
        assert_eq!(placed.arrow_x, Some(360.0));
        assert_eq!(placed.frame.x + placed.arrow_x.unwrap() * 2.0, 2760.0);
    }

    #[test]
    fn left_monitor_coordinates_keep_arrow_aligned() {
        let work = Bounds {
            x: -1920.0,
            y: 25.0,
            width: 1920.0,
            height: 1055.0,
        };
        let tray = Bounds {
            x: -1872.0,
            y: 0.0,
            width: 24.0,
            height: 25.0,
        };
        let placed = layout(work, Some(tray), 1.0, true, POPOVER_HEIGHT);
        assert_eq!(placed.frame.x, -1920.0);
        assert_eq!(placed.arrow_x, Some(60.0));
    }

    #[test]
    fn unavailable_or_outside_anchor_has_no_misleading_arrow() {
        let work = Bounds {
            x: 0.0,
            y: 25.0,
            width: 1280.0,
            height: 775.0,
        };
        let centered = layout(work, None, 1.0, true, POPOVER_HEIGHT);
        assert_eq!(centered.frame.x, 430.0);
        assert_eq!(centered.arrow_x, None);
        let hidden = Bounds {
            x: 0.0,
            y: 800.0,
            width: 34.0,
            height: 0.0,
        };
        let fallback = layout(work, Some(hidden), 1.0, true, POPOVER_HEIGHT);
        assert_eq!(fallback.frame.x, centered.frame.x);
        assert_eq!(fallback.frame.y, centered.frame.y);
        assert_eq!(fallback.arrow_x, None);
        let edge = Bounds {
            x: 1260.0,
            y: 0.0,
            width: 20.0,
            height: 25.0,
        };
        assert_eq!(
            layout(work, Some(edge), 1.0, true, POPOVER_HEIGHT).arrow_x,
            None
        );
    }

    #[test]
    fn popover_height_follows_content_within_bounds() {
        assert_eq!(popover_height(None), POPOVER_HEIGHT);
        assert_eq!(popover_height(Some(f64::NAN)), POPOVER_HEIGHT);
        assert_eq!(popover_height(Some(311.2)), 312.0);
        assert_eq!(popover_height(Some(40.0)), POPOVER_MIN_HEIGHT);
        assert_eq!(popover_height(Some(2000.0)), POPOVER_HEIGHT);
    }

    #[test]
    fn short_popover_stays_attached_to_the_tray() {
        let work = Bounds {
            x: 0.0,
            y: 0.0,
            width: 1440.0,
            height: 875.0,
        };
        let tray = Bounds {
            x: 1368.0,
            y: 875.0,
            width: 24.0,
            height: 25.0,
        };
        let placed = cocoa_layout(work, Some(tray), 300.0);
        assert_eq!(placed.frame.height, 300.0);
        assert_eq!(placed.frame.y + placed.frame.height, tray.y);
        let work = Bounds {
            x: 0.0,
            y: 0.0,
            width: 1920.0 * 1.5,
            height: 1040.0 * 1.5,
        };
        let tray = Bounds {
            x: 1800.0 * 1.5,
            y: 1040.0 * 1.5,
            width: 24.0 * 1.5,
            height: 40.0 * 1.5,
        };
        let placed = layout(work, Some(tray), 1.5, false, 300.0);
        assert_eq!(placed.frame.height, 450.0);
        assert_eq!(placed.frame.y + placed.frame.height, tray.y);
    }

    #[test]
    fn windows_taskbar_placement_respects_each_scale() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let work = Bounds {
                x: -1920.0 * scale,
                y: 0.0,
                width: 1920.0 * scale,
                height: 1040.0 * scale,
            };
            let tray = Bounds {
                x: -80.0 * scale,
                y: 1040.0 * scale,
                width: 24.0 * scale,
                height: 40.0 * scale,
            };
            let placed = layout(work, Some(tray), scale, false, POPOVER_HEIGHT);
            assert_eq!(placed.frame.width, 420.0 * scale);
            assert_eq!(placed.frame.height, 660.0 * scale);
            assert_eq!(placed.frame.x + placed.frame.width, 0.0);
            assert_eq!(placed.frame.y + placed.frame.height, tray.y);
            assert_eq!(placed.arrow_x, None);
        }
    }

    #[test]
    fn top_taskbar_places_window_below_and_small_displays_fit() {
        let work = Bounds {
            x: 0.0,
            y: 40.0,
            width: 360.0,
            height: 440.0,
        };
        let tray = Bounds {
            x: 300.0,
            y: 0.0,
            width: 24.0,
            height: 40.0,
        };
        let placed = layout(work, Some(tray), 1.5, false, POPOVER_HEIGHT);
        assert_eq!(placed.frame.x, 0.0);
        assert_eq!(placed.frame.y, 40.0);
        assert_eq!(placed.frame.width, 360.0);
        assert_eq!(placed.frame.height, 440.0);
    }
}
