//! 平台能力：开机自启、防休眠、桌面通知。
//! 每个设置开关都必须映射到真实的系统行为，而不是只存一个布尔值。

use crate::paths::{Paths, Settings};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::AppHandle;

/// 防休眠当前是否已生效，避免重复申请或重复释放。
static SLEEP_BLOCKED: AtomicBool = AtomicBool::new(false);

/// 启动时把已保存的设置同步到系统状态。
pub fn apply_all(app: &AppHandle, paths: &Paths) {
    let settings = Settings::load(paths);
    if let Err(error) = apply_auto_launch(app, settings.auto_launch) {
        crate::commands::log_to_file(paths, &format!("auto_launch apply failed: {error}"));
    }
    if let Err(error) = apply_prevent_sleep(settings.prevent_sleep) {
        crate::commands::log_to_file(paths, &format!("prevent_sleep apply failed: {error}"));
    }
    // 提前申请通知授权：系统只在第一次询问，而且要等用户点完才会登记本应用。
    // 放到真正要发通知时再问，第一条通知就会被丢掉。
    #[cfg(target_os = "macos")]
    if settings.desktop_notifications {
        crate::notify_mac::request_authorization();
    }
}

/// 退出前释放所有系统级占用。
pub fn release_all() {
    let _ = apply_prevent_sleep(false);
}

/// 当前可执行文件路径（自启动注册需要绝对路径）。
fn current_exe() -> Result<std::path::PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("无法获取程序路径: {e}"))
}

// ---------- 开机自启 ----------

#[cfg(target_os = "windows")]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(target_os = "windows")]
const RUN_VALUE: &str = "DeepSeekHarness";

/// Windows: 写入/删除 HKCU Run 项，通过 reg.exe 保持无额外依赖且免管理员权限。
#[cfg(target_os = "windows")]
pub fn apply_auto_launch(_app: &AppHandle, enabled: bool) -> Result<(), String> {
    let mut cmd = std::process::Command::new("reg.exe");
    if enabled {
        let exe = current_exe()?;
        cmd.args([
            "add",
            &format!(r"HKCU\{RUN_KEY}"),
            "/v",
            RUN_VALUE,
            "/t",
            "REG_SZ",
            "/d",
            &format!("\"{}\"", exe.display()),
            "/f",
        ]);
    } else {
        cmd.args(["delete", &format!(r"HKCU\{RUN_KEY}"), "/v", RUN_VALUE, "/f"]);
    }
    crate::dsh::suppress_console(&mut cmd);
    let output = cmd.output().map_err(|e| format!("注册表操作失败: {e}"))?;
    // 关闭时如果本来没有该项，reg delete 会返回非零，这是预期结果。
    if !output.status.success() && enabled {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(())
}

/// 读取 Windows 自启动项是否真实存在。
#[cfg(target_os = "windows")]
pub fn auto_launch_enabled() -> bool {
    let mut cmd = std::process::Command::new("reg.exe");
    cmd.args(["query", &format!(r"HKCU\{RUN_KEY}"), "/v", RUN_VALUE]);
    crate::dsh::suppress_console(&mut cmd);
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

/// LaunchAgent 的作业标签。
///
/// 不能直接用 bundle identifier：应用运行时 macOS 会在 GUI 域里自动注册
/// `application.com.dshdesk.app.<hash>`，同名标签的 LaunchAgent 会被 launchd
/// 以 `Input/output error` 拒绝加载——文件躺在磁盘上但作业从未登记。
#[cfg(target_os = "macos")]
const LAUNCH_AGENT_LABEL: &str = "com.dshdesk.app.launcher";

#[cfg(target_os = "macos")]
fn launch_agent_path() -> Result<std::path::PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "无法定位用户目录".to_string())?;
    Ok(home.join(format!("Library/LaunchAgents/{LAUNCH_AGENT_LABEL}.plist")))
}

