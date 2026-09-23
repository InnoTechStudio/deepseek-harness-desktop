//! 把 DSH 内核反向代理到一个我们自己持有的本地 http 端口。
//!
//! # 为什么需要它
//!
//! 内核给 Web 界面加了浏览器鉴权：带令牌访问根路径换一个 cookie，之后靠 cookie 放行。
//! 那个 cookie 是 `HttpOnly; SameSite=Strict` 且绑定 `Host`。
//!
//! 宿主页面是 `tauri://localhost`，iframe 若直接指向 `http://127.0.0.1:<内核端口>`，
//! 两者跨站，`SameSite=Strict` 的 cookie 存不下也发不出：带令牌请求 → 服务端 303
//! 并下发 cookie → cookie 被 WebView 丢弃 → 跳转到干净的 `/` 时无凭证 → 401
//! "dsh web authentication required"。用 curl 测全过，因为 curl 没有 SameSite 概念。
//!
//! # 为什么不用 Tauri 自定义协议
//!
//! 试过 `dsh://localhost/`。同源问题确实解决了，但内核建 WebSocket 时会执行
//! `url.protocol = "ws:"`（见 dsh-api-gateway 的 remoteStreamUrl），
//! 自定义协议不在可切换的协议族里，WebKit 直接抛
//! "The string did not match the expected pattern"，界面变成
//! "Failed to load plugins @deepseek-ai/dsh-api-gateway"。
//!
//! 所以 iframe 必须拿到一个**真正的 http origin**。
//!
//! # 做法
//!
//! 在 127.0.0.1 上开一个自己的端口，逐字转发到内核端口。鉴权 cookie 由本模块
//! 持有并在转发时注入，浏览器完全不参与 cookie 决策，SameSite 也就无从谈起。
//!
//! 我们不解析也不理解令牌与 cookie 的格式，只是原样搬运——内核以后改鉴权方式，
//! 这里不用跟着改。

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;

/// 一次内核会话的转发目标与凭证。
#[derive(Debug, Clone, Default)]
struct Upstream {
    /// `http://127.0.0.1:<内核端口>`，无尾斜杠。
    origin: String,
    /// 换到的 cookie（`name=value` 形式），未换到时为空。
    cookie: String,
}

/// 反向代理。内核每次重启都会换端口与令牌，所以目标要可替换。
#[derive(Clone)]
pub struct ProxyState {
    inner: Arc<Mutex<Upstream>>,
    /// 代理监听的端口，服务起来后才有值。
    listen_port: Arc<Mutex<Option<u16>>>,
    /// 代理专用客户端。
    ///
    /// 不复用 `AppState::client`：换 cookie 时必须自己看见 303 才能读到
    /// Set-Cookie，而那个客户端要跟随重定向（下载与更新检查都依赖）。
    client: reqwest::Client,
}

impl Default for ProxyState {
    fn default() -> Self {
        Self::new()
    }
}

