//! Real file paths for files dropped onto the page.
//!
//! The window leaves Tauri's own drag-and-drop handling off (`dragDropEnabled:
//! false`) so that links and pictures dragged out of a browser reach the page as
//! ordinary HTML drops. The cost is that a dropped file reaches the page as a
//! `File` with no path, which is no use to a converter that works on files in
//! place.
//!
//! WebView2 has a supported way round that: the page posts a message along with
//! the dropped `File` objects, and the host receives them as `CoreWebView2File`
//! values that carry the path. This listens for that one message and forwards
//! the paths to the page as an event. Anywhere without WebView2 the page finds
//! no `chrome.webview` and says so, rather than guessing.

/// The message the page posts alongside the dropped files.
pub const DROP_MESSAGE: &str = "zoltraak:drop-paths";
/// The event the paths come back on.
pub const DROPPED_PATHS_EVENT: &str = "zoltraak://dropped-paths";

#[cfg(windows)]
pub fn install(window: &tauri::WebviewWindow) {
    use tauri::Emitter;
    use webview2_com::{
        take_pwstr,
        Microsoft::Web::WebView2::Win32::{
            ICoreWebView2File, ICoreWebView2WebMessageReceivedEventArgs2,
        },
        WebMessageReceivedEventHandler,
    };
    use windows::core::Interface;

    let emitter = window.clone();
    let result = window.with_webview(move |webview| unsafe {
        let Ok(core) = webview.controller().CoreWebView2() else {
            return;
        };
        let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            // Only the drop message; everything else belongs to Tauri's own IPC.
            let mut message = windows::core::PWSTR::null();
            if args.TryGetWebMessageAsString(&mut message).is_err() {
                return Ok(());
            }
            if take_pwstr(message) != DROP_MESSAGE {
                return Ok(());
            }
            let objects = args.cast::<ICoreWebView2WebMessageReceivedEventArgs2>()?.AdditionalObjects()?;
            let mut count = 0;
            objects.Count(&mut count)?;
            let mut paths = Vec::new();
            for index in 0..count {
                let Ok(item) = objects.GetValueAtIndex(index) else {
                    continue;
                };
                if let Ok(file) = item.cast::<ICoreWebView2File>() {
                    let mut path = windows::core::PWSTR::null();
                    if file.Path(&mut path).is_ok() {
                        paths.push(take_pwstr(path));
                    }
                }
            }
            let _ = emitter.emit(DROPPED_PATHS_EVENT, paths);
            Ok(())
        }));
        let mut token = Default::default();
        if core.add_WebMessageReceived(&handler, &mut token).is_err() {
            tracing::warn!("could not listen for dropped file paths");
        }
    });
    if result.is_err() {
        tracing::warn!("could not reach the web view to listen for dropped file paths");
    }
}

#[cfg(not(windows))]
pub fn install(_: &tauri::WebviewWindow) {}
