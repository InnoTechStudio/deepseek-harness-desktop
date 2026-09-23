//! DSH Desk 入口：注册命令、托盘、启动流程

mod commands;
mod downloader;
mod dsh;
mod kernel;
mod mirrors;
#[cfg(target_os = "macos")]
mod notify_mac;
mod paths;
mod platform;
mod proxy;
mod runtime;
mod state;
#[cfg(target_os = "windows")]
mod windows_frame;
#[cfg(target_os = "windows")]
mod windows_permissions;

use state::AppState;
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WindowEvent,
};
#[cfg(target_os = "macos")]
use tauri::RunEvent;
use serde_json::json;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

pub const APP_NAME: &str = "DeepSeek Harness";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = Arc::new(AppState::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // 二次启动：恢复窗口到前台
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(app_state)
        .menu(build_app_menu)
        .setup(|app| {
            #[cfg(target_os = "windows")]
            {
                if let Some(window) = app.get_webview_window("main") {
                    if let Err(error) = crate::windows_frame::install(&window) {
                        log_line(&format!("windows frame install warning: {error}"));
                    }
                    // WebView2 默认拒绝跨源 iframe 的剪贴板/麦克风请求，
                    // 内核界面正是跑在 iframe 里，复制按钮会静默失效。
                    if let Err(error) = crate::windows_permissions::install(&window) {
                        log_line(&format!("windows permission handler warning: {error}"));
                    }
                    let _ = window.set_decorations(false);
                    let _ = window.set_shadow(false);
                }
            }
            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_decorations(true);
            }
            setup_window(app.handle());
            #[cfg(target_os = "windows")]
            {
                // Tauri's empty menu still reserves a native menu host on some WebView2 builds.
                // Remove it after window creation, matching the reference desktop shell.
                let _ = app.remove_menu();
            }
            setup_vibrancy(app.handle());
            if let Some(state) = app.try_state::<Arc<AppState>>() {
                crate::kernel::remove_legacy_skill_market(state.paths());
                crate::kernel::suppress_market_console(state.paths());
                crate::platform::apply_all(app.handle(), state.paths());
            }
            setup_tray(app.handle())?;
            if let Some(state) = app.try_state::<Arc<AppState>>() {
                if let Ok(resource_dir) = app.path().resource_dir() {
                    let desktop_shell = resource_dir.join("resources/dsh-desktop-shell");
                    if desktop_shell.exists() && state.paths().root.join("dsh-home/profiles/web/package.json").exists() {
                        if let Err(error) = crate::kernel::preinstall_desktop_shell(
                            state.paths(), "https://registry.npmjs.org", &desktop_shell,
                        ) {
                            crate::commands::log_to_file(state.paths(), &format!("startup: desktop shell install warning: {error}"));
                        }
                        crate::kernel::suppress_market_console(state.paths());
                    }
                }
            }
            spawn_update_checker(app.handle().clone());
            spawn_watchdog(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::bootstrap,
            commands::install_kernel,
            commands::start_dsh,
            commands::stop_dsh,
            commands::check_update,
            commands::apply_update,
            commands::restart_dsh,
            commands::skip_version,
            commands::get_settings,
            commands::set_settings,
            commands::rollback,
            commands::confirm_healthy,
            commands::speed_probe,
            commands::check_client_update,
            commands::download_client_update,
            commands::get_workspace_drop_path,
            commands::set_desktop_notifications,
            commands::window_minimize,
            commands::window_toggle_maximize,
            commands::window_close,
            commands::send_test_notification,
            commands::open_notification_settings,
            commands::open_path,
            commands::notify_desktop,
            commands::submit_feedback,
            refresh_update_availability,
        ])
        .on_window_event(|window, event| {
            // 红色关闭按钮行为：根据设置决定隐藏还是退出
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    // app.exit() may emit a final close request. Let that request finish
                    // once the shared quit state has claimed shutdown.
                    if QUIT_IN_PROGRESS.load(std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                    api.prevent_close();
                    request_quit(&window.app_handle(), true);
                }
            }
        })
        // macOS 应用菜单事件；Windows 菜单栏被完全关闭，托盘事件直接复用同一处理器。
        .on_menu_event(handle_menu_event)
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let RunEvent::Reopen { .. } = event {
                restore_window(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

/// 统一的菜单/托盘动作处理（app 菜单 + 托盘菜单共用同一组 id）
fn handle_menu_event(
    app: &tauri::AppHandle,
    event: tauri::menu::MenuEvent,
) {
    match event.id().as_ref() {
        "show" | "restore" => restore_window(app),
        "file-hide" => hide_window(app),
        "toggle-window" | "hide" => toggle_window(app),
        "check-update" => check_and_notify(app),
        "about" => show_about(app),
        "quit" => request_quit(app, false),
        _ => {}
    }
}

/// 关于对话框
fn show_about(app: &tauri::AppHandle) {
    // 不再使用系统原生对话框，改为发送事件让前端显示自定义对话框
    let _ = app.emit("show-about-dialog", ());
}

/// Request application quit from any entry point. Exactly one caller may own the prompt or shutdown.
fn request_quit(app: &tauri::AppHandle, from_close_request: bool) {
    let settings = crate::paths::Settings::load(&crate::paths::Paths::resolve());
    if !settings.confirm_exit {
        if from_close_request {
            hide_window(app);
        } else if QUIT_IN_PROGRESS.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok() {
            quit_app(app);
        }
        return;
    }
    confirm_and_quit(app);
}

/// Show one confirmation dialog, then enter the single shutdown path.
fn confirm_and_quit(app: &tauri::AppHandle) {
    use tauri_plugin_dialog::DialogExt;
    if QUIT_IN_PROGRESS.load(std::sync::atomic::Ordering::SeqCst)
        || QUIT_PROMPT_OPEN.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_err()
    {
        return;
    }
    let yes = app
        .dialog()
        .message("确定要退出 DeepSeek Harness 吗？")
        .title("退出 DeepSeek Harness")
        .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancelCustom("退出".into(), "取消".into()))
        .blocking_show();
    if yes {
        // Claim shutdown before clearing the prompt flag. A platform close event can
        // be delivered while the native dialog is unwinding.
        if QUIT_IN_PROGRESS.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok() {
            QUIT_PROMPT_OPEN.store(false, std::sync::atomic::Ordering::SeqCst);
            quit_app(app);
        }
    } else {
        QUIT_PROMPT_OPEN.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 追加一行到数据目录 logs/app.log
fn log_line(msg: &str) {
    use std::io::Write;
    let paths = crate::paths::Paths::resolve();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_dir.join("tray.log"))
    {
        let _ = writeln!(f, "[{}] {}", std::process::id(), msg);
    }
}

/// 应用菜单（macOS 顶部/Dock，Windows 主菜单）- 极简设计
/// 应用菜单。
///
/// Windows 保持无菜单：菜单栏会破坏无边框标题栏的融合效果，动作都在托盘里。
/// 复制粘贴等编辑快捷键在 Windows 由 WebView2 自己处理，不依赖应用菜单。
///
/// macOS 必须提供「编辑」菜单，否则 WKWebView 收不到 Cmd+C/V 这类快捷键——
/// 系统的剪贴板动作是通过菜单项分发的，没有菜单项就没有接收者。
/// 「设置」不放进菜单：客户端设置已迁移到 DSH 自己的设置面板，
/// 而 DSH 面板的开关状态是页面内部状态，宿主无法可靠地远程打开它。
fn build_app_menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    #[cfg(target_os = "windows")]
    {
        // Remove the native menu host on Windows; actions remain in the tray menu.
        Menu::with_items(app, &[])
    }
    #[cfg(not(target_os = "windows"))]
    {
        let about = MenuItem::with_id(app, "about", "关于 DeepSeek Harness", true, None::<&str>)?;
        let check = MenuItem::with_id(app, "check-update", "检查更新…", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "退出", true, Some("CmdOrCtrl+Q"))?;
        let sep = PredefinedMenuItem::separator(app)?;
        let dshdesk = Submenu::with_items(app, "DeepSeek Harness", true, &[&about, &sep, &check, &sep, &quit])?;

        // 预定义项自带平台标准快捷键，传中文文本即可本地化。
        let edit = Submenu::with_items(app, "编辑", true, &[
            &PredefinedMenuItem::undo(app, Some("撤销"))?,
            &PredefinedMenuItem::redo(app, Some("重做"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some("剪切"))?,
            &PredefinedMenuItem::copy(app, Some("复制"))?,
            &PredefinedMenuItem::paste(app, Some("粘贴"))?,
            &PredefinedMenuItem::select_all(app, Some("全选"))?,
        ])?;

        Menu::with_items(app, &[&dshdesk, &edit])
    }
}

/// 隐藏主窗口但保持应用和 dsh 子进程驻留。
fn hide_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        log_line("window: hide");
        let _ = window.hide();
    }
}

