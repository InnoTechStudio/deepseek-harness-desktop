//! dsh 内核：版本检测（npm 镜像）、安装、overlay 原子切换、健康确认、回退。
use crate::mirrors::{choose_registry, npm_mirrors};
use crate::paths::Paths;
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;

/// npm 包名
pub const PKG: &str = "@deepseek-ai/dsh";

/// 从 npm 镜像获取官方最新版本号。
/// 返回 (版本号, 用到的 registry)。
pub async fn latest_kernel_version(
    client: &reqwest::Client,
    paths: &Paths,
) -> Result<(String, String), String> {
    let settings = crate::paths::Settings::load(paths);
    let mirrors = npm_mirrors();
    let registry = choose_registry(client, "npm", settings.preferred_registry.clone(), mirrors)
        .await
        .ok_or_else(|| "无法确定 npm 源".to_string())?;

    let encoded = PKG.replace('/', "%2f");
    let url = format!("{registry}{encoded}");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("查询官方版本失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("查询官方版本失败: HTTP {}", resp.status().as_u16()));
    }
    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    // 版本号在 dist-tags.latest（npm registry 标准结构）
    let ver = json
        .get("dist-tags")
        .and_then(|d| d.get("latest"))
        .and_then(|v| v.as_str())
        .or_else(|| json.get("version").and_then(|v| v.as_str()))
        .ok_or_else(|| "官方版本号缺失".to_string())?
        .to_string();

    // 记住这次选中的 registry，下次直接用（缓存）
    let mut s = crate::paths::Settings::load(paths);
    s.preferred_registry = Some(registry.clone());
    s.save(paths);
    Ok((ver, registry))
}

/// pnpm 安装的真实进度。
///
/// pnpm 不报字节级进度，但 `--reporter=ndjson` 会为每个包发解析/落盘事件，
/// 依赖总数在解析结束时就已知——据此能算出真实比例，而不是靠关键词猜。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPhase {
    /// 正在解析依赖树，总数未知。
    Resolving { resolved: u64 },
    /// 解析完成，正在下载并写入。`total` 为解析阶段确定的依赖总数。
    Importing { imported: u64, total: u64 },
    /// 写入完成，正在做最后校验。
    Finishing,
}

impl InstallPhase {
    /// 该阶段在整个 pnpm 安装中的完成比例（0..1）。
    ///
    /// 解析阶段占前 35%：它的总数事先不知道，用一条趋近曲线让进度持续前进而
    /// 永不越过上界——比让进度条长时间静止要诚实，也不会出现回退。
    pub fn ratio(self) -> f64 {
        match self {
            InstallPhase::Resolving { resolved } => {
                // 每解析 40 个包走完剩余距离的一半，天然收敛到 0.35。
                let progress = 1.0 - 0.5_f64.powf(resolved as f64 / 40.0);
                0.35 * progress
            }
            InstallPhase::Importing { imported, total } => {
                let done = if total == 0 {
                    0.0
                } else {
                    (imported as f64 / total as f64).clamp(0.0, 1.0)
                };
                0.35 + 0.60 * done
            }
            InstallPhase::Finishing => 0.98,
        }
    }

    /// 展示给用户的文案。
    pub fn label(self) -> String {
        match self {
            InstallPhase::Resolving { resolved } => {
                if resolved == 0 {
                    "正在解析依赖".to_string()
                } else {
                    format!("正在解析依赖（{resolved} 个）")
                }
            }
            InstallPhase::Importing { imported, total } => {
                format!("正在安装依赖（{imported}/{total}）")
            }
            InstallPhase::Finishing => "正在校验内核文件".to_string(),
        }
    }
}

/// 解析一行 pnpm ndjson 输出，得到进度变化。
///
/// 只认三种事件，其余（约占输出的九成）直接忽略：
/// - `pnpm:progress` status=resolved —— 解析出一个依赖
/// - `pnpm:progress` status=imported —— 一个依赖已落盘
/// - `pnpm:stage` stage=resolution_done —— 解析结束，此刻 resolved 计数即总数
///
/// @returns 需要上报的新进度；该行不含进度信息时为 None。
pub fn parse_pnpm_progress(line: &str, resolved: &mut u64, imported: &mut u64, total: &mut Option<u64>) -> Option<InstallPhase> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    match value.get("name").and_then(|n| n.as_str())? {
        "pnpm:progress" => {
            match value.get("status").and_then(|s| s.as_str())? {
                "resolved" => {
                    *resolved += 1;
                    // 解析阶段结束后仍可能零星出现 resolved，此时已进入下载阶段，
                    // 不该退回解析文案。
                    if total.is_none() {
                        return Some(InstallPhase::Resolving { resolved: *resolved });
                    }
                    None
                }
                "imported" => {
                    *imported += 1;
                    // total 未知时用当前 resolved 兜底，保证分母不为 0。
                    let denominator = total.unwrap_or(*resolved).max(*imported);
                    Some(InstallPhase::Importing { imported: *imported, total: denominator })
                }
                _ => None,
            }
        }
        "pnpm:stage" => match value.get("stage").and_then(|s| s.as_str())? {
            "resolution_done" => {
                *total = Some(*resolved);
                Some(InstallPhase::Importing { imported: *imported, total: *resolved })
            }
            "importing_done" => Some(InstallPhase::Finishing),
            _ => None,
        },
        _ => None,
    }
}

/// 用内置 node 跑 pnpm 安装指定版本到 staging（全新目录，失败零影响）。
/// pnpm 比 npm 快得多（实测 dsh 内核 5s 装完，npm 会卡住）。
/// on_progress 可选：安装期间实时回调真实进度。
pub async fn install_kernel_to(
    node_exe: &Path,
    staging: &Path,
    registry: &str,
    version: &str,
    on_progress: Option<Arc<dyn Fn(InstallPhase) + Send + Sync>>,
) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging).map_err(|e| e.to_string())?;

    let pkg_spec = format!("{PKG}@{version}");

    // 用内置 node 运行 pnpm。node 官方包不自带 pnpm，先确保装好：
    // 用 node 直接跑 npm-cli.js 安装 pnpm（避免 npm shebang 依赖 PATH）
    let node_dir = node_exe.parent().unwrap();
    let pnpm = node_dir.join(if cfg!(target_os = "windows") { "pnpm.cmd" } else { "pnpm" });
    let node_root = if cfg!(target_os = "windows") {
        node_dir
    } else {
        node_dir.parent().unwrap_or(node_dir)
    };

    if !pnpm.exists() {
        let npm_cli = node_root.join(if cfg!(target_os = "windows") {
            "node_modules/npm/bin/npm-cli.js"
        } else {
            "lib/node_modules/npm/bin/npm-cli.js"
        });
        if !npm_cli.exists() {
            return Err(format!("内置 npm CLI 不存在：{}", npm_cli.display()));
        }
        let mut install_cmd = std::process::Command::new(node_exe);
        crate::dsh::suppress_console(&mut install_cmd);
        let install = install_cmd
            .arg(&npm_cli)
            .args(["install", "-g", "pnpm@11", "--prefix", node_root.to_str().unwrap_or("."), "--registry", registry])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("无法启动 npm 安装 pnpm: {e}"))?;
        if !install.success() {
            return Err("安装内置 pnpm 失败".to_string());
        }
    }

    // 关键：pnpm 的 shebang 是 `#!/usr/bin/env node`，必须把 node 目录放进 PATH，
    // 否则 env node 找不到，pnpm 直接退出。直接用 node 运行 pnpm 入口（pnpm.mjs），
    // 完全绕开 shebang/PATH 问题。
    let pnpm_js = node_root.join(if cfg!(target_os = "windows") {
        "node_modules/pnpm/bin/pnpm.mjs"
    } else {
        "lib/node_modules/pnpm/bin/pnpm.mjs"
    });
    if !pnpm_js.exists() {
        return Err(format!("内置 pnpm 入口不存在：{}", pnpm_js.display()));
    }

    // 流式读取 pnpm 的 ndjson 输出。
    //
    // 之前用 Command::output()：它等进程完整结束才返回，于是整个安装过程中进度
    // 条一动不动，pnpm 一收尾所有回调瞬间回放，进度直接从 44% 跳到 76%。必须
    // spawn + 边跑边读，进度才是真实的。
    let node_exe = node_exe.to_path_buf();
    let staging = staging.to_path_buf();
    let registry = registry.to_string();
    let pkg_spec = pkg_spec.to_string();
    let pnpm_js = pnpm_js.to_path_buf();
    let entry_check = staging.clone();

    let out = tokio::task::spawn_blocking(move || -> Result<(std::process::ExitStatus, String), String> {
        use std::io::{BufRead, BufReader};

        let mut cmd = std::process::Command::new(&node_exe);
        crate::dsh::suppress_console(&mut cmd);
        cmd.arg(&pnpm_js)
            .current_dir(&staging)
            .args([
                "add",
                &pkg_spec,
                // pnpm v11 用 --config.registry= 形式（两个独立参数会被当成布尔开关）
                &format!("--config.registry={}", registry),
                "--ignore-scripts",
                "--node-linker=hoisted",
                // 结构化输出：每行一个 JSON 事件，含逐包的解析/落盘进度。
                "--reporter=ndjson",
            ])
            .env("COREPACK_NPM_REGISTRY", &registry)
            .env("npm_config_registry", &registry)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("无法启动 pnpm: {e}"))?;

        // stderr 另起线程排空：pnpm 出错时会往 stderr 写不少内容，
        // 不排空会填满管道缓冲区把进程卡死。
        let stderr = child.stderr.take();
        let stderr_thread = std::thread::spawn(move || {
            let mut buffer = String::new();
            if let Some(stderr) = stderr {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    buffer.push_str(&line);
                    buffer.push('\n');
                }
            }
            buffer
        });

        // 失败时要给出可读的原因，但 ndjson 每行都很长，只留最后若干行。
        let mut recent: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        let mut resolved = 0_u64;
        let mut imported = 0_u64;
        let mut total: Option<u64> = None;
        if let Some(stdout) = child.stdout.take() {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let (Some(callback), Some(phase)) = (
                    on_progress.as_ref(),
                    parse_pnpm_progress(&line, &mut resolved, &mut imported, &mut total),
                ) {
                    callback(phase);
                }
                if recent.len() == 8 {
                    recent.pop_front();
                }
                recent.push_back(line);
            }
        }
        let status = child.wait().map_err(|e| format!("等待 pnpm 结束失败: {e}"))?;
        let stderr_text = stderr_thread.join().unwrap_or_default();
        let diagnostics = if status.success() {
            String::new()
        } else {
            stderr_text
                .lines()
                .chain(recent.iter().map(String::as_str))
                .filter(|line| !line.trim().is_empty())
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .join(" | ")
        };
        Ok((status, diagnostics))
    })
    .await
    .map_err(|e| format!("安装线程失败: {e}"))??;

    let (status, diagnostics) = out;
    if !status.success() {
        return Err(format!(
            "pnpm 安装失败（退出码 {:?}）：{}",
            status.code(),
            diagnostics
        ));
    }

    // 验证入口存在
    let entry = entry_check
        .join("node_modules")
        .join(PKG)
        .join("lib")
        .join("bin.js");
    if !entry.exists() {
        return Err("安装完成但未找到 dsh 入口文件".to_string());
    }
    Ok(())
}

