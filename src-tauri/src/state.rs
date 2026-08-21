//! 应用全局状态
use crate::paths::Paths;
use std::sync::Arc;
use tokio::sync::Mutex;

/// dsh 服务运行时状态
pub struct DshRuntime {
    pub child: std::process::Child,
    pub port: u16,
}

/// 全局状态
pub struct AppState {
    pub paths: Paths,
    pub client: reqwest::Client,
    /// dsh 子进程（如果有）
    pub dsh: Arc<Mutex<Option<DshRuntime>>>,
    /// 升级是否进行中（防止并发）
    pub updating: Arc<Mutex<bool>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            paths: Paths::resolve(),
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(120))
                .user_agent("DSHDesk/0.1")
                .build()
                .unwrap_or_default(),
            dsh: Arc::new(Mutex::new(None)),
            updating: Arc::new(Mutex::new(false)),
        }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }
}
