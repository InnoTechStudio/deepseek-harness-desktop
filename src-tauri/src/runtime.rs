//! Node runtime：下载（动态选源）→ 校验 → 解压 → 确认 node 可用
use crate::downloader::download_with_diagnostics;
use crate::mirrors::node_mirrors;
use crate::paths::Paths;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub use crate::downloader::DownloadProgress;
use std::sync::Arc;

pub const NODE_VERSION: &str = "v22.22.0";

fn node_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    }
}

/// Node 可执行文件路径
pub fn node_bin(paths: &Paths) -> PathBuf {
    if cfg!(target_os = "windows") {
        paths.node_dir.join("node.exe")
    } else {
        paths.node_dir.join("bin").join("node")
    }
}

/// 判断 Node 是否已就绪
pub fn node_ready(paths: &Paths) -> bool {
    let bin = node_bin(paths);
    if !bin.exists() {
        return false;
    }
    // 简单探测：node --version 快速返回
    let mut cmd = std::process::Command::new(&bin);
    let out = crate::dsh::suppress_console(&mut cmd)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    matches!(out, Ok(s) if s.success())
}

/// 缺失或损坏时自动重装 Node，而不是把错误甩给用户。
///
/// 换安装路径重装、数据目录探测失败、上次解压中断，都会让 `node_ready` 为假。
/// 引导、装内核、升级、启动服务都走这里，避免再出现「找不到 Node.js」硬失败。
pub async fn ensure_node_runtime(
    client: &reqwest::Client,
    paths: &Paths,
    cached_registry: Option<String>,
    on_progress: Option<Arc<dyn Fn(DownloadProgress) + Send + Sync>>,
) -> Result<(), String> {
    if node_ready(paths) {
        return Ok(());
    }
    crate::commands::log_to_file(paths, "ensure_node: 运行时缺失或不可用，正在重新安装");
    install_node_runtime(client, paths, cached_registry, on_progress).await?;
    Ok(())
}

