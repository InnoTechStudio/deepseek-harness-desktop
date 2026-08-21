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
    Emitter, Manager, WindowEvent,
};

pub const APP_NAME: &str = "DSH Desk";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = Arc::new(AppState::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(app_state)
        .setup(|app| {
            // 创建主窗口（在 tauri.conf 里定义，这里确保可见 + 单实例）
            setup_window(app.handle());
            setup_tray(app.handle())?;
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
            // 托盘事件
            if let tauri::RunEvent::TrayIconEvent(
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                },
            ) = event
            {
                if let Some(window) = app_handle.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        });
}

fn setup_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "打开 DSH Desk", true, None::<&str>)?;
    let check = MenuItem::with_id(app, "check-update", "检查更新", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &check, &sep, &quit])?;

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
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "check-update" => {
                // 通知前端打开设置页
                let _ = app.emit("menu-check-update", ());
            }
            "quit" => {
                // 结束 dsh 子进程
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
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        })
        .build(app)?;
    Ok(())
}
