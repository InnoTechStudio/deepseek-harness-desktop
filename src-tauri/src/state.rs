//! 应用全局状态
use crate::paths::Paths;
use std::sync::Arc;
use tokio::sync::Mutex;

/// dsh 服务运行时状态
pub struct DshRuntime {
    pub child: std::process::Child,
    pub port: u16,
    /// 内核打印的完整访问地址（含鉴权令牌）。
    ///
    /// 前端必须加载这一条，不能用端口自己拼：新版内核要靠 URL 里的令牌
    /// 换取 cookie，拼出来的地址会被判 401。见 `dsh::DshEndpoint`。
    pub url: String,
}

/// 全局状态
pub struct AppState {
    pub paths: Paths,
    pub client: reqwest::Client,
    /// dsh 子进程（如果有）
    pub dsh: Arc<Mutex<Option<DshRuntime>>>,
    /// 升级是否进行中（防止并发）
    pub updating: Arc<Mutex<bool>>,
    /// 把内核代理到与宿主同源的地址，绕开跨站 cookie 限制。见 `proxy` 模块。
    pub proxy: crate::proxy::ProxyState,
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
            proxy: crate::proxy::ProxyState::new(),
        }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }
}
