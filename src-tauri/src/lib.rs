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
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

pub const APP_NAME: &str = "DSH Desk";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = Arc::new(AppState::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(app_state)
        .setup(|app| {
            // 创建主窗口（在 tauri.conf 里定义，这里确保可见 + 单实例）
            setup_window(app.handle());
            setup_tray(app.handle())?;
            // 启动后台内核更新检查（每 6h 一次；发现新版弹原生对话框）
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
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app_handle, event| {
            // 托盘左键点击恢复窗口（RunEvent 层）
            if let tauri::RunEvent::TrayIconEvent(
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                },
            ) = event
            {
                restore_window(app_handle);
            }
        });
}

fn restore_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
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
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => restore_window(app),
            "check-update" => check_and_notify(app),
            "rollback" => {
                if let Some(state) = app.try_state::<Arc<AppState>>() {
                    tauri::async_runtime::block_on(async {
                        let _ = crate::commands::rollback_impl(&state).await;
                    });
                }
            }
            "data" => {
                crate::commands::open_data_dir_impl();
            }
            "quit" => quit_app(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                restore_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}
