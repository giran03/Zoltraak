//! The tray icon's right-click menu.
//!
//! The native Win32 popup cannot be themed, so the menu is a small frameless
//! window that renders the app's own page in "tray menu" mode (see
//! `src/TrayMenu.tsx`) and therefore follows every appearance setting. It is
//! anchored to the tray icon the way the Steam or Discord menus are: on the
//! screen edge the taskbar sits on, with one corner at the icon and never
//! covering it. It closes as soon as it loses focus.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Rect, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

pub const LABEL: &str = "tray-menu";

/// Logical size; the page lays itself out to fit exactly this.
const WIDTH: f64 = 232.0;
const HEIGHT: f64 = 150.0;
/// Logical gap between the menu and the taskbar edge / icon.
const GAP: f64 = 8.0;
/// A right-click this soon after the menu closed is the click that closed it.
const REOPEN_GUARD_MS: u64 = 300;

static CLOSED_AT_MS: AtomicU64 = AtomicU64::new(0);
static BUILDING: AtomicBool = AtomicBool::new(false);
static PENDING: Mutex<Option<Area>> = Mutex::new(None);

fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    // +1 so a close at process start is distinguishable from "never closed".
    START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// A rectangle in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Area {
    fn right(&self) -> f64 {
        self.x + self.w
    }
    fn bottom(&self) -> f64 {
        self.y + self.h
    }
    fn cx(&self) -> f64 {
        self.x + self.w / 2.0
    }
    fn cy(&self) -> f64 {
        self.y + self.h / 2.0
    }
}

fn clamp(value: f64, low: f64, high: f64) -> f64 {
    if high < low { low } else { value.max(low).min(high) }
}

/// Top-left corner for a menu of `size`, opened from `icon`.
///
/// `work` is the monitor area the taskbar leaves free. The menu sits against
/// that edge and its near corner lines up with the icon, extending toward
/// whichever side has room, so it can never land on top of the icon.
pub fn place(icon: Area, work: Area, size: (f64, f64), gap: f64) -> (f64, f64) {
    let (w, h) = size;
    let (x, y);
    if icon.cy() >= work.bottom() {
        // Taskbar along the bottom.
        y = work.bottom().min(icon.y) - h - gap;
        x = horizontal(icon, work, w);
    } else if icon.cy() <= work.y {
        // Taskbar along the top.
        y = work.y.max(icon.bottom()) + gap;
        x = horizontal(icon, work, w);
    } else if icon.cx() >= work.right() {
        // Taskbar on the right.
        x = work.right().min(icon.x) - w - gap;
        y = vertical(icon, work, h);
    } else if icon.cx() <= work.x {
        // Taskbar on the left.
        x = work.x.max(icon.right()) + gap;
        y = vertical(icon, work, h);
    } else {
        // Auto-hidden taskbar or an icon inside a flyout: open above it.
        y = icon.y - h - gap;
        x = horizontal(icon, work, w);
    }
    (
        clamp(x, work.x + gap, work.right() - w - gap),
        clamp(y, work.y + gap, work.bottom() - h - gap),
    )
}

fn horizontal(icon: Area, work: Area, w: f64) -> f64 {
    if icon.cx() + w <= work.right() { icon.cx() } else { icon.cx() - w }
}

fn vertical(icon: Area, work: Area, h: f64) -> f64 {
    if icon.cy() + h <= work.bottom() { icon.cy() } else { icon.cy() - h }
}

fn physical_area(rect: &Rect) -> Area {
    let position = rect.position.to_physical::<f64>(1.0);
    let size = rect.size.to_physical::<f64>(1.0);
    Area { x: position.x, y: position.y, w: size.width, h: size.height }
}

/// Entry point for a right-click on the tray icon. `cursor` stands in for
/// icons Windows reports no usable rect for (e.g. in the overflow flyout).
pub fn toggle(app: &AppHandle, rect: &Rect, cursor: PhysicalPosition<f64>) {
    if let Some(window) = app.get_webview_window(LABEL) {
        if window.is_visible().unwrap_or(false) {
            dismiss(&window);
            return;
        }
    }
    let closed = CLOSED_AT_MS.load(Ordering::Relaxed);
    if closed != 0 && now_ms().saturating_sub(closed) < REOPEN_GUARD_MS {
        return;
    }

    let mut icon = physical_area(rect);
    if icon.w < 1.0 || icon.h < 1.0 {
        icon = Area { x: cursor.x, y: cursor.y, w: 1.0, h: 1.0 };
    }

    // Building a window from the event-loop thread deadlocks on Windows.
    let app = app.clone();
    std::thread::spawn(move || open(&app, icon));
}

