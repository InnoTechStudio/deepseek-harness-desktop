//! macOS 原生通知：走 UNUserNotificationCenter。
//!
//! 为什么不用 tauri-plugin-notification：它经 notify_rust → mac-notification-sys
//! 调的是已废弃的 NSUserNotification，那套接口靠 `installNSBundleHook()` 把进程的
//! bundle 身份偷换成目标 App 的，通知的署名和图标全取决于这个 hook 是否生效——
//! 于是出现「有名字、没图标」这种半成品状态。
//!
//! UNUserNotificationCenter 按调用进程真实的 bundle 身份归属通知，名称和图标直接
//! 取自 .app 自己的 Info.plist 与 icon.icns。这条路不需要付费开发者签名：ad-hoc
//! 签名（codesign --sign -）加上应用位于 /Applications 让 LaunchServices 认得它，
//! 就足够了；已在本机实测投递成功。

use core::ptr::NonNull;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_foundation::{NSBundle, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent,
    UNNotificationRequest, UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
};

/// 投递回调等待上限。走本机 XPC，正常毫秒级返回。
/// 与 AUTH_WAIT 相加要留在前端桥的 8s 超时之内。
const DELIVER_TIMEOUT: Duration = Duration::from_secs(3);
/// 首次授权最多等这么久。系统弹窗要用户点，不能把命令堵死。
const AUTH_WAIT: Duration = Duration::from_secs(3);

/// 进程是否有真实的 .app 身份。
///
/// 没有 bundle 身份时 `currentNotificationCenter` 会抛
/// `bundleProxyForCurrentProcess is nil` 直接崩掉进程，所以必须先挡住——
/// `cargo tauri dev` 跑裸二进制就是这种情况。
fn has_bundle_identity() -> bool {
    let bundle = NSBundle::mainBundle();
    if bundle.bundleIdentifier().is_none() {
        return false;
    }
    bundle.bundlePath().to_string().ends_with(".app")
}

/// 通知中心。没有 bundle 身份时返回 None，交给调用方回退。
fn center() -> Option<Retained<UNUserNotificationCenter>> {
    if !has_bundle_identity() {
        return None;
    }
    Some(UNUserNotificationCenter::currentNotificationCenter())
}

/// 读取当前授权状态；读不到返回 None。
fn authorization_status() -> Option<UNAuthorizationStatus> {
    let center = center()?;
    let (tx, rx) = mpsc::sync_channel(1);
    let block = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        let _ = tx.send(unsafe { settings.as_ref() }.authorizationStatus());
    });
    center.getNotificationSettingsWithCompletionHandler(&block);
    rx.recv_timeout(DELIVER_TIMEOUT).ok()
}

/// 申请授权，不等结果。系统只会在第一次真正弹窗。
pub fn request_authorization() {
    let Some(center) = center() else { return };
    let block = RcBlock::new(|_granted: Bool, _error: *mut NSError| {});
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &block,
    );
}

/// 尚未授权时申请一次，并给用户留出点系统弹窗的时间。
/// @returns 等待结束时的授权状态。
fn settle_authorization() -> Option<UNAuthorizationStatus> {
    request_authorization();
    let deadline = Instant::now() + AUTH_WAIT;
    let mut status = authorization_status();
    while status != Some(UNAuthorizationStatus::Authorized) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        status = authorization_status();
    }
    status
}

/// 投递一条通知，返回系统给出的真实结果。
pub fn deliver(title: &str, body: &str) -> Result<(), String> {
    let Some(center) = center() else {
        return Err("进程没有 .app 身份，无法使用系统通知".to_string());
    };

    // 没授权就先要权限，再按最终状态决定怎么回话。
    // 直接硬发的话系统只回一句 "Notifications are not allowed"，
    // 用户看不出到底是没点允许，还是之前拒绝过。
    let mut status = authorization_status();
    if status != Some(UNAuthorizationStatus::Authorized) {
        status = settle_authorization();
    }
    match status {
        Some(UNAuthorizationStatus::Authorized) => {}
        Some(UNAuthorizationStatus::Denied) => {
            return Err(
                "系统已禁止本应用发送通知，请到「系统设置 → 通知 → DeepSeek Harness」重新允许"
                    .to_string(),
            );
        }
        _ => {
            return Err("系统正在询问通知权限，请在弹出的对话框里点「允许」后重新开启".to_string());
        }
    }

    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    content.setSound(Some(&UNNotificationSound::defaultSound()));

    // 标识符只需在本应用内唯一，纳秒时间戳足够，不必为此再引入 NSUUID。
    let identifier = format!(
        "dsh-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&identifier),
        &content,
        None,
    );

    let (tx, rx) = mpsc::sync_channel(1);
    let block = RcBlock::new(move |error: *mut NSError| {
        let message = NonNull::new(error)
            .map(|error| unsafe { error.as_ref() }.localizedDescription().to_string());
        let _ = tx.send(message);
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&block));

    match rx.recv_timeout(DELIVER_TIMEOUT) {
        Ok(None) => Ok(()),
        Ok(Some(message)) => Err(message),
        Err(_) => Err("系统通知服务未在预期时间内响应".to_string()),
    }
}