/// 当前生效版本：overlay 优先，其次 staging？不，staging 是临时。overlay -> 无。
pub fn active_kernel_version(paths: &Paths) -> Option<String> {
    read_pkg_version(&paths.agent_dir)
}

pub fn read_pkg_version(dir: &Path) -> Option<String> {
    let pkg_json = dir
        .join("node_modules")
        .join(PKG)
        .join("package.json");
    let txt = std::fs::read_to_string(pkg_json).ok()?;
    let v: serde_json::Value = serde_json::from_str(&txt).ok()?;
    v.get("version").and_then(|x| x.as_str()).map(|s| s.to_string())
}

/// dsh 入口 bin.js 路径
pub fn kernel_bin(paths: &Paths) -> std::path::PathBuf {
    paths
        .agent_dir
        .join("node_modules")
        .join(PKG)
        .join("lib")
        .join("bin.js")
}

/// 内核安装是否完整可用。
///
/// 只检查入口文件存在是不够的：pnpm 中途被打断时入口可能已经写下，
/// 但依赖树不完整，结果表现为"内核已就绪但服务永远起不来"，而且因为
/// `kernel_ready` 为真，重启也不会重新安装，用户只能重装系统或清数据。
pub fn kernel_healthy(paths: &Paths) -> bool {
    if !kernel_bin(paths).exists() {
        return false;
    }
    let pkg_dir = paths.agent_dir.join("node_modules").join(PKG);
    // 包自身的 package.json 必须可解析，且声明的依赖都要能在 node_modules 里找到。
    let Ok(text) = std::fs::read_to_string(pkg_dir.join("package.json")) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let modules = paths.agent_dir.join("node_modules");
    let Some(deps) = manifest.get("dependencies").and_then(|d| d.as_object()) else {
        // 没有依赖声明时，入口存在即视为完整。
        return true;
    };
    deps.keys().all(|name| modules.join(name).exists())
}

/// 删除已安装的内核与残留的 staging，用于自修复重装。
pub fn purge_kernel(paths: &Paths) -> Result<(), String> {
    for dir in [&paths.agent_dir, &paths.staging_dir] {
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|e| format!("清理 {} 失败: {e}", dir.display()))?;
        }
    }
    Ok(())
}

/// 原子切换：把 staging 换成 overlay。
/// 旧 overlay → agent-previous（保留，供回退）；staging → overlay。
pub fn swap_staging_to_overlay(paths: &Paths) -> Result<(), String> {
    // 先清理上次的 previous
    if paths.backup_dir.exists() {
        std::fs::remove_dir_all(&paths.backup_dir).map_err(|e| e.to_string())?;
    }
    if paths.agent_dir.exists() {
        std::fs::rename(&paths.agent_dir, &paths.backup_dir).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&paths.staging_dir, &paths.agent_dir).map_err(|e| e.to_string())?;
    Ok(())
}

/// 一键回退：agent → 临时，backup → agent
pub fn rollback(paths: &Paths) -> Result<(), String> {
    if !paths.backup_dir.exists() {
        return Err("没有可回退的版本".to_string());
    }
    let tmp = paths.root.join("agent-broken");
    if paths.agent_dir.exists() {
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&paths.agent_dir, &tmp).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&paths.backup_dir, &paths.agent_dir).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

/// 升级完成后健康确认：新版能启动并返回 HTTP 200 → 清理 previous
pub fn confirm_healthy_cleanup(paths: &Paths) {
    if paths.backup_dir.exists() {
        let _ = std::fs::remove_dir_all(&paths.backup_dir);
    }
}

/// 预装 dsh-market 插件到 web profile，让插件市场出现在 dsh 自带设置里。
/// 需要 node runtime 的 pnpm 可用（dsh plugin 命令内部调用 pnpm）。
pub fn preinstall_market(paths: &Paths, registry: &str) -> Result<(), String> {
    allow_plugin_build_scripts(paths);
    preinstall_plugin(paths, registry, "dshmarket")
}

/// 允许插件依赖执行安装脚本。
///
/// pnpm 11 默认拒绝运行依赖的构建脚本，遇到需要编译的包（sharp、tesseract.js
/// 这类原生/OCR 依赖）会以 ERR_PNPM_IGNORED_BUILDS 中止整个安装。用户从插件
/// 市场装带这类依赖的插件时会直接失败，而错误信息只提示"运行 pnpm approve-builds"
/// ——桌面端没有终端可以运行它。这里预先在 workspace 配置里放开白名单。
///
/// 允许插件依赖执行安装脚本。
///
/// pnpm 11 默认拒绝运行依赖的构建脚本，遇到需要编译的包（sharp、tesseract.js
/// 这类原生/OCR 依赖）会以 ERR_PNPM_IGNORED_BUILDS 中止整个安装。用户从插件
/// 市场装带这类依赖的插件时会直接失败，而错误信息只提示"运行 pnpm approve-builds"
/// ——桌面端没有终端可以运行它。
///
/// 做两件事：
/// 1. 预先放开已知需要编译的依赖，让常见插件第一次就能装上；
/// 2. 把 pnpm 写下的任何待决占位行改成 true，这样带全新原生依赖的插件也能装。
///
/// 第 2 条不等于 dangerouslyAllowAllBuilds：占位行只会为"用户当前正在安装的
/// 依赖"生成，是对用户已做出的安装选择放行，而不是无条件预授权整个生态。
/// 而且插件本身就是被加载进内核进程执行的 JS，拦住安装脚本并不能真正限制它，
/// 只会让正经插件装不上。
///
/// 每次启动内核前都要调用：占位行是用户第一次装带原生依赖的插件时才由 pnpm
/// 写入的，只在装内核时放开一次赶不上（见 dsh.rs 里的调用点）。
///
/// @returns 是否改写了文件。
pub fn allow_plugin_build_scripts(paths: &Paths) -> bool {
    /// 已知需要构建脚本的插件依赖，预先放开，省掉一次失败重试。
    const ALLOWED: [&str; 2] = ["sharp", "tesseract.js"];
    /// pnpm 写下的待决标记。
    const PENDING: &str = ": set this to true or false";

    let file = paths
        .root
        .join("dsh-home/profiles/web/pnpm-workspace.yaml");
    let Ok(text) = std::fs::read_to_string(&file) else { return false };

    // 逐行改写：任何待决占位行都置为 true，保持原有缩进与位置，
    // 避免重排用户可能改过的其他配置。
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| match line.find(PENDING) {
            Some(index) => format!("{}: true", &line[..index]),
            None => line.to_string(),
        })
        .collect();

    // 再补上已知依赖：没有 allowBuilds 段就整段追加，有则插入缺失项。
    match lines.iter().position(|line| line.trim_start().starts_with("allowBuilds:")) {
        None => {
            while lines.last().is_some_and(|line| line.trim().is_empty()) {
                lines.pop();
            }
            lines.push("allowBuilds:".to_string());
            for name in ALLOWED {
                lines.push(format!("  {name}: true"));
            }
        }
        Some(index) => {
            for name in ALLOWED {
                let declared = lines
                    .iter()
                    .any(|line| line.trim_start().starts_with(&format!("{name}:")));
                if !declared {
                    lines.insert(index + 1, format!("  {name}: true"));
                }
            }
        }
    }

    let output = format!("{}\n", lines.join("\n"));
    if output == text {
        return false;
    }
    std::fs::write(&file, output).is_ok()
}

pub fn remove_legacy_skill_market(paths: &Paths) {
    let manifest = paths.root.join("dsh-home/profiles/web/package.json");
    let Ok(text) = std::fs::read_to_string(&manifest) else { return };
    let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&text) else { return };
    let mut changed = false;
    if let Some(deps) = json.get_mut("dependencies").and_then(|v| v.as_object_mut()) {
        changed |= deps.remove("dsh-skill-market").is_some();
    }
    if let Some(bundles) = json.pointer_mut("/dsh/profile/bundles").and_then(|v| v.as_array_mut()) {
        let before = bundles.len();
        bundles.retain(|v| v.as_str() != Some("dsh-skill-market"));
        changed |= bundles.len() != before;
    }
    if changed {
        if let Ok(output) = serde_json::to_string_pretty(&json) { let _ = std::fs::write(&manifest, output + "\n"); }
        let _ = std::fs::remove_file(manifest.with_file_name("node_modules/dsh-skill-market"));
    }
}


