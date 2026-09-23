//! Windows 上放行 WebView2 的剪贴板等权限请求。
//!
//! 内核界面跑在 iframe 里，与宿主跨源（宿主 `tauri://localhost`，内核走
//! `http://127.0.0.1:<代理端口>`）。WebView2 是 Chromium，对跨源 iframe 的
//! 权限请求默认**拒绝**且不弹窗，于是内核里的复制按钮调
//! `navigator.clipboard.writeText()` 直接静默失败。
//!
//! macOS 的 WKWebView 不执行 Permissions-Policy、也默认放行媒体捕获，
//! 所以同一份内核代码在 Mac 上复制正常——差异出在 WebView，不在内核。
//!
//! wry 自带的处理只放行了 `CLIPBOARD_READ`（见 wry `webview2/mod.rs`），
//! 写剪贴板和麦克风都不在其中，必须自己再挂一个处理器。
//! 两个处理器会依次收到同一个事件，后挂的把状态改成 ALLOW 即可生效。

#![cfg(target_os = "windows")]

use tauri::WebviewWindow;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_CAMERA,
    COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
    COREWEBVIEW2_PERMISSION_STATE_ALLOW, ICoreWebView2_2,
};
use webview2_com::PermissionRequestedEventHandler;
use windows::core::Interface;

/// 这些权限一律自动放行，不弹窗。
///
/// 都是用户主动点了界面上的按钮才会触发的能力，而且请求方只可能是我们自己
/// 拉起的本地内核；再弹一次系统层面的确认对用户没有意义。
fn is_allowed(kind: COREWEBVIEW2_PERMISSION_KIND) -> bool {
    kind == COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ
        || kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE
        || kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA
}

/// 给主窗口挂上权限放行处理器。失败不致命，只是复制会退回到旧行为。
pub fn install<R: tauri::Runtime>(window: &WebviewWindow<R>) -> Result<(), String> {
    window
        .with_webview(|webview| {
            let controller = webview.controller();
            let Ok(core) = (unsafe { controller.CoreWebView2() }) else {
                return;
            };
            // PermissionRequested 定义在 ICoreWebView2 上，但我们要的
            // 是能拿到 deferral 的版本；这里只用同步分支，取基础接口即可。
            let Ok(core2) = core.cast::<ICoreWebView2_2>() else {
                return;
            };
            let mut token = Default::default();
            let handler = PermissionRequestedEventHandler::create(Box::new(|_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                unsafe { args.PermissionKind(&mut kind)? };
                if is_allowed(kind) {
                    unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)? };
                }
                Ok(())
            }));
            let _ = unsafe { core2.add_PermissionRequested(&handler, &mut token) };
        })
        .map_err(|e| format!("无法挂载 WebView2 权限处理器: {e}"))
}
