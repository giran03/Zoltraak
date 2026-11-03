//! Native window corners, kept in step with the Corner Radius setting.
//!
//! The window has no system frame, so Windows 11 draws its corners from this
//! preference rather than from anything the page can style. It offers square,
//! small-radius and standard-radius corners, which the four settings map onto;
//! older Windows and other platforms simply keep their own corners.

/// The DWM corner preference for a Corner Radius setting.
fn preference(radius: &str) -> i32 {
    match radius {
        // DWMWCP_DONOTROUND
        "sharp" => 1,
        // DWMWCP_ROUNDSMALL
        "bevel" => 3,
        // DWMWCP_ROUND
        _ => 2,
    }
}

#[cfg(windows)]
pub fn apply(window: &tauri::WebviewWindow, radius: &str) {
    use windows_sys::Win32::{Foundation::HWND, Graphics::Dwm::DwmSetWindowAttribute};
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;

    let Ok(hwnd) = window.hwnd() else { return };
    let value = preference(radius);
    // Fails harmlessly before Windows 11, which has no corner preference.
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd.0 as HWND,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::addr_of!(value).cast(),
            std::mem::size_of::<i32>() as u32,
        )
    };
    if result != 0 {
        tracing::debug!(result, "window corner preference not applied");
    }
}

#[cfg(not(windows))]
pub fn apply(_: &tauri::WebviewWindow, _: &str) {}

#[tauri::command]
pub fn set_window_corners(window: tauri::WebviewWindow, radius: String) {
    apply(&window, &radius);
}

#[cfg(test)]
mod tests {
    use super::preference;

    #[test]
    fn each_radius_maps_to_a_native_preference() {
        assert_eq!(preference("sharp"), 1);
        assert_eq!(preference("bevel"), 3);
        assert_eq!(preference("modern"), 2);
        assert_eq!(preference("rounded"), 2);
        assert_eq!(preference("anything else"), 2);
    }
}