pub fn preinstall_desktop_shell(paths: &Paths, _registry: &str, package_dir: &Path) -> Result<(), String> {
    let node_exe = crate::runtime::node_bin(paths);
    let entry = kernel_bin(paths);
    if !node_exe.exists() || !entry.exists() {
        return Ok(())
    }
    let profile_dir = paths.root.join("dsh-home").join("profiles").join("web");
    let profile_package = profile_dir.join("package.json");
    let package_name = std::fs::read_to_string(package_dir.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value.get("name").and_then(|name| name.as_str()).map(str::to_string))
        .unwrap_or_else(|| LOCAL_SHELL_PACKAGE.to_string());
    let destination = profile_dir.join("node_modules").join(&package_name);
    copy_directory(package_dir, &destination)?;

    // 只登记到 bundles，不写 dependencies。
    //
    // dshmarket 的"已安装"列表只读 dependencies（lib/profile.js readInstalled），
    // 所以不写这一项，本地插件就不会出现在市场里——它随客户端更新，不该被市场管理。
    // bundles 必须保留：DSH 只为 bundles 里的名字加载 cordis.patch.yml，
    // 删掉它 ui-layout 就不会被禁用，整个高级布局都不生效。
    // 包本身仍能解析，因为 DSH 从 profile 锚点走 Node 搜索路径找 node_modules。
    //
    // 另外，此前这里写的是包在 node_modules 里的自身路径，pnpm 会因
    // "Symlink path is the same as the target path" 直接退出，导致市场安装全部失败。
    let text = std::fs::read_to_string(&profile_package).map_err(|e| e.to_string())?;
    let mut manifest: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if let Some(dependencies) = manifest.get_mut("dependencies").and_then(|value| value.as_object_mut()) {
        dependencies.remove(package_name.as_str());
    }
    if let Some(bundles) = manifest.pointer_mut("/dsh/profile/bundles").and_then(|value| value.as_array_mut()) {
        if !bundles.iter().any(|value| value.as_str() == Some(package_name.as_str())) {
            bundles.push(serde_json::Value::String(package_name.clone()));
        }
    }
    let output = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(profile_package, output + "\n").map_err(|e| e.to_string())
}


/// 插件市场是否与当前内核不兼容到会让内核启动失败。
///
/// dshmarket 1.38.0 及更早版本从 `@deepseek-ai/dsh-settings` 具名导入
/// `installSettingsSection`，而内核 0.1.2 删掉了这个导出。缺失的具名导入
/// 是模块求值期的 SyntaxError，不是可降级的缺失服务：cordis 会判定整条
/// loader entry 失败，内核随即退出 1。
///
/// 后果是死锁——内核起不来，界面打不开，用户也就没有任何入口去更新市场。
/// 所以必须在启动前检出并自愈，不能等用户自己处理。
///
/// 判定只看 `settings.js` 里是否真的有这条 import 语句。1.39.0 把同样的
/// 文字留在了注释里说明历史，所以不能用裸的子串匹配。
pub fn market_breaks_kernel_boot(paths: &Paths) -> bool {
    let file = paths
        .root
        .join("dsh-home/profiles/web/node_modules/dshmarket/lib/settings.js");
    let Ok(text) = std::fs::read_to_string(&file) else {
        return false;
    };
    text.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("import") && line.contains("installSettingsSection")
    })
}

/// 把指定包从 profile 的 dependencies 和 bundles 里同时摘掉。
///
/// 只摘一处不够：DSH 按 bundles 加载 patch，市场按 dependencies 认领。
/// 目录留在 node_modules 不删，用户的安装记录还在。
fn disable_profile_package(paths: &Paths, package: &str) -> bool {
    let manifest = paths.root.join("dsh-home/profiles/web/package.json");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return false;
    };
    let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let mut changed = false;
    if let Some(deps) = json.get_mut("dependencies").and_then(|v| v.as_object_mut()) {
        changed |= deps.remove(package).is_some();
    }
    if let Some(bundles) = json
        .pointer_mut("/dsh/profile/bundles")
        .and_then(|v| v.as_array_mut())
    {
        let before = bundles.len();
        bundles.retain(|v| v.as_str() != Some(package));
        changed |= bundles.len() != before;
    }
    if !changed {
        return false;
    }
    let Ok(output) = serde_json::to_string_pretty(&json) else {
        return false;
    };
    std::fs::write(&manifest, output + "\n").is_ok()
}

/// 把插件市场从 profile 里摘掉，让内核能启动。
///
/// 只在市场会导致内核启动失败、且自动升级也救不回来时调用。目录留在原地不删：
/// 用户的安装记录还在，等市场发布兼容版本后重新登记即可恢复。
///
/// 宁可少一个插件，也不能让整个软件打不开。
///
/// @returns 是否改动了 profile。
pub fn disable_market(paths: &Paths) -> bool {
    disable_profile_package(paths, "dshmarket")
}

/// 社区文件上传插件。与内核 0.1.5 起内置的 `@deepseek-ai/dsh-client-file-upload`
/// 抢同一个 loader id `file-upload`，不能并存。
pub const COMMUNITY_FILE_UPLOAD_PACKAGE: &str = "dsh-file-upload";

/// 当前内核是否已经自己登记了 `id: file-upload`。
///
/// 0.1.5-rc.1 起 `@deepseek-ai/dsh-web-app` 的 patch 会插入官方
/// `@deepseek-ai/dsh-client-file-upload`。cordis 要求 loader id 全局唯一，
/// 社区插件 `dsh-file-upload` 也用这个 id，两边一并存内核直接退出 1。
///
/// 判定只认 YAML 列表项 `id: file-upload`，注释不算。文件不存在（旧内核）
/// 视为内核没占这个槽，社区插件可以继续用。
pub fn kernel_owns_file_upload(paths: &Paths) -> bool {
    let candidates = [
        paths
            .agent_dir
            .join("node_modules/@deepseek-ai/dsh-web-app/cordis.patch.yml"),
        paths.agent_dir.join(
            "node_modules/@deepseek-ai/dsh/node_modules/@deepseek-ai/dsh-web-app/cordis.patch.yml",
        ),
    ];
    candidates.iter().any(|file| {
        let Ok(text) = std::fs::read_to_string(file) else {
            return false;
        };
        text.lines().any(line_declares_file_upload_id)
    })
}

fn line_declares_file_upload_id(line: &str) -> bool {
    let raw = line.trim_start();
    if raw.starts_with('#') {
        return false;
    }
    let item = raw.strip_prefix('-').map(|s| s.trim_start()).unwrap_or(raw);
    item == "id: file-upload"
        || item == "id: \"file-upload\""
        || item == "id: 'file-upload'"
}

/// 社区 `dsh-file-upload` 是否还登记在 profile 里。
fn profile_has_community_file_upload(paths: &Paths) -> bool {
    let manifest = paths.root.join("dsh-home/profiles/web/package.json");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    if json
        .get("dependencies")
        .and_then(|v| v.as_object())
        .is_some_and(|deps| deps.contains_key(COMMUNITY_FILE_UPLOAD_PACKAGE))
    {
        return true;
    }
    json.pointer("/dsh/profile/bundles")
        .and_then(|v| v.as_array())
        .is_some_and(|bundles| {
            bundles
                .iter()
                .any(|v| v.as_str() == Some(COMMUNITY_FILE_UPLOAD_PACKAGE))
        })
}

/// 官方内核已经占用 `file-upload` 槽，且 profile 里还装着社区同名插件。
///
/// 命中时必须在 spawn 前摘掉社区插件，否则 cordis 报
/// `duplicate loader entry id: file-upload`，内核退出 1，界面会当成
/// 「内核坏了」去重装——重装清不掉 profile 插件，于是死循环。
pub fn file_upload_plugin_collides(paths: &Paths) -> bool {
    kernel_owns_file_upload(paths) && profile_has_community_file_upload(paths)
}

/// 把社区文件上传插件从 profile 里摘掉。目录留在原地不删。
///
/// @returns 是否改动了 profile。
pub fn disable_community_file_upload(paths: &Paths) -> bool {
    disable_profile_package(paths, COMMUNITY_FILE_UPLOAD_PACKAGE)
}

/// Patch the installed market process launcher so Windows never shows a shell window.
/// dsh-market uses COMSPEC for `.cmd` shims; Node's `windowsHide` must be set on
/// that child spawn itself, not only on the desktop host that launched dsh.
pub fn suppress_market_console(paths: &Paths) -> bool {
    let file = paths.root.join("dsh-home/profiles/web/node_modules/dshmarket/lib/dsh-cli.js");
    let Ok(text) = std::fs::read_to_string(&file) else { return false };
    let mut patched = text.clone();
    patched = patched.replace(
        "{ ...spawnOptions, shell: false });",
        "{ ...spawnOptions, shell: false, windowsHide: true });",
    );
    patched = patched.replace(
        "shell: false,\n        windowsVerbatimArguments: true,",
        "shell: false,\n        windowsHide: true,\n        windowsVerbatimArguments: true,",
    );
    // taskkill 也会分配控制台窗口（取消安装时闪一下）。
    patched = patched.replace(
        "['/pid', String(child.pid), '/t', '/f'], { stdio: 'ignore' }",
        "['/pid', String(child.pid), '/t', '/f'], { stdio: 'ignore', windowsHide: true }",
    );
    if patched == text { return false }
    std::fs::write(file, patched).is_ok()
}

/// 桌面端随客户端分发的本地插件。它不经插件市场安装，也不该出现在市场的
/// 「已安装」里——市场靠 dependencies 认领插件，这个名字必须始终不在其中。
pub const LOCAL_SHELL_PACKAGE: &str = "dsh-desktop-shell";