fn toggle_window(app: &tauri::AppHandle) {
    let visible = app
        .get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if visible { hide_window(app); } else { restore_window(app); }
}
fn restore_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        log_line("restore: show()");
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        // macOS：托盘点击时 app 未激活，需把应用激活 + 把窗口带到最前
        #[cfg(target_os = "macos")]
        {
            window_activate(&window);
        }
        log_line(&format!(
            "restore: visible={} focused={}",
            window.is_visible().unwrap_or(false),
            window.is_focused().unwrap_or(false)
        ));
    }
}

#[cfg(target_os = "macos")]
fn window_activate(_w: &tauri::WebviewWindow) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    let mtm = MainThreadMarker::new().expect("must be on main thread");
    let app = NSApplication::sharedApplication(mtm);
    unsafe {
        app.activateIgnoringOtherApps(true);
    }
}

/// 后台内核更新检查：每 6h 一次。发现新版弹原生对话框让用户决定。
fn spawn_update_checker(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        loop {
            // 等应用就绪后先查一次，然后每 6h 一次
            std::thread::sleep(std::time::Duration::from_secs(20));
            // 后台检查：静默（已是最新版不打扰）
            if let Some(state) = app
                .try_state::<Arc<AppState>>()
                .map(|s| s.inner().clone())
            {
                let settings = crate::paths::Settings::load(state.paths());
                if settings.auto_check_updates {
                    run_check_notify(app.clone(), state, false);
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(6 * 3600));
        }
    });
}

