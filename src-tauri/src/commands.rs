//! 命令层：把后端能力暴露给前端
use crate::kernel;
use crate::mirrors;
use crate::runtime;
use crate::state::{AppState, DshRuntime};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

pub type DynProgress = Option<Arc<dyn Fn(runtime::DownloadProgress) + Send + Sync>>;

/// 状态摘要
#[tauri::command]
pub async fn get_state(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let paths = state.paths();
    let node_ready = runtime::node_ready(paths);
    let kernel_ready = kernel::kernel_bin(paths).exists();
    let installed = kernel::active_kernel_version(paths);
    let dsh_guard = state.dsh.lock().await;
    let running = dsh_guard.is_some();
    let port = dsh_guard.as_ref().map(|d| d.port);
    drop(dsh_guard);
    Ok(serde_json::json!({
        "nodeReady": node_ready,
        "kernelReady": kernel_ready,
        "installedVersion": installed,
        "running": running,
        "port": port,
        "platform": paths.platform(),
    }))
}

/// 初始化：确保 Node + 内核就绪。返回状态。
#[tauri::command]
pub async fn bootstrap(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<String, String> {
    use crate::paths::Paths;
    let paths = state.paths();
    std::fs::create_dir_all(&paths.root).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&paths.log_dir).map_err(|e| e.to_string())?;
    log_to_file(paths, "bootstrap: start");

    let cb = make_dl_cb(app.clone(), "node");
    if !runtime::node_ready(paths) {
        log_to_file(paths, "bootstrap: node not ready, installing");
        runtime::install_node_runtime(&state.client, paths, None, cb)
            .await
            .inspect_err(|e| log_to_file(paths, &format!("bootstrap: node install FAILED: {e}")))?;
    }
    log_to_file(paths, "bootstrap: node ready");
    emit(&app, "log", "Node 运行时就绪");
    Ok("ready".into())
}

pub fn log_to_file(paths: &crate::paths::Paths, msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_dir.join("app.log"))
    {
        let _ = writeln!(f, "[{}] {}", chrono_like_ts(), msg);
    }
}

fn chrono_like_ts() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{:?}", d.as_secs()))
        .unwrap_or_default()
}

/// 下载并安装内核（到 staging → 原子切换）。完整初始化。
#[tauri::command]
pub async fn install_kernel(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    use crate::paths::Paths;
    let paths = state.paths();
    log_to_file(paths, "install_kernel: start");
    let (version, registry) = kernel::latest_kernel_version(&state.client, paths)
        .await
        .inspect_err(|e| log_to_file(paths, &format!("install_kernel: version check FAILED: {e}")))?;
    log_to_file(paths, &format!("install_kernel: found latest v{version} @ {registry}"));
    let node = runtime::node_bin(paths);
    if !node.exists() {
        return Err("Node 运行时缺失".to_string());
    }
    let cb = make_log_cb(app.clone());
    let res = kernel::install_kernel_to(&node, &paths.staging_dir, &registry, &version, cb).await;
    if let Err(e) = &res {
        log_to_file(paths, &format!("install_kernel: pnpm install FAILED: {e}"));
        return Err(e.clone());
    }
    log_to_file(paths, "install_kernel: pnpm ok, swapping");
    kernel::swap_staging_to_overlay(paths)?;
    kernel::confirm_healthy_cleanup(paths);
    log_to_file(paths, "install_kernel: done");
    emit(&app, "log", format!("内核 v{version} 安装完成"));
    Ok(version)
}

/// 启动 dsh 服务并等待就绪（返回端口）
#[tauri::command]
pub async fn start_dsh(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<u16, String> {
    let paths = state.paths();
    // 若已在跑，直接返回端口
    {
        let g = state.dsh.lock().await;
        if let Some(d) = g.as_ref() {
            return Ok(d.port);
        }
    }
    let mut child = crate::dsh::spawn_dsh(paths, "web-desktop")?;
    let port = wait_for_port(&mut child, &state).await?;
    *state.dsh.lock().await = Some(DshRuntime { child, port });
    emit(&app, "log", format!("dsh 服务已就绪，端口 {}", port));
    Ok(port)
}

/// 等待 dsh 输出打印 URL 端口
async fn wait_for_port(child: &mut std::process::Child, state: &AppState) -> Result<u16, String> {
    let stdout = child.stdout.take();
    if let Some(mut so) = stdout {
        use std::io::{BufRead, BufReader};
        let (tx, mut rx) = tokio::sync::mpsc::channel::<u16>(4);
        std::thread::spawn(move || {
            let reader = BufReader::new(&mut so);
            for l in reader.lines().flatten() {
                if let Some(p) = crate::dsh::parse_port_from_line(&l) {
                    let _ = tx.blocking_send(p);
                    break;
                }
            }
        });
        if let Some(p) = rx.recv().await {
            return Ok(p);
        }
    }
    // fallback：轮询常见端口范围 / 试 3080
    for _ in 0..80 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if is_http_ok(&state.client, 3080).await {
            return Ok(3080);
        }
    }
    Err("dsh 未在预期时间内就绪".to_string())
}