/// 把 profile 里"装着但没登记"的插件重新写回 dependencies，返回被修复的名称。
///
/// 插件市场的「已安装」列表只读 dependencies（lib/profile.js readInstalled），
/// bundles 完全不参与。于是存在一种半安装状态：包目录在 node_modules 里、名字
/// 在 bundles 里，但 dependencies 里没有——插件照常加载生效，市场却看不见它，
/// 也就再也无法通过市场更新或卸载。
///
/// 这种状态是市场自己制造的：安装失败时它会回滚 dependencies（见
/// readProfileManifestSnapshot / restore），而已经落盘的目录和 bundles 条目
/// 留在原处。Windows 上 pnpm 因 store 位置、构建脚本审批等原因更容易中途失败，
/// 所以同一个插件在 Windows 上"消失"、在 macOS 上还在，看起来像平台差异。
///
/// 修复方向只能是补登记、不能是删目录：插件已经在工作，删掉会让用户丢功能。
/// 版本号取自包自己的 package.json，写成 `^x.y.z` 与市场安装的写法一致。
///
/// 排除 [`LOCAL_SHELL_PACKAGE`]（本就该只在 bundles 里），以及内核已经
/// 占用 `file-upload` 槽时的 [`COMMUNITY_FILE_UPLOAD_PACKAGE`]——再登记回去
/// 会让下一次启动立刻撞 duplicate id。
pub fn restore_unregistered_profile_plugins(paths: &Paths) -> Vec<String> {
    let manifest = paths
        .root
        .join("dsh-home")
        .join("profiles")
        .join("web")
        .join("package.json");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return Vec::new();
    };
    let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let modules = manifest.with_file_name("node_modules");

    let declared: Vec<String> = json
        .get("dependencies")
        .and_then(|deps| deps.as_object())
        .map(|deps| deps.keys().cloned().collect())
        .unwrap_or_default();

    let mut restored: Vec<(String, String)> = Vec::new();
    if let Some(bundles) = json.pointer("/dsh/profile/bundles").and_then(|b| b.as_array()) {
        for entry in bundles {
            let Some(name) = entry.as_str() else { continue };
            // 官方内置 bundle 由内核提供，不在 profile 的 node_modules 里，
            // 市场也主动把它们从「已安装」里滤掉。
            if name.starts_with("@deepseek-ai/") || name == LOCAL_SHELL_PACKAGE {
                continue;
            }
            if name == COMMUNITY_FILE_UPLOAD_PACKAGE && kernel_owns_file_upload(paths) {
                continue;
            }
            if declared.iter().any(|d| d == name) {
                continue;
            }
            let package_json = modules.join(name).join("package.json");
            let Ok(text) = std::fs::read_to_string(&package_json) else {
                // 目录不在就不是"半安装"，交给 prune 去清理 bundles。
                continue;
            };
            let version = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|value| {
                    value
                        .get("version")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                });
            // 读不出版本时用 `*`：宁可让市场以"任意版本"认领它，
            // 也比继续隐身、永远无法更新好。
            let spec = version.map_or_else(|| "*".to_string(), |v| format!("^{v}"));
            restored.push((name.to_string(), spec));
        }
    }

    if restored.is_empty() {
        return Vec::new();
    }
    let Some(deps) = json
        .get_mut("dependencies")
        .and_then(|deps| deps.as_object_mut())
    else {
        return Vec::new();
    };
    for (name, spec) in &restored {
        deps.insert(name.clone(), serde_json::Value::String(spec.clone()));
    }
    let Ok(out) = serde_json::to_string_pretty(&json) else {
        return Vec::new();
    };
    if std::fs::write(&manifest, out + "\n").is_err() {
        return Vec::new();
    }
    restored.into_iter().map(|(name, _)| name).collect()
}

/// 返回被清理掉的 bundle 名称。
///
/// 除了 link:/file: 目标丢失的依赖，还要检查 bundles 里目录已不存在的本地插件——
/// 桌面 shell 只登记在 bundles 里（不写 dependencies），靠依赖扫描发现不了它。
pub fn prune_missing_profile_bundles(paths: &Paths) -> Vec<String> {
    let manifest = paths
        .root
        .join("dsh-home")
        .join("profiles")
        .join("web")
        .join("package.json");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return Vec::new();
    };
    let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let modules = manifest.with_file_name("node_modules");

    // 找出所有 link: 目标已不存在的依赖
    let mut stale = Vec::new();
    if let Some(deps) = json.get("dependencies").and_then(|d| d.as_object()) {
        for (name, spec) in deps {
            let Some(spec) = spec.as_str() else { continue };
            let Some(target) = spec.strip_prefix("link:").or_else(|| spec.strip_prefix("file:"))
            else {
                continue;
            };
            if !Path::new(target).exists() {
                stale.push(name.clone());
            }
        }
    }
    // 再找出 bundles 里包目录已消失的条目。留着会让 DSH 启动时直接报
    // "cannot resolve profile bundle"，而不是降级运行。
    //
    // 只检查没有 dependencies 条目的名字：有条目的已由上面的 link: 检查覆盖，
    // 或者交给 pnpm 管理，它们的 node_modules 布局不该由我们判断。
    if let Some(bundles) = json.pointer("/dsh/profile/bundles").and_then(|b| b.as_array()) {
        let declared: Vec<String> = json
            .get("dependencies")
            .and_then(|d| d.as_object())
            .map(|deps| deps.keys().cloned().collect())
            .unwrap_or_default();
        for entry in bundles {
            let Some(name) = entry.as_str() else { continue };
            // 官方内置 bundle 由内核自己提供，不在 profile 的 node_modules 里。
            if name.starts_with("@deepseek-ai/")
                || declared.iter().any(|d| d == name)
                || stale.iter().any(|s| s == name)
            {
                continue;
            }
            if !modules.join(name).exists() {
                stale.push(name.to_string());
            }
        }
    }
    if stale.is_empty() {
        return Vec::new();
    }

    if let Some(deps) = json.get_mut("dependencies").and_then(|d| d.as_object_mut()) {
        for name in &stale {
            deps.remove(name);
        }
    }
    if let Some(bundles) = json
        .pointer_mut("/dsh/profile/bundles")
        .and_then(|b| b.as_array_mut())
    {
        bundles.retain(|b| !b.as_str().map(|s| stale.contains(&s.to_string())).unwrap_or(false));
    }
    let Ok(out) = serde_json::to_string_pretty(&json) else {
        return Vec::new();
    };
    if std::fs::write(&manifest, out + "\n").is_err() {
        return Vec::new();
    }
    // 同时移除 node_modules 里的悬空符号链接，避免 pnpm/解析再次踩到
    for name in &stale {
        let _ = std::fs::remove_file(modules.join(name));
    }
    stale
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        std::fs::remove_dir_all(destination).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let target = destination.join(entry.file_name());
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn update_profile_manifest(profile_package: &Path, package_name: &str, destination: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(profile_package).map_err(|e| e.to_string())?;
    let mut manifest: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if let Some(dependencies) = manifest.get_mut("dependencies").and_then(|value| value.as_object_mut()) {
        dependencies.insert(package_name.to_string(), serde_json::Value::String(destination.to_string_lossy().to_string()));
    }
    if let Some(bundles) = manifest.pointer_mut("/dsh/profile/bundles").and_then(|value| value.as_array_mut()) {
        if !bundles.iter().any(|value| value.as_str() == Some(package_name)) {
            bundles.push(serde_json::Value::String(package_name.to_string()));
        }
    }
    let output = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(profile_package, output + "\n").map_err(|e| e.to_string())
}

