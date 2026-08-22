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
        .menu(|app| build_app_menu(app))
        .setup(|app| {
            setup_window(app.handle());
            setup_tray(app.handle())?;
            spawn_update_checker(app.handle().clone());
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
            commands::skip_version,
            commands::rollback,
            commands::confirm_healthy,
            commands::speed_probe,
            commands::open_data_dir,
        ])
        .on_window_event(|window, event| {
            // 关闭窗口 → 最小化到托盘（不退出）
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        // 应用菜单（macOS 顶部/Dock，Windows 主菜单）事件
        .on_menu_event(|app, event| handle_menu_event(app, event))
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
        "quit" => quit_app(app),
        _ => {}
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

/// 应用菜单（macOS 顶部/Dock，Windows 主菜单）
fn build_app_menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let show = tauri::menu::MenuItem::with_id(app, "show", "打开 DSH Desk", true, None::<&str>)?;
    let check = tauri::menu::MenuItem::with_id(app, "check-update", "检查内核更新", true, None::<&str>)?;
    let rollback = tauri::menu::MenuItem::with_id(app, "rollback", "回退到上一版本", true, None::<&str>)?;
    let data = tauri::menu::MenuItem::with_id(app, "data", "打开数据目录", true, None::<&str>)?;
    let quit = tauri::menu::MenuItem::with_id(app, "quit", "退出 DSH Desk", true, None::<&str>)?;
    let sep = tauri::menu::PredefinedMenuItem::separator(app)?;
    tauri::menu::Menu::with_items(app, &[&show, &check, &rollback, &data, &sep, &quit])
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
            check_and_notify(&app);
            std::thread::sleep(std::time::Duration::from_secs(6 * 3600));
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
    run_check_notify(app, state);
}

#[allow(clippy::too_many_arguments)]
fn run_check_notify(app: tauri::AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        // 只在已装内核时检查（避免首启时和引导并发）
        let installed = crate::kernel::active_kernel_version(state.paths());
        if installed.is_none() {
            return;
        }
        let Ok((latest, _registry)) =
            crate::kernel::latest_kernel_version(&state.client, state.paths()).await
        else {
            return;
        };
        let available = match &installed {
            Some(i) => crate::kernel::compare_versions(&latest, i) > 0,
            None => false,
        };
        if !available {
            return;
        }
        // 跳过检查
        let skipped = crate::paths::Settings::load(state.paths()).skipped_kernel_version;
        if skipped.as_deref() == Some(latest.as_str()) {
            return;
        }
        let installed_s = installed.unwrap_or_default();
        let latest_s = latest.clone();
        let app2 = app.clone();
        let state2 = state.clone();
        tauri::async_runtime::spawn(async move {
            use tauri_plugin_dialog::DialogExt;
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
        .on_menu_event(|app, event| handle_menu_event(app, event))
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
