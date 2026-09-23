//! 命令层：把后端能力暴露给前端
use crate::kernel;
use crate::mirrors;
use crate::runtime;
use crate::state::{AppState, DshRuntime};
use std::sync::Arc;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, State};

pub type DynProgress = Option<Arc<dyn Fn(runtime::DownloadProgress) + Send + Sync>>;

#[tauri::command]
pub async fn window_minimize(app: AppHandle) -> Result<(), String> {
    app.get_webview_window("main").ok_or_else(|| "主窗口不存在".to_string())?
        .minimize().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn window_toggle_maximize(app: AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or_else(|| "主窗口不存在".to_string())?;
    let maximized = window.is_maximized().map_err(|e| e.to_string())?;
    if maximized { window.unmaximize() } else { window.maximize() }.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn window_close(app: AppHandle) -> Result<(), String> {
    app.get_webview_window("main").ok_or_else(|| "主窗口不存在".to_string())?
        .close().map_err(|e| e.to_string())
}


#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BootstrapProgress {
    step: u8,
    percent: f64,
    message: String,
}

fn emit_bootstrap_progress(app: &AppHandle, step: u8, percent: f64, message: impl Into<String>) {
    let _ = app.emit(
        "bootstrap-progress",
        BootstrapProgress { step, percent: monotonic_percent(percent), message: message.into() },
    );
}

/// 整个引导流程共用一条 0→100 的进度轴，各阶段占用固定区间。
/// 这样"下载完成"到"开始安装"之间不会出现百分比回退。
mod stage {
    /// 运行环境：测速 + 下载 + 解压
    pub const NODE_PROBE: (f64, f64) = (0.0, 4.0);
    pub const NODE_DOWNLOAD: (f64, f64) = (4.0, 34.0);
    pub const NODE_EXTRACT: (f64, f64) = (34.0, 40.0);
    /// 内核：查版本 + pnpm 安装 + 切换 + 附加组件
    pub const KERNEL_RESOLVE: (f64, f64) = (40.0, 44.0);
    pub const KERNEL_INSTALL: (f64, f64) = (44.0, 76.0);
    pub const KERNEL_SWAP: (f64, f64) = (76.0, 80.0);
    pub const KERNEL_EXTRAS: (f64, f64) = (80.0, 88.0);
    /// 服务启动
    pub const SERVICE_START: (f64, f64) = (88.0, 96.0);
    pub const SERVICE_READY: (f64, f64) = (96.0, 100.0);

    /// 把某阶段内部的 0..1 比例映射到全局百分比。
    pub fn at(range: (f64, f64), ratio: f64) -> f64 {
        let ratio = ratio.clamp(0.0, 1.0);
        range.0 + (range.1 - range.0) * ratio
    }
}

/// 已上报的最高百分比（×100 存成整数便于原子操作）。
static PROGRESS_FLOOR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 进度只允许前进，防止不同阶段之间来回跳。
fn monotonic_percent(percent: f64) -> f64 {
    use std::sync::atomic::Ordering;
    let scaled = (percent.clamp(0.0, 100.0) * 100.0) as u64;
    let previous = PROGRESS_FLOOR.fetch_max(scaled, Ordering::SeqCst);
    previous.max(scaled) as f64 / 100.0
}

/// 新一轮引导开始时重置进度下限（用户重试时需要从头走）。
fn reset_progress_floor() {
    PROGRESS_FLOOR.store(0, std::sync::atomic::Ordering::SeqCst);
}

/// 人类可读的字节数，用于进度文案。
fn human_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if (bytes as f64) < MB {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / MB)
    }
}