impl ProxyState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Upstream::default())),
            listen_port: Arc::new(Mutex::new(None)),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .unwrap_or_default(),
        }
    }

    /// 前端应加载的地址；代理未启动时为 None。
    pub fn client_url(&self) -> Option<String> {
        let port = (*self.listen_port.lock().ok()?)?;
        Some(format!("http://127.0.0.1:{port}/"))
    }

    /// 记录本次内核会话，并立刻用令牌换取 cookie。
    ///
    /// 换取动作放在这里而不是首个请求时：iframe 的第一个请求就是取首页，
    /// 那时若还没有 cookie 就会吃到 401，白白闪一次错误页。
    ///
    /// @param authenticated_url 内核打印的完整地址（含令牌）。
    /// @returns 是否成功拿到 cookie。拿不到也不算致命，旧内核本就没有鉴权。
    pub async fn adopt(&self, authenticated_url: &str) -> bool {
        let Ok(parsed) = reqwest::Url::parse(authenticated_url) else {
            return false;
        };
        let origin = format!("{}://{}", parsed.scheme(), parsed.authority());

        // 手动处理 303：要的是它的 Set-Cookie，而不是跟随跳转。
        let cookie = match self.client.get(authenticated_url).send().await {
            Ok(response) => response
                .headers()
                .get_all(reqwest::header::SET_COOKIE)
                .iter()
                .filter_map(|value| value.to_str().ok())
                // 只取 `name=value`，属性由我们自己决定要不要带。
                .filter_map(|raw| raw.split(';').next())
                .collect::<Vec<_>>()
                .join("; "),
            Err(_) => String::new(),
        };

        let ok = !cookie.is_empty();
        if let Ok(mut guard) = self.inner.lock() {
            *guard = Upstream { origin, cookie };
        }
        ok
    }

    /// 当前转发目标；内核未就绪时为 None。
    fn snapshot(&self) -> Option<Upstream> {
        let guard = self.inner.lock().ok()?;
        if guard.origin.is_empty() {
            return None;
        }
        Some(guard.clone())
    }

    /// 内核停止后清空，避免请求打到已失效的端口。
    pub fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            *guard = Upstream::default();
        }
    }

    /// 启动代理监听。端口由系统分配，返回它。
    ///
    /// 只绑 127.0.0.1：这个端口带着内核的鉴权凭证，绝不能对外可达。
    pub async fn serve(&self) -> Result<u16, String> {
        if let Some(port) = *self.listen_port.lock().map_err(|_| "代理端口锁失败")? {
            return Ok(port);
        }
        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .map_err(|error| format!("代理端口监听失败: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("读取代理端口失败: {error}"))?
            .port();
        if let Ok(mut guard) = self.listen_port.lock() {
            *guard = Some(port);
        }

        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let state = state.clone();
                tauri::async_runtime::spawn(async move {
                    let service = service_fn(move |request| {
                        let state = state.clone();
                        async move { Ok::<_, Infallible>(forward(state, request).await) }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        // 内核会用 WebSocket 推流，必须允许协议升级。
                        .serve_connection(TokioIo::new(stream), service)
                        .with_upgrades()
                        .await;
                });
            }
        });
        Ok(port)
    }
}

/// 逐字转发时必须丢掉的逐跳首部。
///
/// 连接管理类首部只对单条 TCP 连接有意义，原样搬到另一条连接上会让两端
/// 对报文边界产生分歧（典型症状：页面加载到一半卡住）。
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
];

fn is_hop_by_hop(name: &str) -> bool {
    HOP_BY_HOP.contains(&name.to_ascii_lowercase().as_str())
}

/// 请求是否要求切换协议（WebSocket 握手）。
///
/// 内核用 WebSocket 推流（`/api/remote.mux`）。这类请求不能按普通 HTTP 转发：
/// `Upgrade`/`Connection` 属于逐跳首部会被剥掉，内核收到的就是一个没有升级
/// 意图的普通 GET，直接回 404，页面因为拿不到数据而一片空白。
fn wants_upgrade(headers: &hyper::HeaderMap) -> bool {
    let connection_upgrades = headers
        .get(hyper::header::CONNECTION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        });
    connection_upgrades && headers.contains_key(hyper::header::UPGRADE)
}

