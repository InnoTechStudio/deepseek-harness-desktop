#![cfg(target_os = "windows")]

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tauri::WebviewWindow;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOREDIRECTIONBITMAP,
};

const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWCP_ROUND: u32 = 2;

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(hwnd: HWND, attribute: u32, value: *const core::ffi::c_void, size: u32) -> i32;
}

pub fn install<R: tauri::Runtime>(window: &WebviewWindow<R>) -> Result<(), String> {
    let handle = window.window_handle().map_err(|e| e.to_string())?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("unexpected non-Windows window handle".to_string());
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);
    unsafe {
        let preference = DWMWCP_ROUND;
        let dark_mode = 1u32;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &preference as *const _ as _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark_mode as *const _ as _, 4);
        let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        if exstyle & WS_EX_NOREDIRECTIONBITMAP.0 as isize != 0 {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, exstyle & !(WS_EX_NOREDIRECTIONBITMAP.0 as isize));
        }
    }
    Ok(())
}