#[tauri::command]
pub async fn get_workspace_drop_path(path: String) -> Result<String, String> {
    let candidate = PathBuf::from(path);
    let canonical = std::fs::canonicalize(&candidate).map_err(|e| format!("无法读取工作区目录: {e}"))?;
    if !canonical.is_dir() {
        return Err("拖入的项目不是文件夹".to_string());
    }
    Ok(canonical.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn set_desktop_notifications(
    state: State<'_, Arc<AppState>>,
    turn_complete: Option<bool>,
    turn_failed: Option<bool>,
    job_complete: Option<bool>,
    job_failed: Option<bool>,
) -> Result<(), String> {
    let paths = state.paths();
    let mut settings = crate::paths::Settings::load(paths);
    if let Some(v) = turn_complete { settings.notification_turn_complete = v; }
    if let Some(v) = turn_failed { settings.notification_turn_failed = v; }
    if let Some(v) = job_complete { settings.notification_job_complete = v; }
    if let Some(v) = job_failed { settings.notification_job_failed = v; }
    settings.save(paths);
    Ok(())
}

/// DSH Web/插件可调用的轻量生命周期状态。
#[tauri::command]
pub async fn get_plugin_lifecycle(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let profile = state.paths().root.join("dsh-home/profiles/web/package.json");
    let manifest = std::fs::read_to_string(&profile).unwrap_or_else(|_| "{}".to_string());
    let json: serde_json::Value = serde_json::from_str(&manifest).unwrap_or_default();
    Ok(serde_json::json!({
        "profile": "web",
        "bundles": json.pointer("/dsh/profile/bundles").cloned().unwrap_or_else(|| serde_json::json!([])),
        "dependencies": json.get("dependencies").cloned().unwrap_or_else(|| serde_json::json!({})),
        "restartRequired": false,
    }))
}

/// 状态摘要
#[tauri::command]
pub async fn get_state(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let paths = state.paths();
    let node_ready = runtime::node_ready(paths);
    let kernel_ready = kernel::kernel_healthy(paths);
    let installed = kernel::active_kernel_version(paths);
    let dsh_guard = state.dsh.lock().await;
    let running = dsh_guard.is_some();
    let port = dsh_guard.as_ref().map(|d| d.port);
    // 前端要用这条完整地址加载 iframe：新版内核靠 URL 里的令牌换鉴权 cookie，
    // 用端口自己拼会被判 401。
    let url = dsh_guard.as_ref().map(|d| d.url.clone());
    drop(dsh_guard);
    Ok(serde_json::json!({
        "nodeReady": node_ready,
        "kernelReady": kernel_ready,
        "installedVersion": installed,
        "running": running,
        "port": port,
        "url": url,
        "platform": paths.platform(),
    }))
}

/// 初始化：确保 Node + 内核就绪。返回状态。
#[tauri::command]
pub async fn bootstrap(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<String, String> {
    
    let paths = state.paths();
    std::fs::create_dir_all(&paths.root).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&paths.log_dir).map_err(|e| e.to_string())?;
    log_to_file(paths, "bootstrap: start");

    reset_progress_floor();
    emit_bootstrap_progress(&app, 0, stage::NODE_PROBE.0, "正在挑选最快的下载线路");
    let cb = Some(Arc::new({
        let app = app.clone();
        move |progress: runtime::DownloadProgress| {
            let ratio = progress.percent.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0;
            match progress.kind.as_str() {
                // 解压/校验阶段：文案说明正在做什么，而不是继续说"下载"。
                "node-install" => emit_bootstrap_progress(
                    &app,
                    0,
                    stage::at(stage::NODE_EXTRACT, ratio),
                    "正在解压运行环境",
                ),
                _ => {
                    let detail = match progress.total {
                        Some(total) if total > 0 => format!(
                            "正在下载运行环境 {} / {}",
                            human_bytes(progress.received),
                            human_bytes(total)
                        ),
                        _ => format!("正在下载运行环境 {}", human_bytes(progress.received)),
                    };
                    emit_bootstrap_progress(&app, 0, stage::at(stage::NODE_DOWNLOAD, ratio), detail)
                }
            }
        }
    }) as Arc<dyn Fn(runtime::DownloadProgress) + Send + Sync>);
    if !runtime::node_ready(paths) {
        log_to_file(paths, "bootstrap: node not ready, installing");
        runtime::install_node_runtime(&state.client, paths, None, cb)
            .await
            .inspect_err(|e| log_to_file(paths, &format!("bootstrap: node install FAILED: {e}")))?;
    }
    emit_bootstrap_progress(&app, 0, stage::NODE_EXTRACT.1, "运行环境已就绪");
    log_to_file(paths, "bootstrap: node ready");
    emit(&app, "log", "Node 运行时就绪");
    Ok("ready".into())
}

/// 任何要用到 Node 的环节都先过这道关：缺了就自动补装。
///
/// 防呆的核心。`bootstrap` 之外的入口以前一律直接返回「Node 运行时缺失」，
/// 而用户在界面上根本没有重新引导的入口——换个位置重装一次就彻底卡死。
/// 现在统一在这里补装，并把进度照常发给引导页。
async fn ensure_node_or_reinstall(
    app: &AppHandle,
    state: &AppState,
    who: &str,
) -> Result<(), String> {
    let paths = state.paths();
    if runtime::node_ready(paths) {
        return Ok(());
    }
    log_to_file(paths, &format!("{who}: Node 不可用，自动重新安装"));
    emit(app, "log", "运行环境缺失，正在自动重新安装");
    std::fs::create_dir_all(&paths.root).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&paths.log_dir).map_err(|e| e.to_string())?;
    let cb = Some(Arc::new({
        let app = app.clone();
        move |progress: runtime::DownloadProgress| {
            let ratio = progress.percent.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0;
            let (range, text) = match progress.kind.as_str() {
                "node-install" => (stage::NODE_EXTRACT, "正在解压运行环境".to_string()),
                _ => (
                    stage::NODE_DOWNLOAD,
                    format!("正在重新下载运行环境 {}", human_bytes(progress.received)),
                ),
            };
            emit_bootstrap_progress(&app, 0, stage::at(range, ratio), text);
        }
    }) as Arc<dyn Fn(runtime::DownloadProgress) + Send + Sync>);
    runtime::ensure_node_runtime(&state.client, paths, None, cb)
        .await
        .map_err(|e| format!("运行环境自动修复失败：{e}"))?;
    log_to_file(paths, &format!("{who}: Node 已重新安装"));
    Ok(())
}

pub fn log_to_file(paths: &crate::paths::Paths, msg: &str) {    use std::io::Write;
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
    
    emit_bootstrap_progress(&app, 1, stage::KERNEL_RESOLVE.0, "正在查询最新版本");
    let paths = state.paths();
    let (version, registry) = kernel::latest_kernel_version(&state.client, paths)
        .await
        .inspect_err(|e| log_to_file(paths, &format!("install_kernel: version check FAILED: {e}")))?;
    log_to_file(paths, &format!("install_kernel: found latest v{version} @ {registry}"));
    emit_bootstrap_progress(&app, 1, stage::KERNEL_RESOLVE.1, format!("准备安装 v{version}"));
    // 换安装位置重装、上次解压中断，都会让 Node 不可用。这里自动补装，
    // 而不是报「Node 运行时缺失」——用户没有任何手段自己修。
    ensure_node_or_reinstall(&app, &state, "install_kernel").await?;
    let node = runtime::node_bin(paths);
    // pnpm 的 ndjson 输出带逐包的解析/落盘事件，据此算真实比例。
    let cb = Some(Arc::new({
        let app = app.clone();
        move |phase: kernel::InstallPhase| {
            emit_bootstrap_progress(
                &app,
                1,
                stage::at(stage::KERNEL_INSTALL, phase.ratio()),
                phase.label(),
            );
        }
    }) as Arc<dyn Fn(kernel::InstallPhase) + Send + Sync>);
    let res = kernel::install_kernel_to(&node, &paths.staging_dir, &registry, &version, cb).await;
    if let Err(e) = &res {
        log_to_file(paths, &format!("install_kernel: pnpm install FAILED: {e}"));
        return Err(e.clone());
    }
    log_to_file(paths, "install_kernel: pnpm ok, swapping");
    emit_bootstrap_progress(&app, 1, stage::KERNEL_SWAP.0, "正在启用新内核");
    kernel::swap_staging_to_overlay(paths)?;
    kernel::confirm_healthy_cleanup(paths);
    emit_bootstrap_progress(&app, 1, stage::KERNEL_EXTRAS.0, "正在配置插件市场");
    if let Err(e) = kernel::preinstall_market(paths, &registry) {
        log_to_file(paths, &format!("install_kernel: preinstall market warning: {e}"));
    }
    // 市场装好后立刻打补丁，否则首次打开市场会闪一次终端窗口。
    kernel::suppress_market_console(paths);
    if let Ok(resource_dir) = app.path().resource_dir() {
        emit_bootstrap_progress(&app, 1, stage::at(stage::KERNEL_EXTRAS, 0.6), "正在配置桌面增强组件");
        let desktop_shell = resource_dir.join("resources/dsh-desktop-shell");
        if desktop_shell.exists() {
            if let Err(e) = kernel::preinstall_desktop_shell(paths, &registry, &desktop_shell) {
                log_to_file(paths, &format!("install_kernel: preinstall desktop shell warning: {e}"));
            }
            kernel::suppress_market_console(paths);
        }
    }
    log_to_file(paths, "install_kernel: done");
    emit_bootstrap_progress(&app, 1, stage::KERNEL_EXTRAS.1, format!("内核 v{version} 已就绪"));
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
    // 启动前兜一次底：前端只在 nodeReady 为假时才调 bootstrap，
    // 而 Node 可能在那次检查之后才失效（换安装位置、目录被清理）。
    ensure_node_or_reinstall(&app, &state, "start_dsh").await?;
    emit_bootstrap_progress(&app, 2, stage::SERVICE_START.0, "正在启动本地服务");
    let mut child = crate::dsh::spawn_dsh(paths, "web-desktop")?;
    emit_bootstrap_progress(&app, 2, stage::SERVICE_READY.0, "正在等待服务响应");
    let endpoint = match wait_for_endpoint(&mut child, &state).await {
        Ok(endpoint) => endpoint,
        Err(first_error) => {
            // 服务起不来最常见的原因是内核安装不完整（下载中断、磁盘写坏）。
            // 之前这里直接失败，用户重启也无法恢复，只能重装系统。
            // 现在自动清掉损坏的内核并重装一次，再启动一次。
            let _ = child.kill();
            let _ = child.wait();
            log_to_file(paths, &format!("start_dsh: first attempt failed: {first_error}"));
            emit_bootstrap_progress(&app, 2, stage::SERVICE_START.0, "服务启动失败，正在自动修复内核");
            emit(&app, "log", "检测到内核可能损坏，正在自动重新安装");
            kernel::purge_kernel(paths)?;
            reset_progress_floor();
            install_kernel(app.clone(), state.clone()).await.map_err(|e| {
                format!("自动修复失败：{e}（原始错误：{first_error}）")
            })?;
            emit_bootstrap_progress(&app, 2, stage::SERVICE_START.0, "修复完成，正在重新启动服务");
            let mut retry = crate::dsh::spawn_dsh(paths, "web-desktop")?;
            match wait_for_endpoint(&mut retry, &state).await {
                Ok(endpoint) => {
                    child = retry;
                    endpoint
                }
                Err(second_error) => {
                    let _ = retry.kill();
                    let _ = retry.wait();
                    return Err(format!(
                        "重新安装内核后仍无法启动服务：{second_error}。请查看 logs/dsh.log"
                    ));
                }
            }
        }
    };
    emit_bootstrap_progress(&app, 2, stage::SERVICE_READY.1, "服务已就绪");
    let port = endpoint.port;
    // 凭证由代理持有，前端走代理端口，不直连内核。
    let authenticated = state.proxy.adopt(&endpoint.url).await;
    let proxy_port = state.proxy.serve().await?;
    let url = state
        .proxy
        .client_url()
        .ok_or_else(|| "代理地址不可用".to_string())?;
    log_to_file(
        paths,
        &format!(
            "start_dsh: 就绪 内核端口={port} 代理端口={proxy_port} 带鉴权参数={} 已换取凭证={authenticated}",
            endpoint.url.contains('?')
        ),
    );
    *state.dsh.lock().await = Some(DshRuntime { child, port, url });
    emit(&app, "log", format!("dsh 服务已就绪，端口 {}", port));
    Ok(port)
}

/// 等待 dsh 打印服务地址，整条带回（含鉴权令牌）。
async fn wait_for_endpoint(
    child: &mut std::process::Child,
    state: &AppState,
) -> Result<crate::dsh::DshEndpoint, String> {
    let paths = state.paths();
    // dsh 启动失败时错误只在 stderr；单独抽干并落盘，避免管道写满且丢失原因。
    if let Some(mut se) = child.stderr.take() {
        let log_dir = paths.log_dir.clone();
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Write};
            let reader = BufReader::new(&mut se);
            for line in reader.lines().map_while(Result::ok) {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log_dir.join("dsh.log"))
                {
                    let _ = writeln!(f, "{line}");
                }
            }
        });
    }
    let stdout = child.stdout.take();
    if let Some(mut so) = stdout {
        use std::io::{BufRead, BufReader};
        let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::dsh::DshEndpoint>(4);
        std::thread::spawn(move || {
            let reader = BufReader::new(&mut so);
            for l in reader.lines() {
                let Ok(l) = l else { break };
                if let Some(endpoint) = crate::dsh::parse_endpoint_from_line(&l) {
                    let _ = tx.blocking_send(endpoint);
                    break;
                }
            }
        });
        if let Some(endpoint) = rx.recv().await {
            return Ok(endpoint);
        }
    }
    // fallback：轮询默认端口。
    //
    // 走到这里说明没能捕获到内核打印的 URL，也就拿不到鉴权令牌。新版内核会因此
    // 返回 401，界面显示 "authentication required"。所以这里只在探测到服务
    // 确实无需鉴权（根路径直接 200）时才认可，否则宁可报错让上层重启。
    for _ in 0..80 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "dsh 启动失败（退出码 {:?}），详情见 logs/dsh.log",
                status.code()
            ));
        }
        if is_http_ok(&state.client, 3080).await {
            return Ok(crate::dsh::DshEndpoint {
                url: "http://127.0.0.1:3080/".to_string(),
                port: 3080,
            });
        }
    }
    Err("dsh 未在预期时间内就绪".to_string())
}

