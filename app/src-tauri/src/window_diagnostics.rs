//! Opt-in native-message timing, never installed during ordinary launches.
#[cfg(windows)]
pub fn install(window: &tauri::WebviewWindow) {
    if std::env::var("ZOLTRAAK_TRACE_WINDOW").as_deref() != Ok("1") {
        return;
    }
    use std::cell::RefCell;
    use std::time::Instant;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    };
    const ID: usize = 0x5a4f4c54;
    #[derive(Default)]
    struct Timing {
        calls: u64,
        slow: u64,
        max_ms: f64,
        total_ms: f64,
        entry_started: Option<Instant>,
        entry_ms: f64,
        first_motion_ms: Option<f64>,
    }
    thread_local! { static TIMING: RefCell<Timing> = RefCell::new(Timing::default()); }
    unsafe extern "system" fn procedure(
        hwnd: HWND,
        msg: u32,
        w: WPARAM,
        l: LPARAM,
        _: usize,
        _: usize,
    ) -> LRESULT {
        if msg == WM_ENTERSIZEMOVE {
            TIMING.with(|state| *state.borrow_mut() = Timing {
                entry_started: Some(Instant::now()),
                ..Timing::default()
            });
        }
        if msg == WM_MOVING || msg == WM_SIZING {
            TIMING.with(|state| {
                let mut state = state.borrow_mut();
                if state.first_motion_ms.is_none() {
                    state.first_motion_ms = state.entry_started.map(|start| start.elapsed().as_secs_f64() * 1000.0);
                }
            });
        }
        let start = Instant::now();
        // Do not bypass any framework handlers or alter window movement.
        let result = unsafe { DefSubclassProc(hwnd, msg, w, l) };
        if matches!(msg, WM_SETFOCUS | WM_ACTIVATE | WM_ENTERSIZEMOVE) {
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            if msg == WM_ENTERSIZEMOVE {
                TIMING.with(|state| state.borrow_mut().entry_ms = ms);
            }
            tracing::info!(message = msg, elapsed_ms = ms, "native focus/drag-entry timing");
        }
        if matches!(msg, WM_MOVE | WM_MOVING | WM_SIZE | WM_WINDOWPOSCHANGED) {
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            TIMING.with(|state| {
                let mut state = state.borrow_mut();
                state.calls += 1;
                state.slow += u64::from(ms > 8.0);
                state.max_ms = state.max_ms.max(ms);
                state.total_ms += ms;
            });
        }
        if msg == WM_EXITSIZEMOVE {
            TIMING.with(|state| {
                let state = state.borrow();
                tracing::info!(
                    calls = state.calls,
                    slow_over_8ms = state.slow,
                    max_ms = state.max_ms,
                    total_ms = state.total_ms,
                    entry_handler_ms = state.entry_ms,
                    first_motion_ms = state.first_motion_ms,
                    "native window drag/resize timing"
                );
            });
        }
        if msg == WM_NCDESTROY {
            unsafe {
                RemoveWindowSubclass(hwnd, Some(procedure), ID);
            }
        }
        result
    }
    if let Ok(hwnd) = window.hwnd() {
        let installed = unsafe { SetWindowSubclass(hwnd.0 as HWND, Some(procedure), ID, 0) };
        if installed == 0 {
            tracing::warn!("could not install native window diagnostics");
        }
    }
}

#[cfg(not(windows))]
pub fn install(_: &tauri::WebviewWindow) {}

/// A minimal window with the same Tauri runtime and size, and no engine or user data.
pub fn run_probe() {
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    tauri::Builder::default()
        .setup(|app| {
            let window = tauri::WebviewWindowBuilder::new(
                app,
                "window-probe",
                tauri::WebviewUrl::App("window-probe.html".into()),
            )
            .title("Zoltraak — Window Probe")
            .inner_size(1450.0, 840.0)
            .general_autofill_enabled(false)
            .data_directory(
                std::env::temp_dir().join(format!("zoltraak-window-probe-{}", std::process::id())),
            )
            .build()?;
            install(&window);
            Ok(())
        })
        .run(context)
        .expect("window probe failed");
}
