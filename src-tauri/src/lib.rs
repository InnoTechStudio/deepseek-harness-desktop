//! DSH Desk 入口：注册命令、托盘、启动流程

mod commands;
mod downloader;
mod dsh;
mod kernel;
mod mirrors;
mod paths;
mod runtime;
mod state;

use state::AppState;
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

pub const APP_NAME: &str = "DSH Desk";

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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(app_state)
        // 应用菜单（Dock/顶部菜单栏）：标准 macOS 菜单栏应用放动作的地方
        .menu(build_app_menu)
        .setup(|app| {
            setup_window(app.handle());
            setup_tray(app.handle())?;
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
            commands::rollback,
            commands::confirm_healthy,
            commands::speed_probe,
            commands::open_data_dir,
        ])
        .on_window_event(|window, event| {
            // 关闭窗口 → 直接退出程序（托盘恢复在 macOS 上不可靠，退出更符合预期）
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    quit_app(window.app_handle());
                }
            }
        })
        // 应用菜单（macOS 顶部/Dock，Windows 主菜单）事件
        .on_menu_event(handle_menu_event)
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_app_handle, _event| {});
}

/// 统一的菜单/托盘动作处理（app 菜单 + 托盘菜单共用同一组 id）
fn handle_menu_event(
    app: &tauri::AppHandle,
    event: tauri::menu::MenuEvent,
) {
    match event.id().as_ref() {
        "show" => restore_window(app),
        "check-update" => check_and_notify(app),
        "rollback" => {
            if let Some(state) = app.try_state::<Arc<AppState>>() {
                tauri::async_runtime::block_on(async {
                    let _ = crate::commands::rollback_impl(&state).await;
                });
            }
        }
        "data" => crate::commands::open_data_dir_impl(),
        "about" => show_about(app),
        "quit" => quit_app(app),
        _ => {}
    }
}

/// 关于对话框
fn show_about(app: &tauri::AppHandle) {
    use tauri_plugin_dialog::DialogExt;
    let _ = app
        .dialog()
        .message(
            "DSH Desk\n\nDeepSeek Harness 桌面客户端\n\n版本 0.1.0\n\n轻量、免装环境、安装即用、自动跟随官方升级。",
        )
        .title("关于 DSH Desk")
        .blocking_show();
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

/// 应用菜单（macOS 顶部/Dock，Windows 主菜单）
fn build_app_menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let show = tauri::menu::MenuItem::with_id(app, "show", "打开 DSH Desk", true, None::<&str>)?;
    let check = tauri::menu::MenuItem::with_id(app, "check-update", "检查内核更新", true, None::<&str>)?;
    let rollback = tauri::menu::MenuItem::with_id(app, "rollback", "回退到上一版本", true, None::<&str>)?;
    let data = tauri::menu::MenuItem::with_id(app, "data", "打开数据目录", true, None::<&str>)?;
    let about = tauri::menu::MenuItem::with_id(app, "about", "关于 DSH Desk", true, None::<&str>)?;
    let quit = tauri::menu::MenuItem::with_id(app, "quit", "退出 DSH Desk", true, None::<&str>)?;
    let sep = tauri::menu::PredefinedMenuItem::separator(app)?;
    tauri::menu::Menu::with_items(app, &[&show, &check, &rollback, &data, &about, &sep, &quit])
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
                run_check_notify(app.clone(), state, false);
            }
            std::thread::sleep(std::time::Duration::from_secs(6 * 3600));
        }
    });
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
                    if let Ok(port) = crate::commands::restart_dsh_impl(&app2, &state2).await {
                        log_line(&format!("watchdog: dsh 已重启，端口 {port}"));
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
    let app = app.clone();
    run_check_notify(app, state, true);
}

/// 检查内核更新并弹窗。`manual` 为 true 时（用户点菜单"检查更新"），
/// 即使已是最新版也弹"已是最新版"提示；后台自动检查则静默。
fn run_check_notify(app: tauri::AppHandle, state: Arc<AppState>, manual: bool) {
    tauri::async_runtime::spawn(async move {
        use tauri_plugin_dialog::DialogExt;
        // 只在已装内核时检查（避免首启时和引导并发）
        let installed = crate::kernel::active_kernel_version(state.paths());
        if installed.is_none() {
            if manual {
                let _ = app
                    .dialog()
                    .message("内核尚未安装，请先完成初始化。")
                    .title("DSH Desk")
                    .blocking_show();
            }
            return;
        }
        let Ok((latest, _registry)) =
            crate::kernel::latest_kernel_version(&state.client, state.paths()).await
        else {
            if manual {
                let _ = app
                    .dialog()
                    .message("无法连接更新源，请检查网络后重试。")
                    .title("DSH Desk")
                    .blocking_show();
            }
            return;
        };
        let available = match &installed {
            Some(i) => crate::kernel::compare_versions(&latest, i) > 0,
            None => false,
        };
        // 跳过检查
        let skipped = crate::paths::Settings::load(state.paths()).skipped_kernel_version;
        let skipped_this = skipped.as_deref() == Some(latest.as_str());

        if !available || skipped_this {
            if manual {
                let _ = app
                    .dialog()
                    .message(format!(
                        "已是最新版本 v{latest}（当前 v{}）。",
                        installed.unwrap_or_default()
                    ))
                    .title("DSH Desk")
                    .blocking_show();
            }
            return;
        }
        let installed_s = installed.unwrap_or_default();
        let latest_s = latest.clone();
        let app2 = app.clone();
        let state2 = state.clone();
        tauri::async_runtime::spawn(async move {
            let yes = app2
                .dialog()
                .message(format!(
                    "发现 DeepSeek Harness 官方更新 v{latest_s}（当前 v{installed_s}），是否立即更新？"
                ))
                .title("DSH Desk")
                .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancelCustom(
                    "立即更新".into(),
                    "稍后".into(),
                ))
                .blocking_show();
            if yes {
                let _ = crate::commands::apply_update_impl(&app2, &state2).await;
                // 更新后重启 dsh 服务
                let _ = crate::commands::restart_dsh_impl(&app2, &state2).await;
            }
        });
    });
}

fn setup_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn quit_app(app: &tauri::AppHandle) {
    // 结束 dsh 子进程再退出
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
    let show = MenuItem::with_id(app, "show", "打开 DSH Desk", true, None::<&str>)?;
    let check = MenuItem::with_id(app, "check-update", "检查内核更新", true, None::<&str>)?;
    let rollback = MenuItem::with_id(app, "rollback", "回退到上一版本", true, None::<&str>)?;
    let data = MenuItem::with_id(app, "data", "打开数据目录", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &check, &rollback, &data, &sep, &quit])?;

    let icon = app
        .default_window_icon()
        .cloned()
        .expect("default window icon missing");

    let _tray = TrayIconBuilder::with_id("dshdesk-tray")
        .icon(icon)
        .tooltip(APP_NAME)
        .menu(&menu)
        // 标准 macOS 菜单栏应用交互：
        //   左键单击 → 触发 TrayIconEvent::Click → 恢复/隐藏窗口
        //   右键单击 → 弹出菜单（打开/检查更新/回退/数据/退出）
        // macOS 上若左键也弹菜单，Click 事件会被系统吞掉导致无法恢复窗口。
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, event| {
            log_line(&format!("tray-icon-event: {event:?}"));
            // 左键单击托盘 → 恢复窗口（macOS/Windows/Linux 一致）
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                ..
            } = event
            {
                log_line("left-click restore");
                restore_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}