/// 探测端口是否返回 HTTP 200
/// 探测端口上是否有 dsh 在监听。
///
/// 401 也算就绪：新版内核给 Web 界面加了鉴权，无凭证请求一律 401。
/// 只认 2xx/3xx 会把"起来了但要鉴权"误判成"没起来"，表现为升级后报
/// "DSH 启动失败"、重启软件又退回首次安装流程重装内核，循环卡死。
///
/// 这里要回答的是"端口后面有没有服务"，不是"我有没有权限"——
/// 能给出 HTTP 状态码就说明服务活着。
/// 这个状态码是否说明"服务已经起来了"。
///
/// 401 必须算通过。新版内核给 Web 界面加了鉴权，不带令牌访问 `/` 就返回 401
/// —— 那恰恰证明它在监听并且能处理请求。把 401 当成失败会连锁出一串故障：
/// 升级后报"DSH 启动失败"，用户重开软件被打回首次安装流程，装完内核又
/// 因为同一个判断再次失败，形成死循环。
fn status_means_serving(status: reqwest::StatusCode) -> bool {
    status.is_success() || status.is_redirection() || status == reqwest::StatusCode::UNAUTHORIZED
}

pub async fn is_http_ok(client: &reqwest::Client, port: u16) -> bool {
    let url = format!("http://127.0.0.1:{}/", port);
    match client.get(&url).timeout(std::time::Duration::from_secs(1)).send().await {
        Ok(r) => status_means_serving(r.status()),
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
    apply_update_impl(&app, &state).await
}

/// 供托盘/后台更新调用的实现
pub async fn apply_update_impl(app: &AppHandle, state: &Arc<AppState>) -> Result<String, String> {
    {
        let mut u = state.updating.lock().await;
        if *u {
            return Err("升级已在进行中".to_string());
        }
        *u = true;
    }
    let res = do_apply_update(app, state).await;
    *state.updating.lock().await = false;
    res
}

async fn do_apply_update(app: &AppHandle, state: &AppState) -> Result<String, String> {
    let paths = state.paths();
    let (version, registry) = kernel::latest_kernel_version(&state.client, paths).await?;
    ensure_node_or_reinstall(app, state, "apply_update").await?;
    let node = runtime::node_bin(paths);
    // 停 dsh 再换
    {
        let mut g = state.dsh.lock().await;
        if let Some(d) = g.as_mut() {
            let _ = d.child.kill();
            let _ = d.child.wait();
        }
        *g = None;
    }
    // 旧端口的凭证立刻作废，避免代理把请求打到已经退出的进程。
    state.proxy.clear();
    let cb = make_log_cb(app.clone());
    kernel::install_kernel_to(&node, &paths.staging_dir, &registry, &version, cb).await?;
    kernel::swap_staging_to_overlay(paths)?;
    // 安装的进度只走到 98%；剩下的重启由前端接手，这里先把文字交代清楚，
    // 免得按钮停在"更新中 98%"却看不出在等什么。
    emit_kernel_progress(app, 98.0, "正在重启服务");
    emit(app, "log", format!("升级完成，已切换到 v{version}"));
    Ok(version)
}

/// 发一条内核升级进度。百分比用于按钮进度条，文字用于按钮标签。
fn emit_kernel_progress(app: &AppHandle, percent: f64, label: &str) {
    let _ = app.emit(
        "kernel-update-progress",
        serde_json::json!({ "percent": percent, "label": label }),
    );
}

/// 供托盘/后台调用：重启 dsh 服务（升级后重新拉起）
pub async fn restart_dsh_impl(app: &AppHandle, state: &Arc<AppState>) -> Result<u16, String> {
    let paths = state.paths();
    // profile 必须与 start_dsh 一致：桌面端用 web-desktop，它才会加载
    // dsh-desktop-shell 的布局补丁。这里曾误写成 web，升级重启后界面会退回
    // 未定制的样子。
    // 升级后重启同样要兜底：升级过程可能刚好把 Node 换到一半。
    ensure_node_or_reinstall(app, state, "restart_dsh").await?;
    let mut child = crate::dsh::spawn_dsh(paths, "web-desktop")?;
    let endpoint = wait_for_endpoint(&mut child, state.as_ref()).await?;
    let port = endpoint.port;
    // 换新令牌换新凭证；前端地址不变（始终是同源的 dsh://），
    // 只需刷新 iframe 让它重新走一遍代理。
    let authenticated = state.proxy.adopt(&endpoint.url).await;
    let proxy_port = state.proxy.serve().await?;
    let url = state
        .proxy
        .client_url()
        .ok_or_else(|| "代理地址不可用".to_string())?;
    log_to_file(
        paths,
        &format!("restart_dsh: 就绪 内核端口={port} 代理端口={proxy_port} 已换取凭证={authenticated}"),
    );
    *state.dsh.lock().await = Some(DshRuntime { child, port, url: url.clone() });
    emit(app, "log", format!("dsh 服务已重启，端口 {}", port));

    // 通知前端重启完成（隐藏加载遮罩）。地址恒定，但必须刷新 iframe：
    // 代理背后的端口和凭证都换了，旧页面里的连接已经失效。
    let _ = app.emit("dsh-restarted", serde_json::json!({
        "port": port,
        "url": url,
        "healthy": true
    }));

    Ok(port)
}

/// 重启 dsh 服务（前端调用，插件市场更新后需要重启生效）
#[tauri::command]
pub async fn restart_dsh(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<u16, String> {
    // 先停掉旧进程
    {
        let mut g = state.dsh.lock().await;
        if let Some(d) = g.as_mut() {
            let _ = d.child.kill();
            let _ = d.child.wait();
        }
        *g = None;
    }
    restart_dsh_impl(&app, &state).await
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

/// 读取设置（前端/插件查询）。开机自启和防休眠回报真实的系统状态，
/// 避免设置文件与系统实际状态不一致时显示错误的开关位置。
#[tauri::command]
pub async fn get_settings(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let paths = state.paths();
    let mut s = crate::paths::Settings::load(paths);
    let actual_auto_launch = crate::platform::auto_launch_enabled();
    if s.auto_launch != actual_auto_launch {
        s.auto_launch = actual_auto_launch;
        s.save(paths);
    }
    Ok(serde_json::json!({
        "autoCheckUpdates": s.auto_check_updates,
        "confirmExit": s.confirm_exit,
        "autoLaunch": actual_auto_launch,
        "preventSleep": crate::platform::prevent_sleep_active(),
        "desktopNotifications": s.desktop_notifications,
        "notificationTurnComplete": s.notification_turn_complete,
        "notificationTurnFailed": s.notification_turn_failed,
        "notificationJobComplete": s.notification_job_complete,
        "notificationJobFailed": s.notification_job_failed,
        "skippedKernelVersion": s.skipped_kernel_version,
    }))
}

/// 更新设置（前端/插件写入）。开关写入后立刻作用到系统，
/// 系统调用失败时回滚该项并返回错误，防止出现"开关已开但功能没生效"。
#[tauri::command]
pub async fn set_settings(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    auto_check_updates: Option<bool>,
    confirm_exit: Option<bool>,
    desktop_notifications: Option<bool>,
    auto_launch: Option<bool>,
    prevent_sleep: Option<bool>,
    notification_turn_complete: Option<bool>,
    notification_turn_failed: Option<bool>,
    notification_job_complete: Option<bool>,
    notification_job_failed: Option<bool>,
) -> Result<(), String> {
    let paths = state.paths();
    let mut s = crate::paths::Settings::load(paths);
    if let Some(v) = auto_check_updates {
        s.auto_check_updates = v;
    }
    if let Some(v) = confirm_exit {
        s.confirm_exit = v;
    }
    if let Some(v) = desktop_notifications {
        s.desktop_notifications = v;
    }
    if let Some(v) = auto_launch {
        crate::platform::apply_auto_launch(&app, v)?;
        s.auto_launch = v;
    }
    if let Some(v) = prevent_sleep {
        crate::platform::apply_prevent_sleep(v)?;
        s.prevent_sleep = v;
    }
    if let Some(v) = notification_turn_complete { s.notification_turn_complete = v; }
    if let Some(v) = notification_turn_failed { s.notification_turn_failed = v; }
    if let Some(v) = notification_job_complete { s.notification_job_complete = v; }
    if let Some(v) = notification_job_failed { s.notification_job_failed = v; }
    s.save(paths);
    Ok(())
}

/// 发送一条测试通知，用于确认桌面通知开关真正生效。
///
/// 必须把系统的真实结果带回前端：以前这里无论投递成功还是被系统拒绝都返回
/// Ok(true)，用户看不到通知也看不到原因，只能猜。
#[tauri::command]
pub async fn send_test_notification(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    let s = crate::paths::Settings::load(state.paths());
    if !s.desktop_notifications {
        return Ok(false);
    }
    crate::platform::notify_detailed(
        &app,
        crate::platform::NotifyKind::Service,
        "DeepSeek Harness",
        "桌面通知已开启，任务完成和失败时会在这里提醒你。",
    )
}

/// 打开系统的通知设置页。
///
/// macOS 把「拒绝通知」永久记在系统里，应用再调 requestAuthorization 也只会
/// 立刻拿到拒绝、不再弹窗。唯一的出路是让用户去系统设置里手动打开，所以这里
/// 直接把那一页拉起来，省得让用户自己找。
#[tauri::command]
pub async fn open_notification_settings(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let target = "x-apple.systempreferences:com.apple.Notifications-Settings.extension";
    #[cfg(target_os = "windows")]
    let target = "ms-settings:notifications";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let target = "";

    if target.is_empty() {
        return Err("当前平台没有可打开的通知设置页".to_string());
    }
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(target, None::<&str>)
        .map_err(|error| format!("无法打开系统通知设置：{error}"))
}

/// 校验「在本地打开」的目标：必须是已存在的绝对路径文件夹。
///
/// 不 canonicalize。Windows 上 canonicalize 会加上 `\\?\` 前缀，
/// 交给 ShellExecute 时有的资源管理器版本打不开。
pub(crate) fn validate_open_directory(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("路径不能为空".to_string());
    }
    let candidate = PathBuf::from(trimmed);
    if !candidate.is_absolute() {
        return Err("只能打开绝对路径的文件夹".to_string());
    }
    let metadata = std::fs::metadata(&candidate).map_err(|_| "目录不存在".to_string())?;
    if !metadata.is_dir() {
        return Err("目标不是文件夹".to_string());
    }
    Ok(candidate)
}

/// 在资源管理器 / Finder 里打开一个工作区目录。
///
/// Windows 上官方内核会从隐藏的 Node 进程里跑 `powershell Invoke-Item`，
/// 进程带着 CREATE_NO_WINDOW | DETACHED_PROCESS，资源管理器经常不出现。
/// 这里改由 GUI 宿主用 opener 的 detached ShellExecute 打开。
#[tauri::command]
pub async fn open_path(app: AppHandle, path: String) -> Result<(), String> {
    let directory = validate_open_directory(&path)?;
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(directory.to_string_lossy().as_ref(), None::<&str>)
        .map_err(|error| format!("无法在本地打开：{error}"))
}

/// DSH Web/插件发送桌面通知的入口。总开关或分类关闭时返回 false。
#[tauri::command]
pub async fn notify_desktop(
    app: AppHandle,
    kind: String,
    title: String,
    body: String,
) -> Result<bool, String> {
    let Some(kind) = crate::platform::NotifyKind::parse(&kind) else {
        return Err(format!("未知的通知类别: {kind}"));
    };
    Ok(crate::platform::notify(&app, kind, &title, &body))
}


/// 回退到上一版本
#[tauri::command]
pub async fn rollback(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    rollback_impl(&state).await
}

/// 供托盘菜单调用的回退实现（无 tauri State 包装）
pub async fn rollback_impl(state: &Arc<AppState>) -> Result<(), String> {
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


// ---------- 问题反馈 ----------

/// 反馈正文里附带的日志上限。反馈正文有长度限制，
/// 而且过长的日志对排查没有帮助——真正有用的是最后那几十行。
const FEEDBACK_LOG_BUDGET: usize = 8 * 1024;
/// 各日志文件截取的尾部行数。
const APP_LOG_TAIL_LINES: usize = 120;
const DSH_LOG_TAIL_LINES: usize = 60;
/// Issue 标题长度上限（超出部分截断并加省略号）。
const FEEDBACK_TITLE_LIMIT: usize = 50;

/// 从用户描述生成 Issue 标题：取首个非空行，压缩空白后截断。
fn feedback_title(description: &str) -> String {
    let first_line = description
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    // 连续空白压成单个空格，避免标题里出现制表符或多余空格。
    let compact = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return "[反馈] 用户反馈（无描述）".to_string();
    }
    // 按字符而非字节截断，否则会切坏中文。
    if compact.chars().count() > FEEDBACK_TITLE_LIMIT {
        let truncated: String = compact.chars().take(FEEDBACK_TITLE_LIMIT).collect();
        format!("[反馈] {truncated}…")
    } else {
        format!("[反馈] {compact}")
    }
}

/// 按关键词粗分反馈类型，仅用于正文展示，不作为标签。
fn feedback_category(description: &str) -> &'static str {
    const BUG_HINTS: [&str; 10] = [
        "崩溃", "闪退", "报错", "失败", "无法", "卡住", "没反应", "不见了", "错误", "bug",
    ];
    const IDEA_HINTS: [&str; 6] = ["希望", "建议", "能不能", "可不可以", "增加", "支持"];
    let lower = description.to_lowercase();
    if BUG_HINTS.iter().any(|hint| lower.contains(hint)) {
        "疑似缺陷"
    } else if IDEA_HINTS.iter().any(|hint| lower.contains(hint)) {
        "功能建议"
    } else {
        "使用反馈"
    }
}

/// 读取单个日志文件的尾部若干行。文件不存在或读取失败时返回 `None`。
fn tail_log(path: &std::path::Path, lines: usize) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let collected: Vec<&str> = text.lines().collect();
    let start = collected.len().saturating_sub(lines);
    let tail = collected[start..].join("\n");
    if tail.trim().is_empty() {
        None
    } else {
        Some(tail)
    }
}