/// 把一个请求转发给内核，返回它的响应。
async fn forward(state: ProxyState, request: Request<Incoming>) -> Response<Full<Bytes>> {
    let Some(upstream) = state.snapshot() else {
        return plain(StatusCode::SERVICE_UNAVAILABLE, "DSH 服务尚未就绪");
    };

    if wants_upgrade(request.headers()) {
        return upgrade(state, upstream, request).await;
    }

    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/")
        .to_string();
    let method = request.method().clone();
    let headers = request.headers().clone();

    let body = match request.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => return plain(StatusCode::BAD_REQUEST, &format!("读取请求体失败：{error}")),
    };

    let target = format!("{}{}", upstream.origin, path_and_query);
    let mut outbound = state.client.request(method, &target);
    for (name, value) in headers.iter() {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        // Host 由 reqwest 按目标地址填；照搬会让内核认错 authority，
        // 而它正是按 Host 派生 cookie 名的。
        if name.as_str().eq_ignore_ascii_case("host") {
            continue;
        }
        // 浏览器发来的 cookie 与内核无关，统一由我们注入。
        if name.as_str().eq_ignore_ascii_case("cookie") {
            continue;
        }
        // 压缩交给内核和浏览器直接协商是做不到的——中间隔着我们。
        // 而 reqwest 没开 gzip 特性，不会替我们解压，转发压缩字节又必须
        // 原样保留 content-encoding，任何一头对不上都会出问题：
        // 声明了却是明文 → 浏览器解码失败、页面空白；
        // 是压缩字节却不声明 → 浏览器当明文渲染、满屏乱码（两种都踩过）。
        // 最稳的是直接要求内核发明文，代理只搬运字节。
        if name.as_str().eq_ignore_ascii_case("accept-encoding") {
            continue;
        }
        // Origin/Referer 同 Host 一起改写：内核按"两者是否一致"判同源，
        // 照搬浏览器发来的代理端口会被判跨站。
        if name.as_str().eq_ignore_ascii_case("origin")
            || name.as_str().eq_ignore_ascii_case("referer")
        {
            continue;
        }
        outbound = outbound.header(name.as_str(), value.as_bytes());
    }
    outbound = outbound.header(reqwest::header::ACCEPT_ENCODING, "identity");
    outbound = outbound.header(reqwest::header::ORIGIN, &upstream.origin);
    if !upstream.cookie.is_empty() {
        outbound = outbound.header(reqwest::header::COOKIE, &upstream.cookie);
    }
    if !body.is_empty() {
        outbound = outbound.body(body.to_vec());
    }

    let response = match outbound.send().await {
        Ok(response) => response,
        Err(error) => {
            return plain(StatusCode::BAD_GATEWAY, &format!("无法连接 DSH 服务：{error}"))
        }
    };

    let status = response.status();
    let response_headers = response.headers().clone();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            return plain(StatusCode::BAD_GATEWAY, &format!("读取 DSH 响应失败：{error}"))
        }
    };

    let mut builder = Response::builder().status(status.as_u16());
    for (name, value) in response_headers.iter() {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        // Set-Cookie 不转发：凭证由本模块持有，交给 WebView 反而会被
        // SameSite 规则丢弃，制造"有时能用有时不能"的随机现象。
        if name.as_str().eq_ignore_ascii_case("set-cookie") {
            continue;
        }
        // 长度按实际转发的字节重算，避免与解压后的实际长度不一致。
        if name.as_str().eq_ignore_ascii_case("content-length") {
            continue;
        }
        // reqwest 已经把 gzip/br 解开了，我们转发的是明文字节。
        // 照搬 content-encoding 会让浏览器拿明文当压缩数据去解，直接失败，
        // 页面表现为一片空白——而 curl 因为容错反而看不出问题。
        if name.as_str().eq_ignore_ascii_case("content-encoding") {
            continue;
        }
        builder = builder.header(name.as_str(), value.as_bytes());
    }
    builder
        .header("content-length", bytes.len().to_string())
        .body(Full::new(bytes))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "构造响应失败"))
}

