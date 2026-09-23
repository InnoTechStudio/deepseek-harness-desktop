//! 路径与配置
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 运行时产物根目录（Node + dsh 内核都放这里）
#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
    pub node_dir: PathBuf,
    pub agent_dir: PathBuf,
    pub staging_dir: PathBuf,
    pub backup_dir: PathBuf,
    pub settings_file: PathBuf,
    pub log_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Self {
        let root = data_root().join("com.dshdesk.app");
        Self {
            agent_dir: root.join("agent"), staging_dir: root.join("agent-staging"),
            backup_dir: root.join("agent-previous"), node_dir: root.join("runtime"),
            settings_file: root.join("settings.json"), log_dir: root.join("logs"), root,
        }
    }
    pub fn platform(&self) -> &'static str {
        if cfg!(target_os = "macos") { "darwin" } else if cfg!(target_os = "windows") { "win32" } else { "linux" }
    }
}

/// 用户级数据根目录。必须是绝对路径，且与软件安装位置无关。
///
/// `dirs::data_dir()` 在个别 Windows 环境会返回 None（`SHGetKnownFolderPath` 失败）。
/// 以前退回 `"."`，Node 就被装进了当时的安装目录；用户换路径重装后旧目录被弃用，
/// 启动就会报找不到 Node.js。家目录 / 临时目录仍然是绝对路径，换安装位置不影响。
fn data_root() -> PathBuf {
    if let Some(dir) = dirs::data_dir() {
        return dir;
    }
    if let Some(home) = dirs::home_dir() {
        if cfg!(target_os = "windows") {
            return home.join("AppData").join("Roaming");
        }
        if cfg!(target_os = "macos") {
            return home.join("Library").join("Application Support");
        }
        return home.join(".local").join("share");
    }
    std::env::temp_dir()
}

#[cfg(test)]
mod path_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn data_root_is_absolute_and_not_cwd() {
        let root = data_root();
        assert!(root.is_absolute(), "数据目录必须是绝对路径，否则换安装位置会找不到 Node");
        assert_ne!(root, PathBuf::from("."));
        let cwd = std::env::current_dir().unwrap();
        assert_ne!(root, cwd, "不能用当前工作目录当数据根");
    }

    #[test]
    fn resolve_keeps_runtime_under_stable_app_data() {
        let paths = Paths::resolve();
        assert!(paths.root.is_absolute());
        assert!(paths.node_dir.is_absolute());
        assert!(paths.root.ends_with("com.dshdesk.app"));
        assert!(
            path_has_suffix(&paths.node_dir, &["com.dshdesk.app", "runtime"]),
            "Node 必须落在稳定的应用数据目录，而不是安装目录: {}",
            paths.node_dir.display()
        );
    }

    fn path_has_suffix(path: &Path, suffix: &[&str]) -> bool {
        let comps: Vec<_> = path.components().collect();
        let tail: Vec<_> = suffix.iter().map(std::ffi::OsStr::new).collect();
        comps
            .iter()
            .rev()
            .map(|c| c.as_os_str())
            .take(tail.len())
            .eq(tail.iter().rev().copied())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    pub skipped_kernel_version: Option<String>,
    pub auto_check_updates: bool,
    pub preferred_registry: Option<String>,
    pub confirm_exit: bool,
    #[serde(default)] pub auto_launch: bool,
    #[serde(default)] pub prevent_sleep: bool,
    #[serde(default)] pub desktop_notifications: bool,
    #[serde(default = "default_true")] pub notification_turn_complete: bool,
    #[serde(default = "default_true")] pub notification_turn_failed: bool,
    #[serde(default = "default_true")] pub notification_job_complete: bool,
    #[serde(default = "default_true")] pub notification_job_failed: bool,
}

fn default_true() -> bool { true }

impl Settings {
    pub fn load(paths: &Paths) -> Self {
        std::fs::read_to_string(&paths.settings_file).ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Self {
                skipped_kernel_version: None, auto_check_updates: true, preferred_registry: None,
                confirm_exit: false, auto_launch: false, prevent_sleep: false,
                desktop_notifications: false, notification_turn_complete: true,
                notification_turn_failed: true, notification_job_complete: true, notification_job_failed: true,
            })
    }
    pub fn save(&self, paths: &Paths) {
        if let Some(parent) = paths.settings_file.parent() { let _ = std::fs::create_dir_all(parent); }
        let _ = std::fs::write(&paths.settings_file, serde_json::to_string_pretty(self).unwrap_or_default());
    }
}