/// 收集运行日志尾部，总量不超过 `FEEDBACK_LOG_BUDGET`。
/// 超出预算时优先保留更靠后的内容（最近发生的事更可能是问题原因）。
fn collect_logs(paths: &crate::paths::Paths) -> Vec<(String, String)> {
    const TRUNCATED_MARKER: &str = "…（已截断更早的内容）\n";
    let sources = [
        ("app.log", APP_LOG_TAIL_LINES),
        ("dsh.log", DSH_LOG_TAIL_LINES),
        ("tray.log", 20),
    ];
    let mut budget = FEEDBACK_LOG_BUDGET;
    let mut out = Vec::new();
    for (name, lines) in sources {
        // 剩余预算装不下标记本身时就不必再截了，后面的文件一并跳过。
        if budget <= TRUNCATED_MARKER.len() {
            break;
        }
        let Some(mut body) = tail_log(&paths.log_dir.join(name), lines) else {
            continue;
        };
        if body.len() > budget {
            // 标记也要占预算，先扣掉再决定保留多少正文。
            let keep = budget - TRUNCATED_MARKER.len();
            let cut = body.len() - keep;
            // 从切点往后找行边界，既不切断 UTF-8 字符也不留半行。
            let boundary = body[cut..]
                .find('\n')
                .map(|offset| cut + offset + 1)
                .unwrap_or(body.len());
            body = match body.get(boundary..) {
                Some(rest) if !rest.trim().is_empty() => format!("{TRUNCATED_MARKER}{rest}"),
                _ => continue,
            };
        }
        budget = budget.saturating_sub(body.len());
        if !body.trim().is_empty() {
            out.push((name.to_string(), body));
        }
    }
    out
}