/// 探测端口是否返回 HTTP 200
pub async fn is_http_ok(client: &reqwest::Client, port: u16) -> bool {
    let url = format!("http://127.0.0.1:{}/", port);
    match client.get(&url).timeout(std::time::Duration::from_secs(1)).send().await {
        Ok(r) => r.status().is_success() || r.status().is_redirection(),
        Err(_) => false,
    }
}

/// 停止 dsh 服务
#[tauri::command]
pub async fn stop_dsh(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let mut g = state.dsh.lock().await;
    if let Some(d) = g.as_mut() {
        let _ = d.child.kill();
        let _ = d.child.wait();
    }
    *g = None;
    Ok(())
}

/// 检查内核更新
#[tauri::command]
pub async fn check_update(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    let paths = state.paths();
    let installed = kernel::active_kernel_version(paths);
    let (latest, registry) = kernel::latest_kernel_version(&state.client, paths).await?;
    let available = match &installed {
        Some(i) => kernel::compare_versions(&latest, i) > 0,
        None => true,
    };
    let skipped = crate::paths::Settings::load(paths).skipped_kernel_version;
    let skipped_this = skipped.as_deref() == Some(latest.as_str());
    emit(&app, "log", format!("检测更新：最新 {}，已装 {:?}", latest, installed));
    Ok(serde_json::json!({
        "installed": installed,
        "latest": latest,
        "updateAvailable": available && !skipped_this,
        "registry": registry,
    }))
}

/// 应用内核升级（下载新版 → staging → 原子切换 → 重启）
#[tauri::command]
pub async fn apply_update(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let _paths = state.paths();
    {
        let mut u = state.updating.lock().await;
        if *u {
            return Err("升级已在进行中".to_string());
        }
        *u = true;
    }
    let res = do_apply_update(&app, &state).await;
    *state.updating.lock().await = false;
    res
}

async fn do_apply_update(app: &AppHandle, state: &AppState) -> Result<String, String> {
    let paths = state.paths();
    let (version, registry) = kernel::latest_kernel_version(&state.client, paths).await?;
    let node = runtime::node_bin(paths);
    if !node.exists() {
        return Err("Node 运行时缺失".to_string());
    }
    // 停 dsh 再换
    {
        let mut g = state.dsh.lock().await;
        if let Some(d) = g.as_mut() {
            let _ = d.child.kill();
            let _ = d.child.wait();
        }
        *g = None;
    }
    let cb = make_log_cb(app.clone());
    kernel::install_kernel_to(&node, &paths.staging_dir, &registry, &version, cb).await?;
    kernel::swap_staging_to_overlay(paths)?;
    emit(app, "log", format!("升级完成，已切换到 v{version}"));
    Ok(version)
}

/// 跳过某版本
#[tauri::command]
pub async fn skip_version(
    state: State<'_, Arc<AppState>>,
    version: String,
) -> Result<(), String> {
    let paths = state.paths();
    let mut s = crate::paths::Settings::load(paths);
    s.skipped_kernel_version = Some(version);
    s.save(paths);
    Ok(())
}

/// 回退到上一版本
#[tauri::command]
pub async fn rollback(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    kernel::rollback(state.paths())
}

/// 健康确认并清理备份
#[tauri::command]
pub async fn confirm_healthy(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    kernel::confirm_healthy_cleanup(state.paths());
    Ok(())
}

/// 测速（调试/设置页）
#[tauri::command]
pub async fn speed_probe(state: State<'_, Arc<AppState>>) -> Result<Vec<mirrors::ProbeOut>, String> {
    let mirrors = mirrors::npm_mirrors();
    let results = mirrors::speed_test(&state.client, &mirrors).await;
    Ok(mirrors::probe_out(&results))
}

/// 打开数据目录（调试用）
#[tauri::command]
pub async fn open_data_dir(_app: AppHandle) -> Result<(), String> {
    let paths = crate::paths::Paths::resolve();
    let _ = std::fs::create_dir_all(&paths.root);
    open_path(&paths.root);
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_path(p: &std::path::Path) {
    let _ = std::process::Command::new("open").arg(p).spawn();
}

#[cfg(target_os = "windows")]
fn open_path(p: &std::path::Path) {
    let _ = std::process::Command::new("explorer").arg(p).spawn();
}

#[cfg(target_os = "linux")]
fn open_path(p: &std::path::Path) {
    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
}

// ---------- helpers ----------

fn make_dl_cb(app: AppHandle, _kind: &'static str) -> DynProgress {
    Some(Arc::new(move |p| {
        let _ = app.emit("download-progress", p);
    }))
}

fn make_log_cb(app: AppHandle) -> Option<Arc<dyn Fn(String) + Send + Sync>> {
    Some(Arc::new(move |line| {
        let _ = app.emit("kernel-log", line);
    }))
}

fn emit<E: serde::Serialize + Clone>(app: &AppHandle, event: &str, payload: E) {
    let _ = app.emit(event, payload);
}