/// 供前端在 DSH 界面就绪后主动索要一次更新状态。
///
/// 后台任务启动 20 秒后才跑第一轮，而 iframe 通常更早就绪；iframe 刷新后状态
/// 也会丢。没有这条命令，标题栏的"新版本"按钮就无从判断该不该显示。
#[tauri::command]
async fn refresh_update_availability(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let settings = crate::paths::Settings::load(state.paths());
    if !settings.auto_check_updates {
        // 用户关掉了自动检查，就不该看到"新版本"按钮。
        let _ = app.emit(
            "update-availability",
            json!({ "available": false, "kernel": false, "client": false }),
        );
        return Ok(());
    }

    // 先发"检查中"占位，iframe 的 useEffect 能立刻收到，不会因异步延迟错过。
    let _ = app.emit(
        "update-availability",
        json!({ "available": false, "kernel": false, "client": false }),
    );

    run_check_notify(app.clone(), state.inner().clone(), false);
    Ok(())
}

/// 看门狗：dsh 服务若意外退出，自动重启（保证常驻稳定）。
fn spawn_watchdog(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(10));
            let Some(state) = app
                .try_state::<Arc<AppState>>()
                .map(|s| s.inner().clone())
            else {
                continue;
            };
            let state2 = state.clone();
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                let mut g = state2.dsh.lock().await;
                let Some(d) = g.as_mut() else { return };
                // 子进程已退出 → 重启
                if let Ok(Some(_)) = d.child.try_wait() {
                    let _ = d.child.wait();
                    drop(g);
                    match crate::commands::restart_dsh_impl(&app2, &state2).await {
                        Ok(port) => {
                            log_line(&format!("watchdog: dsh 已重启，端口 {port}"));
                            crate::platform::notify(
                                &app2,
                                crate::platform::NotifyKind::JobComplete,
                                "DeepSeek Harness",
                                "本地服务已自动恢复运行。",
                            );
                        }
                        Err(error) => {
                            log_line(&format!("watchdog: dsh 重启失败: {error}"));
                            crate::platform::notify(
                                &app2,
                                crate::platform::NotifyKind::JobFailed,
                                "DeepSeek Harness",
                                "本地服务异常退出且自动重启失败，请打开应用查看。",
                            );
                        }
                    }
                }
            });
        }
    });
}

fn check_and_notify(app: &tauri::AppHandle) {
    let Some(state) = app
        .try_state::<Arc<AppState>>()
        .map(|s| s.inner().clone())
    else {
        return;
    };
    run_check_notify(app.clone(), state, true);
}