/// 本地时间字符串（`YYYY-MM-DD HH:MM`）。
/// 项目没有 chrono 依赖，这里按 UTC 秒数手工换算，只用于人读。
fn local_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    // 反馈主要来自国内用户，统一按 UTC+8 展示并标注时区。
    let shifted = secs + 8 * 3600;
    let days = shifted / 86_400;
    let time_of_day = shifted % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} (UTC+8)",
        time_of_day / 3600,
        (time_of_day % 3600) / 60
    )
}

/// 天数转公历日期（Howard Hinnant 的 civil_from_days 算法）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}


/// 详细的操作系统描述，例如 `macOS 26.5.2 (25F84, arm64)`
/// 或 `Windows 11 专业版 24H2 (26100.4061, aarch64)`。
///
/// 只有版本号不够定位问题：同一个 Windows 11 在 22H2 和 24H2 上
/// WebView2 行为就有差异，构建号也常常是复现的关键。
fn os_description() -> String {
    let arch = std::env::consts::ARCH;
    match platform_release() {
        Some(release) => format!("{release} ({arch})"),
        None => format!("{} ({arch})", std::env::consts::OS),
    }
}

/// macOS: 从 `sw_vers` 读产品名、版本号与构建号。
#[cfg(target_os = "macos")]
fn platform_release() -> Option<String> {
    let read = |key: &str| -> Option<String> {
        let out = std::process::Command::new("/usr/bin/sw_vers").arg(key).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!text.is_empty()).then_some(text)
    };
    let name = read("-productName").unwrap_or_else(|| "macOS".to_string());
    let version = read("-productVersion")?;
    Some(match read("-buildVersion") {
        Some(build) => format!("{name} {version} ({build})"),
        None => format!("{name} {version}"),
    })
}