/// 转发一次协议升级（WebSocket）。
///
/// 自己开一条到内核的 TCP 连接，把握手请求逐字写过去；内核同意后，
/// 两侧连接都升级成裸流，之后双向对拷字节即可——代理不关心帧格式。
async fn upgrade(
    state: ProxyState,
    upstream: Upstream,
    mut request: Request<Incoming>,
) -> Response<Full<Bytes>> {
    use tokio::io::AsyncWriteExt;

    let Some(authority) = upstream.origin.strip_prefix("http://") else {
        return plain(StatusCode::BAD_GATEWAY, "上游地址格式异常");
    };
    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/")
        .to_string();

    let mut server = match tokio::net::TcpStream::connect(authority).await {
        Ok(stream) => stream,
        Err(error) => {
            return plain(StatusCode::BAD_GATEWAY, &format!("无法连接 DSH 服务：{error}"))
        }
    };

    // 手写握手请求：这里要的是逐字透传，包括 Upgrade/Connection 这些
    // 在普通转发里会被剥掉的首部。
    let mut handshake = format!(
        "GET {path_and_query} HTTP/1.1\r\nHost: {authority}\r\n"
    );
    for (name, value) in request.headers() {
        if name.as_str().eq_ignore_ascii_case("host")
            || name.as_str().eq_ignore_ascii_case("cookie")
            || name.as_str().eq_ignore_ascii_case("origin")
        {
            continue;
        }
        if let Ok(text) = value.to_str() {
            handshake.push_str(&format!("{}: {}\r\n", name.as_str(), text));
        }
    }
    // Origin 必须与 Host 一致，否则内核判跨站直接 403，WebSocket 建不起来，
    // 界面左下角显示"连接异常"。
    //
    // 浏览器发来的 Origin 是代理端口，而 Host 上面已改成内核 authority，
    // 两者对不上。注意 `--trusted-host` 治不了这条：那是给 /api 围栏用的，
    // 这里比的是 Origin 与 Host 是否严格相等（实测确认）。
    handshake.push_str(&format!("Origin: http://{authority}\r\n"));
    if !upstream.cookie.is_empty() {
        handshake.push_str(&format!("Cookie: {}\r\n", upstream.cookie));
    }
    handshake.push_str("\r\n");

    if let Err(error) = server.write_all(handshake.as_bytes()).await {
        return plain(StatusCode::BAD_GATEWAY, &format!("发送握手失败：{error}"));
    }

    // 读内核的握手响应。101 之后才是双向流，其余情况原样告知浏览器。
    // 成块读，读到首部结束为止；同一批里多出来的字节是已经开始的 WebSocket
    // 数据，必须留给下面的转发，不能丢。
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let head_end = loop {
        use tokio::io::AsyncReadExt;
        match server.read(&mut chunk).await {
            Ok(0) => return plain(StatusCode::BAD_GATEWAY, "DSH 服务在握手期间断开"),
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(index) = find_subslice(&buffer, b"\r\n\r\n") {
                    break index + 4;
                }
                if buffer.len() > 32 * 1024 {
                    return plain(StatusCode::BAD_GATEWAY, "握手响应首部过大");
                }
            }
            Err(error) => {
                return plain(StatusCode::BAD_GATEWAY, &format!("读取握手失败：{error}"))
            }
        }
    };
    let head = buffer[..head_end].to_vec();
    let leftover = buffer[head_end..].to_vec();

    let head_text = String::from_utf8_lossy(&head).to_string();
    if !head_text.starts_with("HTTP/1.1 101") {
        let status = head_text
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .and_then(|code| StatusCode::from_u16(code).ok())
            .unwrap_or(StatusCode::BAD_GATEWAY);
        return plain(status, "DSH 服务拒绝了协议升级");
    }

    // 两边都升级完成后，把字节流对接起来。
    //
    // `leftover` 不能丢：读握手响应时是按字节找 `\r\n\r\n`，而内核往往在同一个
    // TCP 段里就跟着发了第一批 WebSocket 帧。那些字节已经进了我们的缓冲区，
    // 直接开始对拷会把它们吞掉——连接建立了却收不到数据，界面显示"连接异常"。
    tauri::async_runtime::spawn(async move {
        let _ = &state;
        match hyper::upgrade::on(&mut request).await {
            Ok(upgraded) => {
                let mut client = TokioIo::new(upgraded);
                if !leftover.is_empty() {
                    if client.write_all(&leftover).await.is_err() {
                        return;
                    }
                }
                let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
            }
            Err(_) => {
                let _ = server.shutdown().await;
            }
        }
    });

    // 把内核的 101 响应转达给浏览器，让它也完成升级。
    let mut builder = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    for line in head_text.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() || value.is_empty() {
            continue;
        }
        // 这三项由 hyper 自己按升级流程重新生成，照搬会冲突。
        if name.eq_ignore_ascii_case("content-length")
            || name.eq_ignore_ascii_case("transfer-encoding")
        {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder
        .body(Full::new(Bytes::new()))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "构造升级响应失败"))
}