/// 从可执行文件路径回推 .app 包路径。
/// 可执行文件在 `Foo.app/Contents/MacOS/<可执行名>`，需要向上三级。
#[cfg(target_os = "macos")]
fn app_bundle_path() -> Result<std::path::PathBuf, String> {
    let exe = current_exe()?;
    let bundle = exe
        .parent()
        .and_then(|macos| macos.parent())
        .and_then(|contents| contents.parent())
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .ok_or_else(|| "未找到应用包路径，请先把应用拖入「应用程序」后重试".to_string())?;
    // 从 DMG 里直接运行时登录项会指向 /Volumes/...，卸载镜像后就失效了。
    if bundle.starts_with("/Volumes/") {
        return Err("请先把应用拖入「应用程序」文件夹，再开启开机自启".to_string());
    }
    Ok(bundle.to_path_buf())
}

/// `gui/<uid>`，launchctl 的用户域。
#[cfg(target_os = "macos")]
fn gui_domain() -> String {
    // 走 `id -u` 而不是引入 libc：只为拿一个 uid 不值得多一个依赖。
    let uid = std::process::Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|text| text.trim().to_string())
        .unwrap_or_default();
    format!("gui/{uid}")
}

/// `gui/<uid>/<label>`，用于 bootout 与状态查询。
#[cfg(target_os = "macos")]
fn gui_service_target() -> String {
    format!("{}/{LAUNCH_AGENT_LABEL}", gui_domain())
}

/// launchd 是否真的登记了这个作业。
///
/// 只看 plist 文件存在是不够的：文件写下了但 launchd 拒绝加载时，
/// 开关会显示"已开启"而开机后什么也不会发生。
#[cfg(target_os = "macos")]
fn launchd_job_registered() -> bool {
    std::process::Command::new("launchctl")
        .arg("print")
        .arg(gui_service_target())
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// macOS: 写入/删除 LaunchAgent。
///
/// 用 LaunchAgent 而不是 System Events 的登录项 API：后者要通过 AppleScript
/// 驱动 System Events 进程，会让系统弹出"System Events 正在后台运行"这类
/// 与本应用无关的提示，还要额外索取自动化权限。
///
/// plist 里执行的是 `/usr/bin/open -a <App>` 而不是可执行文件本身：直接让
/// launchd 运行二进制会把本进程变成受管作业，关闭自启时会顺带杀掉正在运行的
/// 应用，而且脱离 .app 启动的进程不复用应用包身份，Dock 上会多出一个图标。
#[cfg(target_os = "macos")]
pub fn apply_auto_launch(_app: &AppHandle, enabled: bool) -> Result<(), String> {
    let plist = launch_agent_path()?;
    if !enabled {
        // 先删文件再注销：即使注销影响到当前进程，磁盘状态也已经是"已关闭"。
        if plist.exists() {
            std::fs::remove_file(&plist).map_err(|e| format!("移除登录项失败: {e}"))?;
        }
        let _ = std::process::Command::new("launchctl")
            .arg("bootout")
            .arg(gui_service_target())
            .output();
        return Ok(());
    }

    let bundle = app_bundle_path()?;
    let bundle_str = bundle.display().to_string();
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCH_AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/bin/open</string>
        <string>-a</string>
        <string>{bundle_str}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#
    );
    if let Some(parent) = plist.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建登录项目录失败: {e}"))?;
    }
    std::fs::write(&plist, body).map_err(|e| format!("写入登录项失败: {e}"))?;

    // 重复 bootstrap 会报 already loaded，先注销再登记。
    let _ = std::process::Command::new("launchctl")
        .arg("bootout")
        .arg(gui_service_target())
        .output();
    let output = std::process::Command::new("launchctl")
        .arg("bootstrap")
        .arg(gui_domain())
        .arg(&plist)
        .output()
        .map_err(|e| format!("无法调用 launchctl: {e}"))?;
    // 登记失败时把文件删掉：留着会让开关显示"已开启"而系统里其实没有登录项。
    if !output.status.success() && !launchd_job_registered() {
        let _ = std::fs::remove_file(&plist);
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            "系统拒绝登记登录项".to_string()
        } else {
            format!("系统拒绝登记登录项：{detail}")
        });
    }
    Ok(())
}