/// Windows: 读注册表里的产品名、功能更新版本（24H2 这类）与完整构建号。
#[cfg(target_os = "windows")]
fn platform_release() -> Option<String> {
    const KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let read = |name: &str| -> Option<String> {
        let mut cmd = std::process::Command::new("reg.exe");
        cmd.args(["query", KEY, "/v", name]);
        crate::dsh::suppress_console(&mut cmd);
        let out = cmd.output().ok()?;
        if !out.status.success() {
            return None;
        }
        // reg.exe 输出形如 `    ProductName    REG_SZ    Windows 11 Pro`，
        // 值本身可能含空格，所以只按类型标记切一次。
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines()
            .find(|line| line.trim_start().starts_with(name))
            .and_then(|line| line.split_once("REG_SZ").or_else(|| line.split_once("REG_DWORD")))
            .map(|(_, value)| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };

    let product = read("ProductName")?;
    let mut out = product;
    if let Some(display) = read("DisplayVersion") {
        out.push(' ');
        out.push_str(&display);
    }
    if let Some(build) = read("CurrentBuildNumber") {
        // UBR 是 REG_DWORD，reg.exe 以 0x 十六进制输出，需要转成十进制补丁号。
        let patch = read("UBR")
            .and_then(|raw| u32::from_str_radix(raw.trim_start_matches("0x"), 16).ok());
        match patch {
            Some(patch) => out.push_str(&format!(" ({build}.{patch})")),
            None => out.push_str(&format!(" ({build})")),
        }
    }
    Some(out)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_release() -> Option<String> {
    None
}

/// 组装 Issue 正文：环境信息表格 + 用户描述 + 可选的折叠日志。
fn feedback_body(
    description: &str,
    environment: &[(&str, String)],
    logs: &[(String, String)],
) -> String {
    let mut out = String::from("## 环境信息\n\n| 项目 | 内容 |\n| --- | --- |\n");
    for (key, value) in environment {
        out.push_str(&format!("| {key} | {value} |\n"));
    }
    out.push_str("\n## 用户描述\n\n");
    let trimmed = description.trim();
    if trimmed.is_empty() {
        out.push_str("_用户未填写描述_\n");
    } else {
        out.push_str(trimmed);
        out.push('\n');
    }
    if !logs.is_empty() {
        out.push_str("\n## 运行日志\n\n");
        for (name, body) in logs {
            out.push_str(&format!(
                "<details>\n<summary>{name}</summary>\n\n```\n{body}\n```\n\n</details>\n\n"
            ));
        }
    }
    out.push_str("\n---\n_本 Issue 由客户端「反馈问题」自动提交_\n");
    out
}

/// 打开发起反馈的页面（GitHub 新建 Issue，标题与正文已预填）。
///
/// 用户只需填写描述；环境信息、分类与时间由这里采集补全。
/// 返回打开的链接。
///
/// 为什么不再直接建 Issue：客户端是开源的，任何埋进去的凭据都会随源码
/// 公开。改为让用户在浏览器里用自己的账号提交，凭据不经过我们。
#[tauri::command]
pub async fn submit_feedback(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    description: String,
    include_logs: bool,
) -> Result<String, String> {
    let paths = state.paths();
    if description.trim().is_empty() {
        return Err("请先填写反馈内容".to_string());
    }

    let dsh_guard = state.dsh.lock().await;
    let running = dsh_guard.is_some();
    let port = dsh_guard.as_ref().map(|d| d.port);
    drop(dsh_guard);

    let environment = vec![
        ("提交时间", local_timestamp()),
        ("反馈分类", feedback_category(&description).to_string()),
        ("客户端版本", env!("CARGO_PKG_VERSION").to_string()),
        (
            "内核版本",
            kernel::active_kernel_version(paths).unwrap_or_else(|| "未安装".to_string()),
        ),
        ("操作系统", os_description()),
        (
            "服务状态",
            match (running, port) {
                (true, Some(port)) => format!("运行中，端口 {port}"),
                (true, None) => "运行中".to_string(),
                _ => "未运行".to_string(),
            },
        ),
        (
            "Node 运行时",
            if runtime::node_ready(paths) { "已就绪" } else { "未就绪" }.to_string(),
        ),
    ];

    let title = feedback_title(&description);
    // 日志不进 URL：带完整日志的正文会超过 GitHub 的 URL 长度限制，
    // 而少量诊断信息（系统/版本）已足够复现。这里显式传空日志段，
    // 让正文只剩环境表和用户描述，总长度可控。
    let body = feedback_body(&description, &environment, &[]);
    let logs_hint = if include_logs {
        "（需要时开发者会再请你手动补粘运行日志）"
    } else {
        ""
    };
    let url = build_issue_url(&title, &body, logs_hint);
    log_to_file(paths, &format!("submit_feedback: 打开浏览器 {url_len}", url_len = url.len()));
    open_url_in_browser(&app, &url)
}

/// 用 GET 参数把标题和正文拼成新建 Issue 的预填地址。
fn build_issue_url(title: &str, body: &str, hint: &str) -> String {
    let mut q = String::new();
    q.push_str(&format!("title={}", urlencode(title)));
    let mut full_body = body.to_string();
    if !hint.is_empty() {
        full_body.push_str("\n\n");
        full_body.push_str(hint);
    }
    q.push_str(&format!("&body={}", urlencode(&full_body)));
    format!("{}?{}", crate::mirrors::update_urls::issues_new(), q)
}

/// 简易 URL 编码：空格转加号、UTF-8 百分号编码。不依赖 percent-encoding crate，
/// 因为它的返回值是 `&str` 生命周期，链接到栈上临时值会被借用规则卡住。
fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 用默认浏览器打开外部 URL（不经 Tauri 的 opener 白名单，域名由我们写出）。
fn open_url_in_browser(app: &AppHandle, url: &str) -> Result<String, String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("无法打开浏览器: {e}"))?;
    Ok(url.to_string())
}


// ---------- helpers ----------

/// 把「检查下载并流式校验 sha256 后的」安装包落到目标路径并返回摘要。
/// 共享于两个入口：现状那条（顺手落包更新）+ 下载完毕发生在 elsewhere。

fn make_dl_cb(app: AppHandle, _kind: &'static str) -> DynProgress {
    Some(Arc::new(move |p| {
        let _ = app.emit("download-progress", p);
    }))
}

/// 升级内核时的进度回调：把 pnpm 的真实进度转成一行日志发给前端。
///
/// 升级走的是「检查更新」对话框，那里只有日志区没有进度条，
/// 所以这里报文案而不是百分比。
/// 内核升级期间的进度回调。
///
/// 同时发两个事件：`kernel-log` 是给日志面板看的文字，
/// `kernel-update-progress` 带百分比，更新按钮要拿它当进度条。
/// 只发文字的话按钮只能显示一个静态的"更新中"，用户无法判断是否卡死。
fn make_log_cb(app: AppHandle) -> Option<Arc<dyn Fn(kernel::InstallPhase) + Send + Sync>> {
    Some(Arc::new(move |phase: kernel::InstallPhase| {
        let _ = app.emit("kernel-log", phase.label());
        let _ = app.emit(
            "kernel-update-progress",
            serde_json::json!({
                "percent": phase.ratio() * 100.0,
                "label": phase.label(),
            }),
        );
    }))
}