fn sha256_hex(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// 从 mirror 下载并解压 Node runtime。
/// 返回解压后目录路径（= node_dir）。
pub async fn install_node_runtime(
    client: &reqwest::Client,
    paths: &Paths,
    cached_registry: Option<String>,
    on_progress: Option<Arc<dyn Fn(DownloadProgress) + Send + Sync>>,
) -> Result<String, String> {
    let platform = paths.platform();
    let arch = node_arch();
    let tmp = paths.root.join(format!("node-{}.dl", NODE_VERSION));
    let log = {
        let paths = paths.clone();
        Some(Arc::new(move |line: String| {
            crate::commands::log_to_file(&paths, &line);
        }) as Arc<dyn Fn(String) + Send + Sync>)
    };

    // 已有完整下载则直接解压，跳过选源与下载（避免每次都重传 48MB）。
    let cached_url = cached_registry.filter(|url| url.ends_with(&node_file_name(platform, arch)));
    if tmp.exists() {
        if let Some(url) = &cached_url {
            if verify_checksum(client, url, &tmp).await.is_ok() {
                return finish_install(paths, url, &tmp, &on_progress).await;
            }
        }
    }

    // 竞速：并发试下所有镜像的开头 2 MB，最快的胜出并保留已下载数据续传。
    let candidates: Vec<String> = node_mirrors(NODE_VERSION, platform, arch)
        .into_iter()
        .map(|m| m.base_url)
        .collect();
    let scratch_dir = paths.root.join("race");
    let outcome = crate::downloader::race_sources(client, &candidates, &scratch_dir, &log)
        .await
        .ok_or_else(|| "所有下载源都不可用，请检查网络后重试".to_string())?;
    let url = outcome.url.clone();
    if let Some(cb) = &log {
        cb(format!(
            "node: 选中 {}（{} KB/s），已预热 {} KB",
            url.split('/').nth(2).unwrap_or("?"),
            outcome.bytes_per_sec.map(|b| b / 1024).unwrap_or(0),
            outcome.warm_bytes / 1024
        ));
    }

    // 把竞速阶段已下载的数据挪到目标位置，正式下载从这里续传。
    let winner_part = scratch_dir.join("race-winner.part");
    let _ = tokio::fs::remove_file(&tmp).await;
    if outcome.warm_bytes > 0 && winner_part.exists() {
        if tokio::fs::rename(&winner_part, &tmp).await.is_err() {
            let _ = tokio::fs::remove_file(&winner_part).await;
        }
    }
    let _ = tokio::fs::remove_dir_all(&scratch_dir).await;

    download_with_diagnostics(client, &url, &tmp, on_progress.clone(), "node", log.clone()).await?;
    // 校验属于"解压/安装"阶段的前半段，单独上报，避免下载 100% 后界面静止。
    if let Some(cb) = &on_progress {
        cb(DownloadProgress {
            kind: "node-install".to_string(),
            received: 0,
            total: Some(100),
            percent: Some(0.0),
        });
    }
    if let Err(error) = verify_checksum(client, &url, &tmp).await {
        // 校验失败通常是续传数据与远端不一致，清掉重下一次再报错。
        let _ = tokio::fs::remove_file(&tmp).await;
        download_with_diagnostics(client, &url, &tmp, on_progress.clone(), "node", log.clone())
            .await?;
        verify_checksum(client, &url, &tmp)
            .await
            .map_err(|second| format!("{error}；重试后仍失败：{second}"))?;
    }

    finish_install(paths, &url, &tmp, &on_progress).await
}

/// Node 压缩包文件名，用于判断缓存的 URL 是否还指向当前版本。
fn node_file_name(platform: &str, arch: &str) -> String {
    let node_os = if platform == "win32" { "win" } else { platform };
    let ext = if platform == "win32" { "zip" } else { "tar.gz" };
    format!("node-{NODE_VERSION}-{node_os}-{arch}.{ext}")
}

/// 解压并确认 node 可用。
async fn finish_install(
    paths: &Paths,
    url: &str,
    tmp: &Path,
    on_progress: &Option<Arc<dyn Fn(DownloadProgress) + Send + Sync>>,
) -> Result<String, String> {
    // 校验通过，进入解压阶段。
    if let Some(cb) = &on_progress {
        cb(DownloadProgress {
            kind: "node-install".to_string(),
            received: 30,
            total: Some(100),
            percent: Some(30.0),
        });
    }

    // 解压到 node_dir（先清空再解）
    if paths.node_dir.exists() {
        std::fs::remove_dir_all(&paths.node_dir).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&paths.node_dir).map_err(|e| e.to_string())?;

    if url.ends_with(".zip") {
        extract_zip(tmp, &paths.node_dir)?;
    } else {
        extract_tar_gz(tmp, &paths.node_dir)?;
    }
    if let Some(cb) = &on_progress {
        cb(DownloadProgress {
            kind: "node-install".to_string(),
            received: 100,
            total: Some(100),
            percent: Some(100.0),
        });
    }
    let _ = std::fs::remove_file(tmp);

    // 确认 node 可用
    if !node_ready(paths) {
        return Err("Node 运行时解压后不可用".to_string());
    }
    Ok(paths.node_dir.display().to_string())
}

/// 校验下载文件的 sha256（SHASUMS256.txt 在同一目录）
async fn verify_checksum(
    client: &reqwest::Client,
    file_url: &str,
    file: &Path,
) -> Result<(), String> {
    // SHASUMS URL = 同目录下的 SHASUMS256.txt
    let sums_url = file_url
        .rsplit_once('/')
        .map(|(dir, _)| format!("{dir}/SHASUMS256.txt"))
        .ok_or_else(|| "非法下载 URL".to_string())?;
    let body = client
        .get(&sums_url)
        .send()
        .await
        .map_err(|e| format!("获取校验文件失败: {e}"))?
        .text()
        .await
        .map_err(|e| e.to_string())?;

    // SHASUMS 里的文件名 = 下载 URL 的末段（如 node-v22.22.0-darwin-arm64.tar.gz），
    // 本地临时文件用了别的名字（.dl），不能用本地 basename 匹配。
    let remote_name = file_url
        .rsplit('/')
        .next()
        .ok_or_else(|| "非法下载 URL".to_string())?;
    // SHASUMS 条目可能是 "hash  name" 或 "hash *name"；文件名可能带目录前缀
    let expect = body.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?;
        let bare = name.trim_start_matches('*');
        let bare = bare.rsplit('/').next().unwrap_or(bare);
        if bare == remote_name {
            Some(hash.to_string())
        } else {
            None
        }
    });
    let expect = expect.ok_or_else(|| "校验文件中未找到对应条目".to_string())?;
    let actual = sha256_hex(file)?;
    if actual.eq_ignore_ascii_case(&expect) {
        Ok(())
    } else {
        Err("校验和不匹配（下载文件可能损坏）".to_string())
    }
}

fn extract_tar_gz(src: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(src).map_err(|e| e.to_string())?;
    let gz = flate2::read::GzDecoder::new(f);
    let mut archive = tar::Archive::new(gz);
    // Node tarball 顶层是 node-vX/ 目录，需要 strip 一层
    archive
        .entries()
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter_map(|mut entry| {
            let path = entry.path().ok()?.to_path_buf();
            // strip 第一层（node-v22.../bin/... -> bin/...）
            let rel = path.components().skip(1).collect::<PathBuf>();
            if rel.as_os_str().is_empty() {
                return None;
            }
            let dest_path = dest.join(&rel);
            if let Some(parent) = dest_path.parent() {
                if std::fs::create_dir_all(parent).is_err() {
                    return None;
                }
            }
            if entry.header().entry_type().is_dir() {
                std::fs::create_dir_all(&dest_path).ok();
                None
            } else {
                let _ = entry.unpack(&dest_path);
                Some(dest_path)
            }
        })
        .count();
    // 设置可执行权限
    set_exec(dest);
    Ok(())
}

fn extract_zip(src: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(src).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let path = entry.enclosed_name().ok_or_else(|| "非法 zip 条目".to_string())?;
        let rel = path.components().skip(1).collect::<PathBuf>();
        if rel.as_os_str().is_empty() {
            continue;
        }
        let dest_path = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&dest_path).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = std::fs::File::create(&dest_path).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// 给 bin/ 下的可执行文件加权限
#[cfg(unix)]
fn set_exec(node_dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(entries) = std::fs::read_dir(node_dir.join("bin")) {
        for e in entries.flatten() {
            let p = e.path();
            if let Ok(meta) = std::fs::metadata(&p) {
                let mut perm = meta.permissions();
                perm.set_mode(0o755);
                let _ = std::fs::set_permissions(&p, perm);
            }
        }
    }
}

#[cfg(not(unix))]
fn set_exec(_node_dir: &Path) {}