/// 在字节序列里找子串位置。诊断注入用。
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn plain(status: StatusCode, message: &str) -> Response<Full<Bytes>> {
    let body = Bytes::from(message.to_string());
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .header("cache-control", "no-store")
        .header("content-length", body.len().to_string())
        .body(Full::new(body))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_by_hop_headers_are_recognized_case_insensitively() {
        assert!(is_hop_by_hop("Connection"));
        assert!(is_hop_by_hop("TRANSFER-ENCODING"));
        assert!(!is_hop_by_hop("content-type"));
        // cookie 不是逐跳首部，它由转发逻辑单独处理。
        assert!(!is_hop_by_hop("cookie"));
    }

    #[test]
    fn state_starts_empty_and_clears() {
        let state = ProxyState::new();
        assert!(state.snapshot().is_none(), "未就绪时不应有转发目标");
        assert!(state.client_url().is_none(), "代理未启动时不应给出地址");
        if let Ok(mut guard) = state.inner.lock() {
            *guard = Upstream {
                origin: "http://127.0.0.1:1234".to_string(),
                cookie: "a=b".to_string(),
            };
        }
        assert!(state.snapshot().is_some());
        state.clear();
        assert!(state.snapshot().is_none(), "clear 后必须回到未就绪");
    }

    #[test]
    fn client_url_uses_the_listening_port() {
        let state = ProxyState::new();
        if let Ok(mut guard) = state.listen_port.lock() {
            *guard = Some(45678);
        }
        // 必须是真正的 http origin：自定义协议下内核建 WebSocket 会失败。
        assert_eq!(state.client_url().as_deref(), Some("http://127.0.0.1:45678/"));
    }

    #[test]
    fn websocket_handshake_is_detected() {
        // 必须同时有 Connection: upgrade 与 Upgrade 首部才算升级请求。
        let mut headers = hyper::HeaderMap::new();
        assert!(!wants_upgrade(&headers), "空首部不是升级请求");

        headers.insert(hyper::header::UPGRADE, "websocket".parse().unwrap());
        assert!(!wants_upgrade(&headers), "只有 Upgrade 还不够");

        headers.insert(hyper::header::CONNECTION, "Upgrade".parse().unwrap());
        assert!(wants_upgrade(&headers), "两者齐备才是升级请求");
    }

    #[test]
    fn connection_header_may_list_several_tokens() {
        // 浏览器常发 `Connection: keep-alive, Upgrade`，逐项比对才不会漏判。
        let mut headers = hyper::HeaderMap::new();
        headers.insert(hyper::header::UPGRADE, "websocket".parse().unwrap());
        headers.insert(hyper::header::CONNECTION, "keep-alive, Upgrade".parse().unwrap());
        assert!(wants_upgrade(&headers));
    }

    #[test]
    fn ordinary_requests_are_not_upgrades() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(hyper::header::CONNECTION, "keep-alive".parse().unwrap());
        assert!(!wants_upgrade(&headers), "普通长连接不该走升级路径");
    }

    #[test]
    fn error_responses_declare_their_real_length() {
        let response = plain(StatusCode::SERVICE_UNAVAILABLE, "DSH 服务尚未就绪");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let declared = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok());
        // 声明长度与实际不符时 WebView 会等一个永远收不满的 body。
        assert_eq!(declared, Some("DSH 服务尚未就绪".len()));
    }
}

/// 手工验证：起真实内核 + 代理，走一遍完整链路。
/// `cargo test --lib live_proxy -- --ignored --nocapture`
#[cfg(test)]
mod live_proxy {
    use super::*;