fn emit<E: serde::Serialize + Clone>(app: &AppHandle, event: &str, payload: E) {
    let _ = app.emit(event, payload);
}

/// 客户端更新检查
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct ClientUpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub download_url: Option<String>,
    pub version_sha256: Option<String>,
    pub release_notes: Option<String>,
}

#[tauri::command]
pub async fn check_client_update(state: State<'_, Arc<AppState>>) -> Result<ClientUpdateInfo, String> {
    fetch_client_update_info(&state.client).await
}

/// 拉取版本清单并回一次完整检查结果。被命令与 lib.rs 的后台任务共用。
///
/// 不查 GitHub API：api.github.com 国内连不稳，匿名限流按出口 IP 计；
/// 清单是一个静态文件，能被镜像直接缓存。
pub async fn fetch_client_update_info(client: &reqwest::Client) -> Result<ClientUpdateInfo, String> {
    use crate::mirrors::{github_update_mirrors, pick_fastest, speed_test, update_urls};

    let current_version = env!("CARGO_PKG_VERSION");

    // 并发测速各镜像前缀，选最快的、能稳定拿到清单的那个。
    let mirrors = github_update_mirrors();
    let results = speed_test(client, &mirrors).await;
    let fastest = pick_fastest(&results)
        .ok_or_else(|| "没有可用的更新检查源，请检查网络".to_string())?;

    let manifest_url = format!("{}{}", fastest.base_url, update_urls::version_json());
    let response = client
        .get(manifest_url)
        .header("cache-control", "no-cache")
        .send()
        .await
        .map_err(|e| format!("无法连接更新检查源: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("读取版本清单失败: HTTP {}", response.status()));
    }

    let manifest: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("解析版本清单失败: {e}"))?;

    let latest_version = manifest["version"]
        .as_str()
        .unwrap_or(current_version)
        .trim_start_matches('v');

    let release_notes = manifest["notes"].as_str();

    // 平台文件名 → 直接给出可以用的下载地址、大小和校验和。
    let platform = std::env::consts::OS;
    let platform_key = match platform {
        "macos" => "macos",
        "windows" => "windows",
        _ => "",
    };

    let download_url = manifest
        .get("files")
        .and_then(|v| v.get(platform_key))
        .and_then(|f| f["url"].as_str())
        .map(|s| s.to_string());

    let version_sha256 = manifest
        .get("files")
        .and_then(|v| v.get(platform_key))
        .and_then(|f| f["sha256"].as_str())
        .map(|s| s.to_string());

    let update_available = version_compare(current_version, latest_version);

    Ok(ClientUpdateInfo {
        current_version: current_version.to_string(),
        latest_version: latest_version.to_string(),
        update_available,
        download_url,
        version_sha256,
        release_notes: release_notes.map(|s| s.to_string()),
    })
}

/// 简单的版本比较（语义化版本）
fn version_compare(current: &str, latest: &str) -> bool {
    let parse_version = |v: &str| -> Vec<u32> {
        v.split('.')
            .filter_map(|s| s.parse::<u32>().ok())
            .collect()
    };

    let current_parts = parse_version(current);
    let latest_parts = parse_version(latest);

    for i in 0..3 {
        let curr = current_parts.get(i).copied().unwrap_or(0);
        let late = latest_parts.get(i).copied().unwrap_or(0);
        if late > curr {
            return true;
        } else if late < curr {
            return false;
        }
    }
    false
}

