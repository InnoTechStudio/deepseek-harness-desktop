//! 路径与配置
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 运行时产物根目录（Node + dsh 内核都放这里）
#[derive(Debug, Clone)]
pub struct Paths {
    /// 用户数据根目录：~/Library/Application Support/com.dshdesk.app
    pub root: PathBuf,
    /// node runtime 目录
    pub node_dir: PathBuf,
    /// 内核安装目录（overlay）
    pub agent_dir: PathBuf,
    /// 内核 staging（升级用，全新安装）
    pub staging_dir: PathBuf,
    /// 备份目录（升级前旧版本 + 配置快照）
    pub backup_dir: PathBuf,
    /// 设置文件
    pub settings_file: PathBuf,
    /// 日志目录
    pub log_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Self {
        let root = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("com.dshdesk.app");
        Self {
            agent_dir: root.join("agent"),
            staging_dir: root.join("agent-staging"),
            backup_dir: root.join("agent-previous"),
            node_dir: root.join("runtime"),
            settings_file: root.join("settings.json"),
            log_dir: root.join("logs"),
            root,
        }
    }

    /// 本机架构（npm 安装的 dsh 只依赖 node 本身，无需架构判断）
    pub fn platform(&self) -> &'static str {
        if cfg!(target_os = "macos") {
            "darwin"
        } else if cfg!(target_os = "windows") {
            "win32"
        } else {
            "linux"
        }
    }
}

/// 应用设置（持久化）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// 跳过的内核版本（升级弹窗选"跳过此版本"时写入）
    pub skipped_kernel_version: Option<String>,
    /// 是否自动检查更新（默认开）
    pub auto_check_updates: bool,
    /// 内核版本来源缓存（测速选出的最快 npm 镜像）
    pub preferred_registry: Option<String>,
}

impl Settings {
    pub fn load(paths: &Paths) -> Self {
        std::fs::read_to_string(&paths.settings_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Settings {
                skipped_kernel_version: None,
                auto_check_updates: true,
                preferred_registry: None,
            })
    }

    pub fn save(&self, paths: &Paths) {
        if let Some(parent) = paths.settings_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            &paths.settings_file,
            serde_json::to_string_pretty(self).unwrap_or_default(),
        );
    }
}