/// Serialize native-close and tray-quit confirmation flows.
static QUIT_PROMPT_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static QUIT_IN_PROGRESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 检查内核更新并弹窗。`manual` 为 true 时（用户点菜单"检查更新"），
/// 即使已是最新版也弹"已是最新版"提示；后台自动检查则静默。
///
/// 无论有无更新都发一次 `update-availability`：标题栏的"新版本"按钮靠它决定
/// 显示与否，只在有更新时才发会让按钮一旦出现就再也不消失。
fn run_check_notify(app: tauri::AppHandle, state: Arc<AppState>, manual: bool) {
    tauri::async_runtime::spawn(async move {
        // 只在已装内核时检查（避免首启时和引导并发）
        let installed = crate::kernel::active_kernel_version(state.paths());
        if installed.is_none() {
            return;
        }

        // 检查内核更新
        let Ok((latest, _registry)) =
            crate::kernel::latest_kernel_version(&state.client, state.paths()).await
        else {
            return;
        };
        let kernel_available = match &installed {
            Some(i) => crate::kernel::compare_versions(&latest, i) > 0,
            None => false,
        };

        // 跳过检查
        let skipped = crate::paths::Settings::load(state.paths()).skipped_kernel_version;
        let skipped_this = skipped.as_deref() == Some(latest.as_str());
        let kernel_update = kernel_available && !skipped_this;

        // 检查客户端更新
        let client_update = check_client_update_internal(&state.client).await.ok();
        let has_client_update = client_update.as_ref().map(|c| c.update_available).unwrap_or(false);

        // 标题栏按钮的唯一依据，每次检查都上报（含"没有更新"）。
        let _ = app.emit(
            "update-availability",
            json!({
                "available": has_client_update || kernel_update,
                "kernel": kernel_update,
                "client": has_client_update,
            }),
        );

        // 只在有更新时发送通知（自动检查模式）或手动检查时发送
        if manual || has_client_update || kernel_update {
            let mut payload = json!({
                "installed": installed.unwrap_or_default(),
                "latest": latest,
                "updateAvailable": kernel_update
            });

            if let Some(client) = client_update {
                payload["clientUpdate"] = json!(client);
            }

            let _ = app.emit("show-update-result", payload);
        }

        // 后台自动检查发现新版本时，额外发一条桌面通知（窗口可能被隐藏）。
        if !manual && (has_client_update || kernel_update) {
            crate::platform::notify(
                &app,
                crate::platform::NotifyKind::Service,
                "DeepSeek Harness 有新版本",
                "发现可用更新，打开应用即可查看并安装。",
            );
        }
    });
}

async fn check_client_update_internal(client: &reqwest::Client) -> Result<crate::commands::ClientUpdateInfo, String> {
    crate::commands::fetch_client_update_info(client).await
}

fn setup_vibrancy(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
        let _ = apply_vibrancy(
            &window,
            NSVisualEffectMaterial::Sidebar,
            Some(NSVisualEffectState::FollowsWindowActiveState),
            Some(12.0),
        );
        let _ = window.set_title_bar_style(tauri::TitleBarStyle::Overlay);
    }
    #[cfg(target_os = "windows")]
    {
        use window_vibrancy::apply_mica;
        let _ = apply_mica(&window, Some(true));
    }
}

fn setup_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn quit_app(app: &tauri::AppHandle) {
    // 释放防休眠等系统级占用，再结束 dsh 子进程
    crate::platform::release_all();
    if let Some(state) = app.try_state::<Arc<AppState>>() {
        tauri::async_runtime::block_on(async {
            let mut g = state.dsh.lock().await;
            if let Some(d) = g.as_mut() {
                let _ = d.child.kill();
                let _ = d.child.wait();
            }
            *g = None;
        });
    }
    app.exit(0);
}

fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    // 使用自定义的托盘图标（纯黑色 logo，macOS 会自动转换为白色）
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/IconTemplate@2x.png"))
        .expect("failed to load tray icon");

    // Windows 没有应用菜单栏，所有动作通过托盘右键菜单提供。
    // macOS 保持原有的精简托盘菜单不变。
    #[cfg(target_os = "windows")]
    let menu = {
        let show = MenuItem::with_id(app, "show", "显示应用", true, None::<&str>)?;
        let check = MenuItem::with_id(app, "check-update", "检查更新…", true, None::<&str>)?;
        let about = MenuItem::with_id(app, "about", "关于 DeepSeek Harness", true, None::<&str>)?;
        let sep = PredefinedMenuItem::separator(app)?;
        let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
        Menu::with_items(app, &[&show, &sep, &check, &sep, &about, &sep, &quit])?
    };
    #[cfg(not(target_os = "windows"))]
    let menu = {
        let show = MenuItem::with_id(app, "tray-show", "显示应用", true, None::<&str>)?;
        let sep = PredefinedMenuItem::separator(app)?;
        let quit = MenuItem::with_id(app, "tray-quit", "退出", true, None::<&str>)?;
        Menu::with_items(app, &[&show, &sep, &quit])?
    };

    let _tray = TrayIconBuilder::with_id("dshdesk-tray")
        .icon(icon)
        .icon_as_template(true)
        .tooltip(APP_NAME)
        .menu(&menu)
        .menu_on_left_click(false)
        .on_menu_event(|app, event| {
            #[cfg(target_os = "windows")]
            {
                // The application-level handler receives Windows tray menu events.
                // Registering a second tray handler would show two quit dialogs.
                let _ = (app, event);
            }
            #[cfg(not(target_os = "windows"))]
            match event.id().as_ref() {
                "tray-show" => restore_window(app),
                "tray-quit" => {
                    log_line("tray-quit");
                    request_quit(app, false);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            match event {
                // 左键单击托盘 → 切换窗口显示/隐藏
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    log_line("left-click toggle");
                    toggle_window(tray.app_handle());
                }
                _ => {}
            }
        })
        .build(app)?;
    Ok(())
}