/// 按 `update.json` 里记录的平台下载安装包，全程流式校验 sha256 并交给安装器。
#[tauri::command]
pub async fn download_client_update(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<String, String> {
    use crate::downloader::download_with_sha256;

    let info = check_client_update(state.clone()).await?;
    let download_url = info
        .download_url
        .ok_or_else(|| "没有可用的安装包下载地址".to_string())?;

    let expected_sha = info.version_sha256.ok_or_else(|| {
        "版本清单缺少安装包校验和，无法安全下载".to_string()
    })?;

    let paths = state.paths();
    let download_dir = app
        .path()
        .download_dir()
        .unwrap_or_else(|_| dirs::download_dir().unwrap_or_else(|| paths.root.join("downloads")));
    std::fs::create_dir_all(&download_dir).map_err(|e| format!("创建下载目录失败: {e}"))?;

    let file_name = download_url
        .split('/')
        .next_back()
        .filter(|name| !name.is_empty())
        .unwrap_or("DeepSeekHarness-setup");
    let safe_name = std::path::Path::new(file_name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("DeepSeekHarness-setup");
    let file_path = download_dir.join(safe_name);

    let emitted_progress = Arc::new({
        let app = app.clone();
        move |p: crate::downloader::DownloadProgress| {
            let _ = app.emit("client-update-progress", serde_json::json!({
                "received": p.received,
                "total": p.total,
                "percent": p.percent,
            }));
        }
    });
    let log = log_callback(paths, "client-update");

    let sha = download_with_sha256(
        &state.client,
        &download_url,
        &file_path,
        Some(emitted_progress),
        "client-update",
        log,
        Some(&expected_sha),
    )
    .await?;

    log_to_file(paths, &format!("下载完成 {}（sha256 {}）", file_path.display(), sha));

    // 打开文件管理器选中下载好的安装包，让用户直接点下一步。
    if cfg!(target_os = "macos") {
        let _ = std::process::Command::new("open").arg("-R").arg(&file_path).spawn();
    } else if cfg!(target_os = "windows") {
        let mut cmd = std::process::Command::new("explorer");
        crate::dsh::suppress_console(&mut cmd);
        let _ = cmd.arg("/select,").arg(&file_path).spawn();
    }

    Ok(file_path.to_string_lossy().into_owned())
}

/// 把下载动作里收集成的日志写入 app.log，方便排障。
fn log_callback(paths: &crate::paths::Paths, tag: &'static str) -> crate::downloader::LogFn {
    let paths = paths.clone();
    Some(Arc::new(move |msg: String| {
        crate::commands::log_to_file(&paths, &format!("[{tag}] {msg}"));
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_directory_rejects_empty_and_relative() {
        assert!(validate_open_directory("").is_err());
        assert!(validate_open_directory("   ").is_err());
        assert!(validate_open_directory("relative/folder").is_err());
    }

    #[test]
    fn open_directory_rejects_missing_and_file() {
        let dir = scratch("open-path");
        assert!(validate_open_directory(dir.join("missing").to_str().unwrap()).is_err());
        let file = dir.join("note.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(validate_open_directory(file.to_str().unwrap()).is_err());
        assert!(validate_open_directory(dir.to_str().unwrap()).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dsh-fb-{name}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn paths_with_logs(dir: &std::path::Path) -> crate::paths::Paths {
        crate::paths::Paths {
            agent_dir: dir.join("agent"),
            staging_dir: dir.join("staging"),
            backup_dir: dir.join("previous"),
            node_dir: dir.join("runtime"),
            settings_file: dir.join("settings.json"),
            log_dir: dir.join("logs"),
            root: dir.to_path_buf(),
        }
    }

    #[test]
    fn title_uses_first_non_empty_line() {
        assert_eq!(feedback_title("\n\n  点击设置没反应  \n后面还有"), "[反馈] 点击设置没反应");
    }

    #[test]
    fn title_collapses_inner_whitespace() {
        assert_eq!(feedback_title("窗口\t\t按钮   失效"), "[反馈] 窗口 按钮 失效");
    }

    #[test]
    fn title_falls_back_when_description_is_blank() {
        assert_eq!(feedback_title("   \n\t "), "[反馈] 用户反馈（无描述）");
    }

    #[test]
    fn title_truncates_on_character_boundary() {
        // 全中文超长输入：按字符截断，不能切坏 UTF-8。
        let long = "问".repeat(120);
        let title = feedback_title(&long);
        assert!(title.starts_with("[反馈] "));
        assert!(title.ends_with('…'));
        assert_eq!(title.chars().filter(|c| *c == '问').count(), FEEDBACK_TITLE_LIMIT);
    }

    #[test]
    fn category_detects_bug_idea_and_default() {
        assert_eq!(feedback_category("软件闪退了"), "疑似缺陷");
        assert_eq!(feedback_category("希望增加暗色主题"), "功能建议");
        assert_eq!(feedback_category("整体用起来还不错"), "使用反馈");
    }

    #[test]
    fn missing_log_files_are_skipped_without_panicking() {
        let dir = scratch("nolog");
        let paths = paths_with_logs(&dir);
        assert!(collect_logs(&paths).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_logs_keeps_only_the_tail() {
        let dir = scratch("tail");
        let paths = paths_with_logs(&dir);
        std::fs::create_dir_all(&paths.log_dir).unwrap();
        let body: String = (0..500).map(|i| format!("line-{i}\n")).collect();
        std::fs::write(paths.log_dir.join("app.log"), body).unwrap();

        let logs = collect_logs(&paths);
        assert_eq!(logs.len(), 1);
        let (name, text) = &logs[0];
        assert_eq!(name, "app.log");
        assert_eq!(text.lines().count(), APP_LOG_TAIL_LINES);
        // 保留的必须是结尾，而不是开头。
        assert!(text.contains("line-499"));
        assert!(!text.contains("line-0\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_logs_respects_total_budget() {
        let dir = scratch("budget");
        let paths = paths_with_logs(&dir);
        std::fs::create_dir_all(&paths.log_dir).unwrap();
        // 每行都很长，120 行远超 8KB 预算。
        let fat: String = (0..120).map(|i| format!("{i}: {}\n", "x".repeat(200))).collect();
        std::fs::write(paths.log_dir.join("app.log"), &fat).unwrap();
        std::fs::write(paths.log_dir.join("dsh.log"), &fat).unwrap();

        let logs = collect_logs(&paths);
        let total: usize = logs.iter().map(|(_, body)| body.len()).sum();
        assert!(total <= FEEDBACK_LOG_BUDGET, "日志总量 {total} 应不超过预算");
        assert!(logs.iter().any(|(_, body)| body.contains("已截断更早的内容")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn body_contains_environment_description_and_logs() {
        let env = vec![("客户端版本", "1.0.0".to_string())];
        let logs = vec![("app.log".to_string(), "boot ok".to_string())];
        let body = feedback_body("设置打不开", &env, &logs);
        assert!(body.contains("| 客户端版本 | 1.0.0 |"));
        assert!(body.contains("设置打不开"));
        assert!(body.contains("<summary>app.log</summary>"));
        assert!(body.contains("boot ok"));
    }

    #[test]
    fn body_omits_log_section_when_user_opts_out() {
        let env = vec![("客户端版本", "1.0.0".to_string())];
        let body = feedback_body("有个建议", &env, &[]);
        assert!(!body.contains("## 运行日志"));
        assert!(!body.contains("<details>"));
    }

    #[test]
    fn timestamp_has_expected_shape() {
        let stamp = local_timestamp();
        // YYYY-MM-DD HH:MM (UTC+8)
        assert!(stamp.ends_with("(UTC+8)"), "实际 {stamp}");
        assert_eq!(stamp.chars().filter(|c| *c == '-').count(), 2);
        assert!(stamp.starts_with("20"));
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        // 闰日
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }
}

#[cfg(test)]
mod os_tests {
    use super::*;

    #[test]
    fn description_always_includes_architecture() {
        let text = os_description();
        assert!(text.contains(std::env::consts::ARCH), "实际 {text}");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_description_carries_version_and_build() {
        let text = os_description();
        // 形如 macOS 26.5.2 (25F84, arm64)
        assert!(text.starts_with("macOS "), "实际 {text}");
        assert!(text.contains('('), "应带构建号与架构：{text}");
        // 版本号至少要有一个点分段，避免只拿到产品名。
        assert!(text.chars().any(|c| c.is_ascii_digit()), "应含版本号：{text}");
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_description_carries_product_and_build() {
        let text = os_description();
        assert!(text.to_lowercase().contains("windows"), "实际 {text}");
        assert!(text.chars().any(|c| c.is_ascii_digit()), "应含构建号：{text}");
    }

    #[test]
    fn description_is_a_single_line() {
        // 会进 Markdown 表格单元格，换行会破坏表格。
        let text = os_description();
        assert!(!text.contains('\n'), "实际 {text:?}");
        assert!(!text.contains('|'), "竖线会破坏表格：{text:?}");
    }
}

#[cfg(test)]
mod health_probe_tests {
    use super::*;
    use reqwest::StatusCode;

    /// 用户报告过的 Windows 故障链的起点：内核带鉴权后首页返回 401，
    /// 旧逻辑只认 2xx/3xx，于是判定"服务没起来"，升级后报 DSH 启动失败，
    /// 重开软件退回首次安装，装完再次失败——循环。
    #[test]
    fn unauthorized_means_the_service_is_up() {
        assert!(status_means_serving(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn success_and_redirects_pass() {
        assert!(status_means_serving(StatusCode::OK));
        assert!(status_means_serving(StatusCode::NO_CONTENT));
        // 内核用令牌换 cookie 时回 303。
        assert!(status_means_serving(StatusCode::SEE_OTHER));
        assert!(status_means_serving(StatusCode::TEMPORARY_REDIRECT));
    }

    /// 反过来也要守住：真正的故障不能被当成健康。
    #[test]
    fn server_errors_and_missing_routes_still_fail() {
        assert!(!status_means_serving(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(!status_means_serving(StatusCode::BAD_GATEWAY));
        assert!(!status_means_serving(StatusCode::NOT_FOUND));
        // 403 是同源校验失败，说明请求被拒而不是服务未就绪，
        // 但它不该让健康检查通过——那会掩盖代理配置错误。
        assert!(!status_means_serving(StatusCode::FORBIDDEN));
    }
}