    #[test]
    #[ignore = "需要已安装的内核与网络，手工运行"]
    fn proxy_serves_kernel_through_plain_http() {
        let paths = crate::paths::Paths::resolve();
        let node = crate::runtime::node_bin(&paths);
        let entry = crate::kernel::kernel_bin(&paths);
        assert!(node.exists() && entry.exists(), "内核未就绪");

        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut child = crate::dsh::spawn_dsh(&paths, "web-desktop").expect("内核应能启动");

            // 抓内核打印的完整地址
            let stdout = child.stdout.take().expect("应有 stdout");
            let endpoint = tokio::task::spawn_blocking(move || {
                use std::io::{BufRead, BufReader};
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if let Some(found) = crate::dsh::parse_endpoint_from_line(&line) {
                        return Some(found);
                    }
                }
                None
            })
            .await
            .unwrap()
            .expect("应打印出服务地址");
            println!("内核端口 {} 带鉴权参数={}", endpoint.port, endpoint.url.contains('?'));

            let state = ProxyState::new();
            let got_cookie = state.adopt(&endpoint.url).await;
            println!("换取凭证: {got_cookie}");
            let port = state.serve().await.expect("代理应能监听");
            let base = state.client_url().expect("应有地址");
            println!("代理地址: {base}");
            assert!(base.starts_with("http://127.0.0.1:"), "必须是真正的 http origin");

            let probe = reqwest::Client::new();
            let index = probe.get(&base).send().await.expect("首页应可取");
            println!("首页: HTTP {}", index.status());
            assert_eq!(index.status(), 200, "经代理取首页应为 200，而不是 401");
            let html = index.text().await.unwrap_or_default();
            assert!(html.contains("<html") || html.contains("<!"), "应返回 HTML");

            let api = probe
                .get(format!("http://127.0.0.1:{port}/dsh-market/installed"))
                .send()
                .await
                .expect("API 应可取");
            println!("市场接口: HTTP {}", api.status());
            assert_eq!(api.status(), 200, "API 也要带上凭证");

            // WebSocket 是页面渲染的必要条件：内核靠它推流。
            // 之前代理把升级请求当普通 GET 转发，内核回 404，界面一片空白。
            let handshake = probe
                .get(format!("http://127.0.0.1:{port}/api/remote.mux"))
                .header("Connection", "Upgrade")
                .header("Upgrade", "websocket")
                .header("Sec-WebSocket-Version", "13")
                .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
                .send()
                .await
                .expect("握手应有响应");
            println!("WebSocket 升级: HTTP {}", handshake.status());

            // 浏览器一定会带 Origin，且是代理端口。内核有来源校验，
            // 不加 --trusted-host 就 403，界面显示"连接异常"——
            // 而命令行不带 Origin 测试时一切正常，最容易漏掉。
            let with_origin = probe
                .get(format!("http://127.0.0.1:{port}/api/remote.mux"))
                .header("Connection", "Upgrade")
                .header("Upgrade", "websocket")
                .header("Sec-WebSocket-Version", "13")
                .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
                .header("Origin", format!("http://127.0.0.1:{port}"))
                .send()
                .await
                .expect("带 Origin 的握手应有响应");
            println!("带 Origin 的升级: HTTP {}", with_origin.status());
            assert_eq!(
                with_origin.status().as_u16(),
                101,
                "带 Origin 必须也能升级；403 说明内核缺 --trusted-host"
            );

            // 代理向内核要明文，所以转发出去的一定不带 content-encoding。
            // 两种错配都真实踩过：声明了却是明文 → 页面空白；
            // 是压缩字节却不声明 → 满屏乱码。
            let index_headers = probe.get(&base)
                .header("Accept-Encoding", "gzip, deflate")
                .send().await.expect("首页应可取");
            let encoding = index_headers.headers().get("content-encoding").cloned();
            println!("content-encoding: {encoding:?}（必须为 None）");
            assert!(encoding.is_none(), "不应声明压缩编码");

            // 真正验证内容是明文 HTML，而不是没解开的压缩字节。
            let body = index_headers.text().await.unwrap_or_default();
            println!("首页开头: {}", body.chars().take(40).collect::<String>());
            assert!(
                body.trim_start().starts_with("<!") || body.contains("<html"),
                "内容必须是可读 HTML；乱码说明拿到的是压缩字节"
            );
            assert_eq!(
                handshake.status().as_u16(),
                101,
                "必须是 101 切换协议；404 说明升级首部被剥掉了"
            );

            let _ = child.kill();
            let _ = child.wait();
        });
    }
}