/// 读取 macOS 登录项是否真实生效：文件与 launchd 登记都要在。
#[cfg(target_os = "macos")]
pub fn auto_launch_enabled() -> bool {
    launch_agent_path().map(|p| p.exists()).unwrap_or(false) && launchd_job_registered()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn apply_auto_launch(_app: &AppHandle, _enabled: bool) -> Result<(), String> {
    Err("当前平台不支持开机自启".to_string())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn auto_launch_enabled() -> bool {
    false
}

// ---------- 防休眠 ----------

#[cfg(target_os = "windows")]
mod sleep_win {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
    const ES_AWAYMODE_REQUIRED: u32 = 0x0000_0040;

    /// 保持系统不进入睡眠；display 仍可关闭以省电。
    pub fn set(enabled: bool) -> Result<(), String> {
        let flags = if enabled {
            ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_AWAYMODE_REQUIRED
        } else {
            ES_CONTINUOUS
        };
        let previous = unsafe { SetThreadExecutionState(flags) };
        if previous == 0 {
            return Err("SetThreadExecutionState 调用失败".to_string());
        }
        Ok(())
    }
}

/// macOS 上用 caffeinate 子进程持有电源断言，进程结束即自动释放。
#[cfg(target_os = "macos")]
mod sleep_mac {
    use std::sync::Mutex;
    static CAFFEINATE: Mutex<Option<std::process::Child>> = Mutex::new(None);

    pub fn set(enabled: bool) -> Result<(), String> {
        let mut guard = CAFFEINATE.lock().map_err(|_| "电源断言状态锁失败".to_string())?;
        if enabled {
            if guard.is_some() {
                return Ok(());
            }
            let child = std::process::Command::new("/usr/bin/caffeinate")
                .arg("-i")
                .arg("-w")
                .arg(std::process::id().to_string())
                .spawn()
                .map_err(|e| format!("启动 caffeinate 失败: {e}"))?;
            *guard = Some(child);
        } else if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }
}

/// 应用防休眠开关；重复调用是安全的。
pub fn apply_prevent_sleep(enabled: bool) -> Result<(), String> {
    if SLEEP_BLOCKED.load(Ordering::SeqCst) == enabled {
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    sleep_win::set(enabled)?;
    #[cfg(target_os = "macos")]
    sleep_mac::set(enabled)?;
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    return Err("当前平台不支持防休眠".to_string());
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        SLEEP_BLOCKED.store(enabled, Ordering::SeqCst);
        Ok(())
    }
}

/// 防休眠是否已真实生效。
pub fn prevent_sleep_active() -> bool {
    SLEEP_BLOCKED.load(Ordering::SeqCst)
}

// ---------- 桌面通知 ----------

/// 通知类别，与设置里的分类开关一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyKind {
    TurnComplete,
    TurnFailed,
    JobComplete,
    JobFailed,
    Service,
}

impl NotifyKind {
    /// 从前端/插件传入的类别名解析。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "turn-complete" => Some(NotifyKind::TurnComplete),
            "turn-failed" => Some(NotifyKind::TurnFailed),
            "job-complete" => Some(NotifyKind::JobComplete),
            "job-failed" => Some(NotifyKind::JobFailed),
            "service" => Some(NotifyKind::Service),
            _ => None,
        }
    }

    /// 该类别是否被用户单独关闭。
    fn allowed(self, settings: &Settings) -> bool {
        match self {
            NotifyKind::TurnComplete => settings.notification_turn_complete,
            NotifyKind::TurnFailed => settings.notification_turn_failed,
            NotifyKind::JobComplete => settings.notification_job_complete,
            NotifyKind::JobFailed => settings.notification_job_failed,
            // 服务级提醒（内核更新、服务重启）只受总开关控制。
            NotifyKind::Service => true,
        }
    }
}