fn open(app: &AppHandle, icon: Area) {
    if let Some(window) = app.get_webview_window(LABEL) {
        present(&window, icon);
        return;
    }
    if BUILDING.swap(true, Ordering::SeqCst) {
        return;
    }
    // Shown by `ready` once the page has painted, so it never flashes blank.
    if let Ok(mut pending) = PENDING.lock() {
        *pending = Some(icon);
    }
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
        .inner_size(WIDTH, HEIGHT)
        .decorations(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(true)
        .visible(false)
        .focused(false)
        .build();
    BUILDING.store(false, Ordering::SeqCst);
    if let Err(error) = built {
        tracing::warn!(%error, "could not create the tray menu window");
        if let Ok(mut pending) = PENDING.lock() {
            *pending = None;
        }
    }
}

/// Called by the page once it has mounted: shows a menu requested while the
/// window was still being created.
pub fn ready(app: &AppHandle) {
    let icon = PENDING.lock().ok().and_then(|mut pending| pending.take());
    if let (Some(icon), Some(window)) = (icon, app.get_webview_window(LABEL)) {
        present(&window, icon);
    }
}

fn present(window: &WebviewWindow, icon: Area) {
    let app = window.app_handle();
    let monitor = app
        .monitor_from_point(icon.cx(), icon.cy())
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };

    let scale = monitor.scale_factor();
    let work = monitor.work_area();
    let work = Area {
        x: work.position.x as f64,
        y: work.position.y as f64,
        w: work.size.width as f64,
        h: work.size.height as f64,
    };
    let size = (WIDTH * scale, HEIGHT * scale);
    let (x, y) = place(icon, work, size, GAP * scale);

    let _ = window.set_size(PhysicalSize::new(size.0.round() as u32, size.1.round() as u32));
    let _ = window.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
    crate::webview_power::shown(window);
    let _ = window.show();
    let _ = window.set_focus();
    let _ = window.emit_to(LABEL, "tray-menu-shown", ());
}

/// Hide the menu (focus lost, an item chosen, or Escape).
pub fn dismiss(window: &WebviewWindow) {
    if !window.is_visible().unwrap_or(false) {
        return;
    }
    CLOSED_AT_MS.store(now_ms(), Ordering::Relaxed);
    let _ = window.hide();
    crate::webview_power::hidden(window);
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Area = Area { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 };
    const SIZE: (f64, f64) = (232.0, 150.0);

    fn icon_at(x: f64, y: f64) -> Area {
        Area { x, y, w: 24.0, h: 24.0 }
    }

    #[test]
    fn bottom_taskbar_menu_sits_above_the_icon_with_its_right_corner_at_it() {
        let icon = icon_at(1800.0, 1050.0);
        let (x, y) = place(icon, WORK, SIZE, 8.0);
        assert_eq!(y, 1040.0 - 150.0 - 8.0);
        assert_eq!(x + SIZE.0, icon.cx());
    }

    #[test]
    fn bottom_taskbar_menu_extends_right_when_there_is_room() {
        let icon = icon_at(300.0, 1050.0);
        let (x, _) = place(icon, WORK, SIZE, 8.0);
        assert_eq!(x, icon.cx());
    }

    #[test]
    fn menu_never_overlaps_the_icon() {
        for (ix, iy) in [(1800.0, 1050.0), (10.0, 1050.0), (1000.0, 2.0), (1900.0, 500.0)] {
            let icon = icon_at(ix, iy);
            let work = if iy < 10.0 {
                Area { y: 40.0, h: 1000.0, ..WORK }
            } else if ix > 1890.0 {
                Area { w: 1880.0, ..WORK }
            } else {
                WORK
            };
            let (x, y) = place(icon, work, SIZE, 8.0);
            let overlaps = x < icon.right()
                && x + SIZE.0 > icon.x
                && y < icon.bottom()
                && y + SIZE.1 > icon.y;
            assert!(!overlaps, "menu at {x},{y} covers icon at {ix},{iy}");
        }
    }

    #[test]
    fn top_taskbar_menu_opens_below() {
        let work = Area { y: 40.0, h: 1000.0, ..WORK };
        let (_, y) = place(icon_at(1800.0, 8.0), work, SIZE, 8.0);
        assert_eq!(y, 40.0 + 8.0);
    }

    #[test]
    fn right_taskbar_menu_opens_to_the_left() {
        let work = Area { w: 1880.0, ..WORK };
        let (x, _) = place(icon_at(1890.0, 500.0), work, SIZE, 8.0);
        assert_eq!(x, 1880.0 - SIZE.0 - 8.0);
    }

    #[test]
    fn menu_stays_inside_the_work_area() {
        let (x, y) = place(icon_at(1915.0, 1050.0), WORK, SIZE, 8.0);
        assert!(x >= 8.0 && x + SIZE.0 <= 1920.0 - 8.0);
        assert!(y >= 8.0 && y + SIZE.1 <= 1040.0 - 8.0);
    }
}
