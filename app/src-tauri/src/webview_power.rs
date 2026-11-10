//! A window hidden to the tray gives its memory and CPU back.
//!
//! WebView2 keeps the whole page alive when the window is hidden. Asking it
//! for a low memory target lets it trim caches, and suspending it stops page
//! timers and rendering. Downloads, notifications and the tray are native and
//! keep running. WebView2 resumes a suspended page by itself when it becomes
//! visible; [`shown`] also restores the normal memory target.

#[cfg(windows)]
fn set_hidden(window: &tauri::WebviewWindow, hidden: bool) {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            ICoreWebView2_19, ICoreWebView2_3, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
        },
        TrySuspendCompletedHandler,
    };
    use windows::core::Interface;

    let result = window.with_webview(move |webview| unsafe {
        let Ok(core) = webview.controller().CoreWebView2() else {
            return;
        };
        // Older runtimes lack these interfaces; the window then simply keeps
        // its memory, as before.
        if let Ok(core) = core.cast::<ICoreWebView2_19>() {
            let level = if hidden {
                COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
            } else {
                COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
            };
            let _ = core.SetMemoryUsageTargetLevel(level);
        }
        // Hiding the native window does not tell WebView2 it is hidden. While
        // it still counts as visible it keeps compositing and TrySuspend is
        // refused, so the GPU process stayed busy behind the tray icon.
        let controller = webview.controller();
        if !hidden {
            let _ = controller.SetIsVisible(true);
        }
        if let Ok(core) = core.cast::<ICoreWebView2_3>() {
            if hidden {
                let _ = controller.SetIsVisible(false);
                let handler = TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(())));
                let _ = core.TrySuspend(&handler);
            } else {
                let _ = core.Resume();
            }
        } else if hidden {
            let _ = controller.SetIsVisible(false);
        }
    });
    if let Err(e) = result {
        tracing::debug!(error = %e, hidden, "could not change webview power state");
    }
}

#[cfg(not(windows))]
fn set_hidden(_window: &tauri::WebviewWindow, _hidden: bool) {}

/// Call after hiding the window to the tray.
pub fn hidden(window: &tauri::WebviewWindow) {
    set_hidden(window, true);
}

/// Call when the window is back in front of the user.
pub fn shown(window: &tauri::WebviewWindow) {
    set_hidden(window, false);
}