/// 发送一条桌面通知。总开关关闭或分类关闭时静默丢弃。
/// @returns 是否真的发出了通知。
pub fn notify(app: &AppHandle, kind: NotifyKind, title: &str, body: &str) -> bool {
    notify_detailed(app, kind, title, body).is_ok()
}

/// 与 `notify` 相同，但把失败原因带回来，供设置里的测试通知显示给用户。
/// @returns Ok(true) 已投递；Ok(false) 被开关拦下；Err 系统拒绝。
pub fn notify_detailed(
    app: &AppHandle,
    kind: NotifyKind,
    title: &str,
    body: &str,
) -> Result<bool, String> {
    let paths = Paths::resolve();
    let settings = Settings::load(&paths);
    if !settings.desktop_notifications || !kind.allowed(&settings) {
        return Ok(false);
    }
    match deliver(app, title, body) {
        Ok(()) => Ok(true),
        Err(error) => {
            crate::commands::log_to_file(&paths, &format!("notification failed: {error}"));
            Err(error)
        }
    }
}

/// macOS 下投递通知。
///
/// 走 UNUserNotificationCenter（见 `notify_mac`）：这是现在唯一能让通知带上本应用
/// 名称和图标的接口，而且不需要付费签名。插件通道（NSUserNotification）只在没有
/// .app 身份时兜底，比如 `cargo tauri dev` 直接跑二进制的场景。
#[cfg(target_os = "macos")]
fn deliver(app: &AppHandle, title: &str, body: &str) -> Result<(), String> {
    match crate::notify_mac::deliver(title, body) {
        Ok(()) => Ok(()),
        Err(error) => {
            crate::commands::log_to_file(
                &Paths::resolve(),
                &format!("notification: UN 通道失败（{error}），改用插件通道"),
            );
            deliver_via_plugin(app, title, body).map_err(|fallback| {
                // 两条通道都失败时，UN 的原因更接近根因（权限/身份），优先呈现。
                format!("{error}；插件通道也失败：{fallback}")
            })
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn deliver(app: &AppHandle, title: &str, body: &str) -> Result<(), String> {
    deliver_via_plugin(app, title, body)
}

fn deliver_via_plugin(app: &AppHandle, title: &str, body: &str) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod notify_kind_tests {
    use super::*;

    /// 分类名是前端与插件共用的线上协议，写错会静默丢通知。
    #[test]
    fn known_kinds_parse() {
        assert_eq!(NotifyKind::parse("turn-complete"), Some(NotifyKind::TurnComplete));
        assert_eq!(NotifyKind::parse("turn-failed"), Some(NotifyKind::TurnFailed));
        assert_eq!(NotifyKind::parse("job-complete"), Some(NotifyKind::JobComplete));
        assert_eq!(NotifyKind::parse("job-failed"), Some(NotifyKind::JobFailed));
        assert_eq!(NotifyKind::parse("service"), Some(NotifyKind::Service));
    }

    #[test]
    fn unknown_kind_is_rejected() {
        assert_eq!(NotifyKind::parse("turn_complete"), None);
        assert_eq!(NotifyKind::parse(""), None);
    }

    #[test]
    fn category_switches_gate_their_own_kind() {
        let mut settings = Settings::default();
        settings.notification_turn_complete = false;
        settings.notification_turn_failed = true;
        assert!(!NotifyKind::TurnComplete.allowed(&settings));
        assert!(NotifyKind::TurnFailed.allowed(&settings));
    }

    #[test]
    fn service_kind_ignores_category_switches() {
        // 内核更新、服务重启这类提醒只受总开关控制，分类开关全关也要能发。
        let settings = Settings::default();
        assert!(!settings.notification_turn_complete);
        assert!(NotifyKind::Service.allowed(&settings));
    }
}