/// 预装一个插件到指定的 web profile。重复安装由 dsh 自身处理，失败不影响内核。
pub fn preinstall_plugin(paths: &Paths, registry: &str, package: &str) -> Result<(), String> {
    let node_exe = crate::runtime::node_bin(paths);
    let entry = kernel_bin(paths);
    if !node_exe.exists() || !entry.exists() {
        return Ok(()); // 未就绪则跳过
    }
    let dsh_home = paths.root.join("dsh-home");
    let profile_dir = dsh_home.join("profiles").join("web");
    let profile_package = profile_dir.join("package.json");
    let package_path = Path::new(package);
    let package_name = if package_path.exists() {
        std::fs::read_to_string(package_path.join("package.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| value.get("name").and_then(|name| name.as_str()).map(str::to_string))
            .unwrap_or_else(|| LOCAL_SHELL_PACKAGE.to_string())
    } else {
        package.to_string()
    };

    if package_path.exists() {
        let destination = profile_dir.join("node_modules").join(&package_name);
        copy_directory(package_path, &destination)?;
        update_profile_manifest(&profile_package, &package_name, &destination)?;
        return Ok(());
    }

    let package_arg = package.to_string();
    let pnpm_bin = if cfg!(target_os = "windows") {
        node_exe
            .parent()
            .map(|p| p.join("node_modules/pnpm/bin/pnpm.mjs"))
    } else {
        node_exe
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("lib/node_modules/pnpm/bin/pnpm.mjs"))
    };
    let mut cmd = std::process::Command::new(&node_exe);
    crate::dsh::suppress_console(&mut cmd);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000 | 0x0000_0008);
    }
    cmd.arg(&entry)
        .args(["plugin", "--profile", "web", "add", &package_arg])
        .arg(format!("--registry={registry}"))
        .env("DSH_HOME", &dsh_home)
        .env(
            "PATH",
            prepend_dir(node_exe.parent().unwrap_or(std::path::Path::new("."))),
        );
    if let Some(pnpm) = pnpm_bin.filter(|p| p.exists()) {
        cmd.env("DSH_PNPM", pnpm);
    }
    let status = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("无法启动 dsh plugin 预装 {package}: {e}"))?;
    if !status.success() {
        return Err(format!("dsh plugin 预装 {package} 失败：退出码 {:?}", status.code()));
    }
    if package_path.exists() && profile_package.exists() {
        let text = std::fs::read_to_string(&profile_package).map_err(|e| e.to_string())?;
        let mut manifest: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if let Some(dependencies) = manifest.get_mut("dependencies").and_then(|value| value.as_object_mut()) {
            dependencies.insert(package_name.clone(), serde_json::Value::String(package_path.to_string_lossy().to_string()));
        }
        if let Some(bundles) = manifest.pointer_mut("/dsh/profile/bundles").and_then(|value| value.as_array_mut()) {
            if !bundles.iter().any(|value| value.as_str() == Some(package_name.as_str())) {
                bundles.push(serde_json::Value::String(package_name));
            }
        }
        let output = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
        std::fs::write(&profile_package, output + "\n").map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn prepend_dir(dir: &std::path::Path) -> std::ffi::OsString {
    let mut paths = vec![dir.to_path_buf()];
    if let Ok(existing) = std::env::var("PATH") {
        for p in std::env::split_paths(&existing) {
            paths.push(p);
        }
    }
    std::env::join_paths(paths).unwrap_or_default()
}

/// semver 比较（兼容 rc 预发布：prerelease 版本 < 正式版）
pub fn compare_versions(a: &str, b: &str) -> i32 {
    let parse = |v: &str| -> (Vec<u64>, bool, String) {
        let (core, pre) = match v.trim().trim_start_matches('v').split_once('-') {
            Some((c, p)) => (c.to_string(), p.to_string()),
            None => (v.trim().trim_start_matches('v').to_string(), String::new()),
        };
        let nums: Vec<u64> = core
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .take(3)
            .collect();
        (nums, !pre.is_empty(), pre)
    };
    let (an, ap, apv) = parse(a);
    let (bn, bp, bpv) = parse(b);
    for i in 0..3 {
        let x = an.get(i).copied().unwrap_or(0);
        let y = bn.get(i).copied().unwrap_or(0);
        if x != y {
            return if x > y { 1 } else { -1 };
        }
    }
    match (ap, bp) {
        (false, false) => 0,
        (true, false) => -1, // prerelease < release
        (false, true) => 1,
        (true, true) => {
            if apv == bpv {
                0
            } else if apv < bpv {
                -1
            } else {
                1
            }
        }
    }
}

/// 升级状态（前端展示）
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct KernelState {
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub node_ready: bool,
    pub kernel_ready: bool,
    pub running: bool,
    pub port: Option<u16>,
    pub downloading: bool,
    pub progress: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths_at(root: &Path) -> Paths {
        Paths {
            root: root.to_path_buf(),
            node_dir: root.join("runtime"),
            agent_dir: root.join("agent"),
            staging_dir: root.join("staging"),
            backup_dir: root.join("previous"),
            settings_file: root.join("settings.json"),
            log_dir: root.join("logs"),
        }
    }

    fn write_profile(root: &Path, manifest: &str) -> std::path::PathBuf {
        let dir = root.join("dsh-home/profiles/web");
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        let file = dir.join("package.json");
        std::fs::write(&file, manifest).unwrap();
        file
    }

    #[test]
    fn prune_removes_dangling_link_dependency() {
        let root = std::env::temp_dir().join(format!("dshdesk-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let manifest = write_profile(
            &root,
            r#"{"dependencies":{"dshmarket":"^1.17.1","gone":"link:/definitely/missing/dir"},
                "dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base","dshmarket","gone"]}}}"#,
        );

        let pruned = prune_missing_profile_bundles(&paths_at(&root));
        assert_eq!(pruned, vec!["gone".to_string()]);

        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
        let deps = json["dependencies"].as_object().unwrap();
        assert!(deps.contains_key("dshmarket"));
        assert!(!deps.contains_key("gone"));
        let bundles = json["dsh"]["profile"]["bundles"].as_array().unwrap();
        assert_eq!(bundles.len(), 2);
        assert!(!bundles.iter().any(|b| b == "gone"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prune_keeps_manifest_when_all_links_exist() {
        let root = std::env::temp_dir().join(format!("dshdesk-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let local = root.join("local-plugin");
        std::fs::create_dir_all(&local).unwrap();
        let manifest = write_profile(
            &root,
            &format!(
                r#"{{"dependencies":{{"here":"link:{}"}},
                    "dsh":{{"profile":{{"bundles":["here"]}}}}}}"#,
                local.display()
            ),
        );
        let before = std::fs::read_to_string(&manifest).unwrap();

        assert!(prune_missing_profile_bundles(&paths_at(&root)).is_empty());
        assert_eq!(std::fs::read_to_string(&manifest).unwrap(), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prune_is_noop_without_profile() {
        let root = std::env::temp_dir().join(format!("dshdesk-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert!(prune_missing_profile_bundles(&paths_at(&root)).is_empty());
    }
}

#[cfg(test)]
mod health_tests {
    use super::*;

    fn temp_paths() -> (tempdir_lite::TempDir, Paths) {
        let dir = tempdir_lite::TempDir::new();
        let root = dir.path().to_path_buf();
        let paths = Paths {
            agent_dir: root.join("agent"),
            staging_dir: root.join("agent-staging"),
            backup_dir: root.join("agent-previous"),
            node_dir: root.join("runtime"),
            settings_file: root.join("settings.json"),
            log_dir: root.join("logs"),
            root,
        };
        (dir, paths)
    }

    fn write_kernel(paths: &Paths, deps: &str, create_dep_dirs: &[&str]) {
        let pkg_dir = paths.agent_dir.join("node_modules").join(PKG);
        std::fs::create_dir_all(pkg_dir.join("lib")).unwrap();
        std::fs::write(pkg_dir.join("lib").join("bin.js"), "//").unwrap();
        std::fs::write(
            pkg_dir.join("package.json"),
            format!(r#"{{"name":"{PKG}","version":"1.0.0","dependencies":{deps}}}"#),
        )
        .unwrap();
        for dep in create_dep_dirs {
            std::fs::create_dir_all(paths.agent_dir.join("node_modules").join(dep)).unwrap();
        }
    }

    #[test]
    fn healthy_requires_entry_file() {
        let (_dir, paths) = temp_paths();
        assert!(!kernel_healthy(&paths));
    }

    #[test]
    fn healthy_when_all_dependencies_present() {
        let (_dir, paths) = temp_paths();
        write_kernel(&paths, r#"{"left-pad":"^1.0.0"}"#, &["left-pad"]);
        assert!(kernel_healthy(&paths));
    }

    #[test]
    fn unhealthy_when_dependency_missing() {
        let (_dir, paths) = temp_paths();
        write_kernel(&paths, r#"{"left-pad":"^1.0.0"}"#, &[]);
        assert!(!kernel_healthy(&paths));
    }

    #[test]
    fn purge_removes_agent_and_staging() {
        let (_dir, paths) = temp_paths();
        write_kernel(&paths, "{}", &[]);
        std::fs::create_dir_all(&paths.staging_dir).unwrap();
        purge_kernel(&paths).unwrap();
        assert!(!paths.agent_dir.exists());
        assert!(!paths.staging_dir.exists());
    }

    /// Minimal scratch-directory helper so tests do not need a new dependency.
    mod tempdir_lite {
        pub struct TempDir(std::path::PathBuf);

        impl TempDir {
            pub fn new() -> Self {
                let base = std::env::temp_dir().join(format!(
                    "dshdesk-kernel-test-{}-{:?}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&base).unwrap();
                Self(base)
            }
            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}

#[cfg(test)]
mod bundle_visibility_tests {
    use super::*;

    fn scratch(tag: &str) -> Paths {
        let root = std::env::temp_dir().join(format!(
            "dsh-bundle-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let profile = root.join("dsh-home/profiles/web");
        std::fs::create_dir_all(profile.join("node_modules")).unwrap();
        Paths {
            agent_dir: root.join("agent"),
            staging_dir: root.join("staging"),
            backup_dir: root.join("previous"),
            node_dir: root.join("runtime"),
            settings_file: root.join("settings.json"),
            log_dir: root.join("logs"),
            root,
        }
    }

    fn write_manifest(paths: &Paths, deps: &str, bundles: &str) {
        let file = paths.root.join("dsh-home/profiles/web/package.json");
        std::fs::write(
            &file,
            format!(r#"{{"dependencies":{deps},"dsh":{{"profile":{{"bundles":{bundles}}}}}}}"#),
        )
        .unwrap();
    }

    fn read_manifest(paths: &Paths) -> serde_json::Value {
        let file = paths.root.join("dsh-home/profiles/web/package.json");
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
    }

    #[test]
    fn local_bundle_is_pruned_when_its_directory_disappears() {
        // 桌面 shell 只登记在 bundles 里，靠依赖扫描发现不了；
        // 目录丢了必须清掉，否则 DSH 启动直接报 cannot resolve profile bundle。
        let paths = scratch("gone");
        write_manifest(&paths, "{}", r#"["@deepseek-ai/dsh-base","dsh-desktop-shell"]"#);

        let pruned = prune_missing_profile_bundles(&paths);
        assert_eq!(pruned, vec!["dsh-desktop-shell"]);

        let manifest = read_manifest(&paths);
        let bundles = manifest.pointer("/dsh/profile/bundles").unwrap().as_array().unwrap();
        assert_eq!(bundles.len(), 1, "只应留下官方 bundle");
        assert_eq!(bundles[0].as_str(), Some("@deepseek-ai/dsh-base"));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn present_local_bundle_survives_pruning() {
        let paths = scratch("present");
        write_manifest(&paths, "{}", r#"["dsh-desktop-shell"]"#);
        std::fs::create_dir_all(
            paths.root.join("dsh-home/profiles/web/node_modules/dsh-desktop-shell"),
        )
        .unwrap();

        assert!(prune_missing_profile_bundles(&paths).is_empty());
        let manifest = read_manifest(&paths);
        assert_eq!(manifest.pointer("/dsh/profile/bundles").unwrap().as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn official_bundles_are_never_pruned() {
        // 官方 bundle 由内核提供，不在 profile 的 node_modules 里，
        // 不能因为"目录不存在"就把它们清掉。
        let paths = scratch("official");
        write_manifest(
            &paths,
            "{}",
            r#"["@deepseek-ai/dsh-base","@deepseek-ai/dsh-web-app"]"#,
        );
        assert!(prune_missing_profile_bundles(&paths).is_empty());
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn market_installed_plugin_with_missing_link_is_pruned() {
        // 市场安装的插件写在 dependencies 里；link 目标消失时连带清掉 bundles。
        let paths = scratch("market");
        write_manifest(
            &paths,
            r#"{"some-plugin":"link:/definitely/not/here"}"#,
            r#"["some-plugin"]"#,
        );
        let pruned = prune_missing_profile_bundles(&paths);
        assert_eq!(pruned, vec!["some-plugin"]);
        let manifest = read_manifest(&paths);
        assert!(manifest["dependencies"].as_object().unwrap().is_empty());
        assert!(manifest.pointer("/dsh/profile/bundles").unwrap().as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn registry_installed_plugin_is_left_alone() {
        // 版本号规格（非 link:）的依赖由 pnpm 管理，不该被我们碰。
        let paths = scratch("registry");
        write_manifest(&paths, r#"{"dshmarket":"^1.21.0"}"#, r#"["dshmarket"]"#);
        std::fs::create_dir_all(paths.root.join("dsh-home/profiles/web/node_modules/dshmarket"))
            .unwrap();
        assert!(prune_missing_profile_bundles(&paths).is_empty());
        let manifest = read_manifest(&paths);
        assert_eq!(manifest["dependencies"]["dshmarket"].as_str(), Some("^1.21.0"));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    /// 在 node_modules 里放一个带版本号的包，模拟"已装好"。
    fn install_package(paths: &Paths, name: &str, version: Option<&str>) {
        let dir = paths.root.join("dsh-home/profiles/web/node_modules").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let body = match version {
            Some(version) => format!(r#"{{"name":"{name}","version":"{version}"}}"#),
            None => format!(r#"{{"name":"{name}"}}"#),
        };
        std::fs::write(dir.join("package.json"), body).unwrap();
    }

    #[test]
    fn half_installed_market_plugin_is_registered_again() {
        // 市场安装失败会回滚 dependencies，却留下 node_modules 目录和 bundles 条目。
        // 插件仍在工作，但市场再也看不见它——必须补登记，否则无法更新或卸载。
        let paths = scratch("halfinstalled");
        write_manifest(&paths, r#"{"dshmarket":"^1.36.0"}"#, r#"["dshmarket","dsh-file-upload"]"#);
        install_package(&paths, "dshmarket", Some("1.36.0"));
        install_package(&paths, "dsh-file-upload", Some("0.4.3"));

        let restored = restore_unregistered_profile_plugins(&paths);
        assert_eq!(restored, vec!["dsh-file-upload"]);

        let manifest = read_manifest(&paths);
        assert_eq!(
            manifest["dependencies"]["dsh-file-upload"].as_str(),
            Some("^0.4.3"),
            "版本号要取自包自己的 package.json，写法与市场一致"
        );
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn local_shell_is_never_registered() {
        // 这条是用户的硬要求：本地桌面插件永远不出现在市场「已安装」里。
        let paths = scratch("localshell");
        write_manifest(&paths, "{}", r#"["dsh-desktop-shell"]"#);
        install_package(&paths, LOCAL_SHELL_PACKAGE, Some("0.1.0"));

        assert!(restore_unregistered_profile_plugins(&paths).is_empty());
        let manifest = read_manifest(&paths);
        assert!(
            manifest["dependencies"].as_object().unwrap().is_empty(),
            "本地 shell 不能被写进 dependencies"
        );
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn official_bundles_are_never_registered() {
        // 官方 bundle 不在 profile 的 node_modules 里，市场也主动滤掉它们。
        let paths = scratch("officialreg");
        write_manifest(&paths, "{}", r#"["@deepseek-ai/dsh-base","@deepseek-ai/dsh-web-app"]"#);
        assert!(restore_unregistered_profile_plugins(&paths).is_empty());
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn already_registered_plugin_keeps_its_original_spec() {
        // 已登记的插件不能被改写：用户或市场可能钉了具体版本范围。
        let paths = scratch("keepspec");
        write_manifest(&paths, r#"{"dsh-file-upload":"~0.4.0"}"#, r#"["dsh-file-upload"]"#);
        install_package(&paths, "dsh-file-upload", Some("0.4.3"));

        assert!(restore_unregistered_profile_plugins(&paths).is_empty());
        let manifest = read_manifest(&paths);
        assert_eq!(manifest["dependencies"]["dsh-file-upload"].as_str(), Some("~0.4.0"));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn bundle_without_directory_is_not_registered() {
        // 目录不存在说明不是"半安装"，该交给 prune 清 bundles，
        // 这里凭空造依赖只会让 pnpm 下次安装报错。
        let paths = scratch("nodir");
        write_manifest(&paths, "{}", r#"["ghost-plugin"]"#);
        assert!(restore_unregistered_profile_plugins(&paths).is_empty());
        let manifest = read_manifest(&paths);
        assert!(manifest["dependencies"].as_object().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn package_without_version_falls_back_to_any() {
        // 读不出版本也要登记：继续隐身意味着永远无法通过市场更新。
        let paths = scratch("noversion");
        write_manifest(&paths, "{}", r#"["odd-plugin"]"#);
        install_package(&paths, "odd-plugin", None);

        assert_eq!(restore_unregistered_profile_plugins(&paths), vec!["odd-plugin"]);
        let manifest = read_manifest(&paths);
        assert_eq!(manifest["dependencies"]["odd-plugin"].as_str(), Some("*"));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn prune_then_restore_leaves_a_consistent_manifest() {
        // 两个函数按启动顺序连跑：prune 摘掉目录已消失的条目，
        // restore 只补真正装着的那个，不该把刚摘掉的又写回来。
        let paths = scratch("pipeline");
        write_manifest(
            &paths,
            "{}",
            r#"["@deepseek-ai/dsh-base","dsh-desktop-shell","dsh-file-upload","ghost-plugin"]"#,
        );
        install_package(&paths, LOCAL_SHELL_PACKAGE, Some("0.1.0"));
        install_package(&paths, "dsh-file-upload", Some("0.4.3"));

        let pruned = prune_missing_profile_bundles(&paths);
        assert_eq!(pruned, vec!["ghost-plugin"]);
        let restored = restore_unregistered_profile_plugins(&paths);
        assert_eq!(restored, vec!["dsh-file-upload"]);

        let manifest = read_manifest(&paths);
        let deps = manifest["dependencies"].as_object().unwrap();
        assert_eq!(deps.len(), 1, "只有市场插件被登记");
        assert!(deps.contains_key("dsh-file-upload"));
        let bundles: Vec<&str> = manifest
            .pointer("/dsh/profile/bundles")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(bundles, vec!["@deepseek-ai/dsh-base", "dsh-desktop-shell", "dsh-file-upload"]);
        let _ = std::fs::remove_dir_all(&paths.root);
    }
}

#[cfg(test)]
mod build_allowlist_tests {
    use super::*;

    fn scratch(tag: &str) -> Paths {
        let root = std::env::temp_dir().join(format!(
            "dsh-allow-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(root.join("dsh-home/profiles/web")).unwrap();
        Paths {
            agent_dir: root.join("agent"),
            staging_dir: root.join("staging"),
            backup_dir: root.join("previous"),
            node_dir: root.join("runtime"),
            settings_file: root.join("settings.json"),
            log_dir: root.join("logs"),
            root,
        }
    }

    fn workspace(paths: &Paths) -> std::path::PathBuf {
        paths.root.join("dsh-home/profiles/web/pnpm-workspace.yaml")
    }

    #[test]
    fn placeholder_lines_become_true() {
        // pnpm 写入的占位行必须原地替换，否则安装继续中止。
        let paths = scratch("placeholder");
        std::fs::write(
            workspace(&paths),
            "packages:\n  - .\n\nnodeLinker: hoisted\nallowBuilds:\n  sharp: set this to true or false\n  tesseract.js: set this to true or false\n",
        )
        .unwrap();

        allow_plugin_build_scripts(&paths);

        let text = std::fs::read_to_string(workspace(&paths)).unwrap();
        assert!(text.contains("sharp: true"), "实际:\n{text}");
        assert!(text.contains("tesseract.js: true"), "实际:\n{text}");
        assert!(!text.contains("set this to true or false"));
        // 不能破坏其他配置项。
        assert!(text.contains("nodeLinker: hoisted"));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn allowlist_section_is_created_when_absent() {
        let paths = scratch("create");
        std::fs::write(workspace(&paths), "packages:\n  - .\n\nnodeLinker: hoisted\n").unwrap();

        allow_plugin_build_scripts(&paths);

        let text = std::fs::read_to_string(workspace(&paths)).unwrap();
        assert!(text.contains("allowBuilds:"), "实际:\n{text}");
        assert!(text.contains("sharp: true"));
        assert!(text.contains("tesseract.js: true"));
        assert!(text.contains("nodeLinker: hoisted"));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn missing_entries_are_added_to_existing_section() {
        // 只有一项被 pnpm 记录时，另一项也要补上。
        let paths = scratch("partial");
        std::fs::write(
            workspace(&paths),
            "packages:\n  - .\n\nallowBuilds:\n  sharp: true\n",
        )
        .unwrap();

        allow_plugin_build_scripts(&paths);

        let text = std::fs::read_to_string(workspace(&paths)).unwrap();
        assert!(text.contains("tesseract.js: true"), "实际:\n{text}");
        // 已有项不应重复。
        assert_eq!(text.matches("sharp: true").count(), 1, "实际:\n{text}");
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn already_allowed_file_is_left_untouched() {
        let paths = scratch("noop");
        let original = "packages:\n  - .\n\nallowBuilds:\n  sharp: true\n  tesseract.js: true\n";
        std::fs::write(workspace(&paths), original).unwrap();

        allow_plugin_build_scripts(&paths);

        assert_eq!(std::fs::read_to_string(workspace(&paths)).unwrap(), original);
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn missing_workspace_file_is_not_created() {
        // profile 还没初始化时不该凭空造出配置文件。
        let paths = scratch("absent");
        allow_plugin_build_scripts(&paths);
        assert!(!workspace(&paths).exists());
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn unknown_pending_dependency_is_also_approved() {
        // 带全新原生依赖的插件：pnpm 为它写下占位行后中止安装。
        // 只放开预设的两项不够，否则用户永远装不上这个插件。
        let paths = scratch("unknown");
        std::fs::write(
            workspace(&paths),
            "packages:\n  - .\n\nallowBuilds:\n  sharp: true\n  tesseract.js: true\n  better-sqlite3: set this to true or false\n",
        )
        .unwrap();

        assert!(allow_plugin_build_scripts(&paths));

        let text = std::fs::read_to_string(workspace(&paths)).unwrap();
        assert!(text.contains("better-sqlite3: true"), "实际:\n{text}");
        assert!(!text.contains("set this to true or false"), "实际:\n{text}");
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn several_pending_dependencies_are_approved_at_once() {
        let paths = scratch("multi");
        std::fs::write(
            workspace(&paths),
            "packages:\n  - .\n\nallowBuilds:\n  esbuild: set this to true or false\n  node-pty: set this to true or false\n",
        )
        .unwrap();

        assert!(allow_plugin_build_scripts(&paths));

        let text = std::fs::read_to_string(workspace(&paths)).unwrap();
        assert!(text.contains("esbuild: true"), "实际:\n{text}");
        assert!(text.contains("node-pty: true"), "实际:\n{text}");
        // 预设项也要在。
        assert!(text.contains("sharp: true"));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn unrelated_settings_and_indentation_survive() {
        // 用户或 pnpm 可能改过其他配置项，改写不能破坏它们。
        let paths = scratch("preserve");
        let original = "packages:\n  - .\n\nnodeLinker: hoisted\nautoInstallPeers: false\nallowBuilds:\n  sharp: set this to true or false\n  tesseract.js: true\n";
        std::fs::write(workspace(&paths), original).unwrap();

        assert!(allow_plugin_build_scripts(&paths));

        let text = std::fs::read_to_string(workspace(&paths)).unwrap();
        assert!(text.contains("nodeLinker: hoisted"), "实际:\n{text}");
        assert!(text.contains("autoInstallPeers: false"), "实际:\n{text}");
        assert!(text.contains("  sharp: true"), "缩进要保持两空格:\n{text}");
        assert_eq!(text.matches("tesseract.js").count(), 1, "不应重复插入:\n{text}");
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn return_value_reports_whether_the_file_changed() {
        // 返回值决定日志是否记一行"已放开构建脚本"。每次启动都会调用它，
        // 若无改动也返回 true，日志会被同一行刷满。
        let paths = scratch("changed");
        std::fs::write(
            workspace(&paths),
            "packages:\n  - .\n\nallowBuilds:\n  sharp: set this to true or false\n  tesseract.js: true\n",
        )
        .unwrap();

        assert!(allow_plugin_build_scripts(&paths), "首次替换占位行应报告已改动");
        assert!(!allow_plugin_build_scripts(&paths), "已是 true 时不应再报告改动");
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn placeholder_written_after_kernel_install_is_still_fixed() {
        // 这是 Windows 上插件装不上的真实时序：装内核时文件里还没有 allowBuilds
        // 段，等用户第一次装带 sharp 的插件，pnpm 才写下占位行并中止安装。
        // 若只在装内核时放开一次，占位行就永久留下了。
        let paths = scratch("timeline");
        // 第一步：装内核时，profile 刚建好，只有基础配置。
        std::fs::write(workspace(&paths), "packages:\n  - .\n\nnodeLinker: hoisted\n").unwrap();
        assert!(allow_plugin_build_scripts(&paths));

        // 第二步：pnpm 用自己的格式重写了文件，占位行出现。
        std::fs::write(
            workspace(&paths),
            "packages:\n  - .\n\nnodeLinker: hoisted\nallowBuilds:\n  sharp: set this to true or false\n",
        )
        .unwrap();

        // 第三步：下次启动内核前再过一遍，占位行必须被改掉。
        assert!(allow_plugin_build_scripts(&paths));
        let text = std::fs::read_to_string(workspace(&paths)).unwrap();
        assert!(text.contains("sharp: true"), "实际:\n{text}");
        assert!(!text.contains("set this to true or false"), "实际:\n{text}");
        let _ = std::fs::remove_dir_all(&paths.root);
    }
}

#[cfg(test)]
mod install_progress_tests {
    use super::*;

    /// 按顺序喂若干行，返回最后一次上报的进度。
    fn feed(lines: &[&str]) -> Option<InstallPhase> {
        let mut resolved = 0;
        let mut imported = 0;
        let mut total = None;
        let mut last = None;
        for line in lines {
            if let Some(phase) = parse_pnpm_progress(line, &mut resolved, &mut imported, &mut total) {
                last = Some(phase);
            }
        }
        last
    }

    fn resolved_line(id: &str) -> String {
        format!(r#"{{"name":"pnpm:progress","packageId":"{id}","status":"resolved"}}"#)
    }

    fn imported_line(id: &str) -> String {
        format!(r#"{{"name":"pnpm:progress","packageId":"{id}","status":"imported"}}"#)
    }

    const RESOLUTION_DONE: &str = r#"{"name":"pnpm:stage","stage":"resolution_done"}"#;

    #[test]
    fn resolved_events_count_up_during_resolution() {
        let phase = feed(&[&resolved_line("a@1"), &resolved_line("b@1")]);
        assert_eq!(phase, Some(InstallPhase::Resolving { resolved: 2 }));
    }

    #[test]
    fn resolution_done_fixes_the_total() {
        let phase = feed(&[&resolved_line("a@1"), &resolved_line("b@1"), RESOLUTION_DONE]);
        assert_eq!(phase, Some(InstallPhase::Importing { imported: 0, total: 2 }));
    }

    #[test]
    fn imported_events_advance_against_the_total() {
        let phase = feed(&[
            &resolved_line("a@1"),
            &resolved_line("b@1"),
            RESOLUTION_DONE,
            &imported_line("a@1"),
        ]);
        assert_eq!(phase, Some(InstallPhase::Importing { imported: 1, total: 2 }));
    }

    #[test]
    fn noise_lines_are_ignored() {
        // ndjson 里九成的行与进度无关，不能让它们扰动计数。
        assert_eq!(feed(&[r#"{"name":"pnpm:context","storeDir":"/tmp"}"#]), None);
        assert_eq!(feed(&[r#"{"name":"pnpm:_dependency_resolved"}"#]), None);
        assert_eq!(feed(&["not json at all"]), None);
        assert_eq!(feed(&[""]), None);
    }

    #[test]
    fn late_resolved_events_do_not_revert_to_resolving() {
        // 解析阶段结束后仍会零星出现 resolved；此时已在下载，
        // 若退回"正在解析依赖"，用户会看到进度条倒退。
        let phase = feed(&[
            &resolved_line("a@1"),
            RESOLUTION_DONE,
            &imported_line("a@1"),
            &resolved_line("late@1"),
        ]);
        assert_eq!(phase, Some(InstallPhase::Importing { imported: 1, total: 1 }));
    }

    #[test]
    fn importing_without_resolution_done_still_has_a_denominator() {
        // 万一 pnpm 没发 resolution_done，也不能出现除以零。
        let phase = feed(&[&resolved_line("a@1"), &imported_line("a@1")]);
        assert_eq!(phase, Some(InstallPhase::Importing { imported: 1, total: 1 }));
    }

    #[test]
    fn importing_done_reports_finishing() {
        let phase = feed(&[RESOLUTION_DONE, r#"{"name":"pnpm:stage","stage":"importing_done"}"#]);
        assert_eq!(phase, Some(InstallPhase::Finishing));
    }

    #[test]
    fn ratio_never_exceeds_its_phase_budget() {
        // 解析阶段总数未知，靠趋近曲线推进，必须收敛在 35% 以内。
        assert!(InstallPhase::Resolving { resolved: 0 }.ratio() < 0.01);
        assert!(InstallPhase::Resolving { resolved: 10_000 }.ratio() <= 0.35);
        // 下载阶段占 35%..95%。
        assert!((InstallPhase::Importing { imported: 0, total: 100 }.ratio() - 0.35).abs() < 1e-9);
        assert!((InstallPhase::Importing { imported: 100, total: 100 }.ratio() - 0.95).abs() < 1e-9);
        assert!(InstallPhase::Finishing.ratio() < 1.0);
    }

    #[test]
    fn ratio_is_monotonic_along_a_real_sequence() {
        // 真实安装序列里进度必须单调不降，否则进度条会抖。
        let mut resolved = 0;
        let mut imported = 0;
        let mut total = None;
        let mut lines: Vec<String> = (0..50).map(|i| resolved_line(&format!("p{i}@1"))).collect();
        lines.push(RESOLUTION_DONE.to_string());
        lines.extend((0..50).map(|i| imported_line(&format!("p{i}@1"))));
        lines.push(r#"{"name":"pnpm:stage","stage":"importing_done"}"#.to_string());

        let mut previous = 0.0;
        for line in &lines {
            if let Some(phase) = parse_pnpm_progress(line, &mut resolved, &mut imported, &mut total) {
                let ratio = phase.ratio();
                assert!(ratio >= previous - 1e-9, "进度回退了：{previous} -> {ratio}");
                previous = ratio;
            }
        }
        assert!(previous > 0.9, "序列结束时应接近完成，实际 {previous}");
    }

    #[test]
    fn zero_total_does_not_divide_by_zero() {
        let ratio = InstallPhase::Importing { imported: 0, total: 0 }.ratio();
        assert!(ratio.is_finite());
        assert!((ratio - 0.35).abs() < 1e-9);
    }

    /// 前端进度条按 `width: <percent>%` 渲染，拿到的必须是 0..100。
    ///
    /// `ratio()` 返回的是 0..1，发事件时要乘 100（见 commands::make_log_cb）。
    /// 漏掉这一步不会报错，只会让进度条永远卡在 1% 以内——看起来就像卡死，
    /// 而这正是用户报告"点更新没有进度条"时最容易复发的写法。
    #[test]
    fn emitted_percent_spans_the_full_scale() {
        let percent = |phase: InstallPhase| phase.ratio() * 100.0;
        assert!(percent(InstallPhase::Resolving { resolved: 0 }) < 1.0);
        let mid = percent(InstallPhase::Importing { imported: 50, total: 100 });
        assert!((60.0..=70.0).contains(&mid), "中段应在 60~70%，实际 {mid}");
        let end = percent(InstallPhase::Finishing);
        assert!((90.0..=100.0).contains(&end), "收尾应接近 100%，实际 {end}");
    }

    #[test]
    fn labels_carry_the_counts_users_see() {
        assert_eq!(InstallPhase::Resolving { resolved: 0 }.label(), "正在解析依赖");
        assert_eq!(InstallPhase::Resolving { resolved: 12 }.label(), "正在解析依赖（12 个）");
        assert_eq!(
            InstallPhase::Importing { imported: 3, total: 40 }.label(),
            "正在安装依赖（3/40）"
        );
        assert_eq!(InstallPhase::Finishing.label(), "正在校验内核文件");
    }
}

/// 仅用于手工验证真实 pnpm 输出的进度序列（`cargo test -- --ignored live_`）。
#[cfg(test)]
mod live_progress_probe {
    use super::*;

    #[test]
    #[ignore = "需要网络与内置 node，手工运行以核对真实进度序列"]
    fn live_install_reports_monotonic_progress() {
        let paths = Paths::resolve();
        let node = crate::runtime::node_bin(&paths);
        assert!(node.exists(), "内置 node 不存在，先完成初始化");
        let staging = std::env::temp_dir().join(format!("dsh-live-progress-{}", std::process::id()));

        let samples: Arc<std::sync::Mutex<Vec<(f64, String)>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = samples.clone();
        let callback = Some(Arc::new(move |phase: InstallPhase| {
            sink.lock().unwrap().push((phase.ratio(), phase.label()));
        }) as Arc<dyn Fn(InstallPhase) + Send + Sync>);

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(install_kernel_to(
            &node,
            &staging,
            "https://repo.huaweicloud.com/repository/npm/",
            "0.1.1-rc.2",
            callback,
        ));
        let _ = std::fs::remove_dir_all(&staging);
        result.expect("安装应成功");

        let samples = samples.lock().unwrap();
        assert!(samples.len() > 50, "回调次数太少（{}），说明没有流式读取", samples.len());
        let mut previous = 0.0;
        for (ratio, _) in samples.iter() {
            assert!(*ratio >= previous - 1e-9, "进度回退：{previous} -> {ratio}");
            previous = *ratio;
        }
        println!("回调 {} 次，首个 {:.3}，末个 {:.3}", samples.len(), samples[0].0, previous);
        println!("末尾文案：{}", samples.last().unwrap().1);
        assert!(previous > 0.9, "结束时应接近完成，实际 {previous}");
    }
}

#[cfg(test)]
mod market_compat_tests {
    use super::*;

    fn scratch(tag: &str) -> Paths {
        let root = std::env::temp_dir().join(format!(
            "dsh-market-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(root.join("dsh-home/profiles/web/node_modules/dshmarket/lib")).unwrap();
        Paths {
            agent_dir: root.join("agent"),
            staging_dir: root.join("staging"),
            backup_dir: root.join("previous"),
            node_dir: root.join("runtime"),
            settings_file: root.join("settings.json"),
            log_dir: root.join("logs"),
            root,
        }
    }

    fn write_settings(paths: &Paths, body: &str) {
        let file = paths
            .root
            .join("dsh-home/profiles/web/node_modules/dshmarket/lib/settings.js");
        std::fs::write(file, body).unwrap();
    }

    /// dshmarket ≤1.38.0 的真实写法：内核 0.1.2 已删掉这个导出，
    /// 具名导入缺失会在模块求值期抛 SyntaxError，内核直接退出 1。
    #[test]
    fn old_market_with_the_removed_import_is_detected() {
        let paths = scratch("old");
        write_settings(
            &paths,
            "import { installSettingsSection, settingsNamespace } from '@deepseek-ai/dsh-settings';\n",
        );
        assert!(market_breaks_kernel_boot(&paths));
    }

    /// 1.39.0 修好了，但在注释里保留了整段说明，其中原样引用了这个符号名。
    /// 用裸的子串匹配会把已修复的版本误判成坏版本，于是每次启动都去"修"
    /// 一个没坏的东西，甚至最终把好端端的市场停用掉。
    #[test]
    fn fixed_market_mentioning_it_in_comments_is_not_flagged() {
        let paths = scratch("fixed");
        write_settings(
            &paths,
            " * This module used to import two convenience helpers,\n\
             * `installSettingsSection` and `settingsNamespace`, from\n\
             * `@deepseek-ai/dsh-settings`. dsh 0.1.2-alpha.1 deleted both.\n\
             *   SyntaxError: The requested module does not\n\
             *   provide an export named 'installSettingsSection'\n\
             import z from '@deepseek-ai/schemastery';\n",
        );
        assert!(!market_breaks_kernel_boot(&paths));
    }

    #[test]
    fn missing_market_is_not_a_problem() {
        let paths = scratch("absent");
        assert!(!market_breaks_kernel_boot(&paths));
    }

    /// 停用要同时摘掉 dependencies 和 bundles：只摘一处内核仍会去加载它。
    #[test]
    fn disabling_removes_both_registrations() {
        let paths = scratch("disable");
        let manifest = paths.root.join("dsh-home/profiles/web/package.json");
        std::fs::write(
            &manifest,
            r#"{"dependencies":{"dshmarket":"^1.38.0","dsh-file-upload":"^0.4.3"},
                "dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base","dshmarket","dsh-desktop-shell"]}}}"#,
        )
        .unwrap();

        assert!(disable_market(&paths));

        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
        let deps = json.get("dependencies").unwrap().as_object().unwrap();
        assert!(!deps.contains_key("dshmarket"));
        assert!(deps.contains_key("dsh-file-upload"), "不能误伤其他插件");
        let bundles = json.pointer("/dsh/profile/bundles").unwrap().as_array().unwrap();
        assert!(!bundles.iter().any(|v| v.as_str() == Some("dshmarket")));
        assert_eq!(bundles.len(), 2, "其余 bundle 必须原样保留");

        // 幂等：已经摘干净了就不该再报告改动，否则日志会每次启动都刷一条。
        assert!(!disable_market(&paths));
    }
}

#[cfg(test)]
mod file_upload_collision_tests {
    use super::*;

    fn scratch(tag: &str) -> Paths {
        let root = std::env::temp_dir().join(format!(
            "dsh-upload-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("dsh-home/profiles/web")).unwrap();
        std::fs::create_dir_all(root.join("agent/node_modules/@deepseek-ai/dsh-web-app")).unwrap();
        Paths {
            agent_dir: root.join("agent"),
            staging_dir: root.join("staging"),
            backup_dir: root.join("previous"),
            node_dir: root.join("runtime"),
            settings_file: root.join("settings.json"),
            log_dir: root.join("logs"),
            root,
        }
    }

    fn write_web_app_patch(paths: &Paths, body: &str) {
        std::fs::write(
            paths
                .agent_dir
                .join("node_modules/@deepseek-ai/dsh-web-app/cordis.patch.yml"),
            body,
        )
        .unwrap();
    }

    fn write_profile(paths: &Paths, body: &str) {
        std::fs::write(
            paths.root.join("dsh-home/profiles/web/package.json"),
            body,
        )
        .unwrap();
    }

    /// 0.1.5-rc.1 的真实写法：web-app patch 插入官方 file-upload。
    #[test]
    fn kernel_patch_with_file_upload_id_is_detected() {
        let paths = scratch("kernel-owns");
        write_web_app_patch(
            &paths,
            "    # Raw Blob and ReadableStream uploads\n    - id: file-upload\n      name: '@deepseek-ai/dsh-client-file-upload'\n",
        );
        assert!(kernel_owns_file_upload(&paths));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    /// 注释里提到这个 id 不能算内核占用了槽。
    #[test]
    fn commented_file_upload_id_is_ignored() {
        let paths = scratch("comment");
        write_web_app_patch(&paths, "    # - id: file-upload\n    - id: ui-layout\n");
        assert!(!kernel_owns_file_upload(&paths));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    /// 旧内核没有这条 patch，社区插件可以继续用。
    #[test]
    fn missing_web_app_patch_means_kernel_does_not_own_the_slot() {
        let paths = scratch("old-kernel");
        assert!(!kernel_owns_file_upload(&paths));
        assert!(!file_upload_plugin_collides(&paths));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    /// 内核占了槽、profile 还装着社区插件 = 启动必炸。
    #[test]
    fn collision_requires_both_kernel_and_community_plugin() {
        let paths = scratch("collide");
        write_web_app_patch(&paths, "- id: file-upload\n  name: '@deepseek-ai/dsh-client-file-upload'\n");
        write_profile(
            &paths,
            r#"{"dependencies":{"dsh-file-upload":"^0.4.3"},"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-web-app","dsh-file-upload"]}}}"#,
        );
        assert!(file_upload_plugin_collides(&paths));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    /// 停用要同时摘 dependencies 和 bundles，不能误伤市场。
    #[test]
    fn disabling_community_plugin_removes_both_registrations() {
        let paths = scratch("disable");
        write_profile(
            &paths,
            r#"{"dependencies":{"dshmarket":"^1.39.0","dsh-file-upload":"^0.4.3"},"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base","dshmarket","dsh-file-upload","dsh-desktop-shell"]}}}"#,
        );

        assert!(disable_community_file_upload(&paths));

        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(paths.root.join("dsh-home/profiles/web/package.json")).unwrap(),
        )
        .unwrap();
        let deps = json.get("dependencies").unwrap().as_object().unwrap();
        assert!(!deps.contains_key("dsh-file-upload"));
        assert!(deps.contains_key("dshmarket"), "不能误伤市场");
        let bundles = json.pointer("/dsh/profile/bundles").unwrap().as_array().unwrap();
        assert!(!bundles.iter().any(|v| v.as_str() == Some("dsh-file-upload")));
        assert_eq!(bundles.len(), 3);

        assert!(!disable_community_file_upload(&paths));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    /// 内核已经占槽时，restore 绝不能把社区插件再写回 dependencies。
    /// 否则 spawn 先 disable、后 restore（或下次启动 restore 在 disable 之前）
    /// 会立刻把冲突装回来。
    #[test]
    fn restore_does_not_re_register_colliding_file_upload() {
        let paths = scratch("norestore");
        write_web_app_patch(&paths, "- id: file-upload\n");
        write_profile(
            &paths,
            r#"{"dependencies":{},"dsh":{"profile":{"bundles":["dsh-file-upload"]}}}"#,
        );
        std::fs::create_dir_all(
            paths
                .root
                .join("dsh-home/profiles/web/node_modules/dsh-file-upload"),
        )
        .unwrap();
        std::fs::write(
            paths
                .root
                .join("dsh-home/profiles/web/node_modules/dsh-file-upload/package.json"),
            r#"{"name":"dsh-file-upload","version":"0.4.3"}"#,
        )
        .unwrap();

        assert!(restore_unregistered_profile_plugins(&paths).is_empty());
        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(paths.root.join("dsh-home/profiles/web/package.json")).unwrap(),
        )
        .unwrap();
        assert!(
            json.get("dependencies")
                .and_then(|v| v.as_object())
                .map(|deps| !deps.contains_key("dsh-file-upload"))
                .unwrap_or(true)
        );
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    /// 旧内核没占槽时，半安装的社区插件仍应被补登记——那是市场能看见它的唯一办法。
    #[test]
    fn restore_still_registers_file_upload_on_old_kernel() {
        let paths = scratch("oldrestore");
        write_profile(
            &paths,
            r#"{"dependencies":{},"dsh":{"profile":{"bundles":["dsh-file-upload"]}}}"#,
        );
        std::fs::create_dir_all(
            paths
                .root
                .join("dsh-home/profiles/web/node_modules/dsh-file-upload"),
        )
        .unwrap();
        std::fs::write(
            paths
                .root
                .join("dsh-home/profiles/web/node_modules/dsh-file-upload/package.json"),
            r#"{"name":"dsh-file-upload","version":"0.4.3"}"#,
        )
        .unwrap();

        assert_eq!(
            restore_unregistered_profile_plugins(&paths),
            vec!["dsh-file-upload"]
        );
        let _ = std::fs::remove_dir_all(&paths.root);
    }
}
