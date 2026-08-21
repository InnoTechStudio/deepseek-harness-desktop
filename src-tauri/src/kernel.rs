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

/// 用内置 node 跑 pnpm 安装指定版本到 staging（全新目录，失败零影响）。
/// pnpm 比 npm 快得多（实测 dsh 内核 5s 装完，npm 会卡住）。
/// progress 可选：回调安装进度文本。
pub async fn install_kernel_to(
    node_exe: &Path,
    staging: &Path,
    registry: &str,
    version: &str,
    on_log: Option<Arc<dyn Fn(String) + Send + Sync>>,
) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging).map_err(|e| e.to_string())?;

    let pkg_spec = format!("{PKG}@{version}");

    // 用内置 node 运行 pnpm。node 官方包不自带 pnpm，先确保装好：
    // 用 node 直接跑 npm-cli.js 安装 pnpm（避免 npm shebang 依赖 PATH）
    let node_dir = node_exe.parent().unwrap();
    let pnpm = node_dir.join(if cfg!(target_os = "windows") { "pnpm.cmd" } else { "pnpm" });

    if !pnpm.exists() {
        // npm-cli 实际在 runtime/lib/node_modules/npm/bin/npm-cli.js（node bin 的上一级 lib）
        let node_root = node_dir.parent().unwrap_or(node_dir);
        let npm_cli = node_root.join("lib/node_modules/npm/bin/npm-cli.js");
        if !npm_cli.exists() {
            return Err(format!("内置 npm CLI 不存在：{}", npm_cli.display()));
        }
        let install = std::process::Command::new(node_exe)
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
    let pnpm_js = node_dir
        .parent()
        .unwrap()
        .join("lib/node_modules/pnpm/bin/pnpm.mjs");
    if !pnpm_js.exists() {
        return Err(format!("内置 pnpm 入口不存在：{}", pnpm_js.display()));
    }

    // 用 spawn_blocking 跑同步的 Command::output()：简单可靠，不依赖管道排空。
    let node_exe = node_exe.to_path_buf();
    let staging = staging.to_path_buf();
    let registry = registry.to_string();
    let pkg_spec = pkg_spec.to_string();
    let pnpm_js = pnpm_js.to_path_buf();
    let on_log_c = on_log.clone();
    let entry_check = staging.clone();

    let out = tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new(&node_exe);
        cmd.arg(&pnpm_js)
            .current_dir(&staging)
            .args([
                "add",
                &pkg_spec,
                // pnpm v11 用 --config.registry= 形式（两个独立参数会被当成布尔开关）
                &format!("--config.registry={}", registry),
                "--ignore-scripts",
            ])
            .env("COREPACK_NPM_REGISTRY", &registry)
            .env("npm_config_registry", &registry)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        cmd.output()
    })
    .await
    .map_err(|e| format!("安装线程失败: {e}"))?
    .map_err(|e| format!("无法启动 pnpm: {e}"))?;

    if !out.status.success() {
        let tail = String::from_utf8_lossy(&out.stderr)
            .lines()
            .chain(String::from_utf8_lossy(&out.stdout).lines())
            .filter(|l| !l.trim().is_empty())
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .join(" | ");
        return Err(format!("pnpm 安装失败（退出码 {:?}）：{}", out.status.code(), tail));
    }
    if let Some(cb) = &on_log_c {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            cb(line.to_string());
        }
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
            return (x - y) as i32;
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
