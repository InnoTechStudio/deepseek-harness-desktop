//! 动态选源：并行测量各镜像的**实际吞吐量**，选下载最快的源。
//!
//! 早期版本只测首包时间（TTFB），但首包快不等于带宽高——国内镜像常见
//! "握手很快、下载很慢"。现在改为在固定时间窗口内实际拉取数据并计算
//! 字节/秒，同时保留 TTFB 作为次要指标用于打平时排序。
//! 候选源只收录"完整、快、稳定"的厂商级镜像。

use std::time::{Duration, Instant};

/// 测速时每个源最多拉取的数据量（够区分带宽，又不浪费流量）。
const PROBE_BYTES: u64 = 768 * 1024;
/// 单个源的测速时间上限，超时即用已下载量估算带宽。
const PROBE_WINDOW: Duration = Duration::from_millis(1500);
/// 连接阶段超时。跨境链路握手常在 1.5s 以上，给足时间以免把可用的境外源
/// （境外用户唯一的快源）误判为不可用。
const PROBE_CONNECT_TIMEOUT: Duration = Duration::from_millis(4000);

/// 单个候选源
#[derive(Debug, Clone)]
pub struct Mirror {
    pub name: &'static str,
    /// 测速用的 URL，需要是该源上足够大的真实文件
    pub probe_url: String,
    /// 实际下载/查版本的 URL
    pub base_url: String,
}

/// 客户端更新分发的基准地址。
///
/// 这里只定义不带镜像前缀的规范地址；要不要加镜像前缀、加哪一个，
/// 由 `github_update_mirrors` 测速决定。
pub mod update_urls {
    /// 仓库与分支。
    pub const REPO: &str = "InnoTechStudio/deepseek-harness-desktop";
    pub const BRANCH: &str = "main";

    /// 版本清单（客户端更新检查读它）。静态文件，无 API 限流，可被镜像缓存。
    pub fn version_json() -> String {
        format!("https://raw.githubusercontent.com/{REPO}/{BRANCH}/update.json")
    }

    /// 问题反馈入口（浏览器预填新建 Issue）。
    pub fn issues_new() -> String {
        format!("https://github.com/{REPO}/issues/new")
    }
}

/// GitHub 客户端更新源的候选镜像。
///
/// 更新检查读的是静态 `update.json`，下载的是 Release 资产——两个地址都由
/// 应用按「镜像前缀 + 规范地址」拼出来，所以这个列表只记录各源的前缀。
///
/// 收录标准与 `npm_mirrors`/`node_mirrors` 相同：完整、快、稳定。
/// 直连排第一作为基准；国内镜像只对被它代理加速时才有意义。
pub fn github_update_mirrors() -> Vec<Mirror> {
    // 一个固定的公开文件做探针：内容稳定、体积足够测出带宽差异。
    let probe = "https://raw.githubusercontent.com/microsoft/vscode/main/package.json";
    let make = |name: &'static str, prefix: &str| Mirror {
        name,
        probe_url: format!("{prefix}{probe}"),
        base_url: prefix.to_string(),
    };
    vec![
        make("github", ""),
        make("gh-proxy", "https://gh-proxy.com/"),
        make("ghfast", "https://ghfast.top/"),
        make("ghproxy", "https://ghproxy.net/"),
    ]
}

/// npm 镜像候选源（用于版本检测 + 内核 tarball 下载）。
/// 实测元数据延迟：npmmirror 0.26s < 华为 0.39s < 腾讯 0.54s < 官方 1.57s。
pub fn npm_mirrors() -> Vec<Mirror> {
    let pkg = "@deepseek-ai/dsh";
    let encoded = pkg.replace('/', "%2f");
    let make = |name: &'static str, base: &str| Mirror {
        name,
        probe_url: format!("{base}{encoded}"),
        base_url: base.to_string(),
    };
    vec![
        make("npmmirror", "https://registry.npmmirror.com/"),
        make("huawei", "https://repo.huaweicloud.com/repository/npm/"),
        make("tencent", "https://mirrors.cloud.tencent.com/npm/"),
        // 官方源留作境外/兜底：国内慢但始终可用，境外反而是最快的。
        make("npmjs", "https://registry.npmjs.org/"),
    ]
}

/// Node runtime 镜像候选源（下载 node 压缩包）。
/// 测速直接打真实的 node 包本体，这样测到的带宽就是实际下载会拿到的带宽。
///
/// 候选源经过实测筛选，只保留在多种网络下都能返回 206 的地址：
/// - `repo.huaweicloud.com` 与 `mirrors.huaweicloud.com` 内容相同，但前者
///   在家用宽带快 2.6 倍、在移动热点快 150 倍以上，因此优先。
/// - 清华、浙大、北外、华科的 `nodejs-release` 路径实测返回 404（已不再镜像
///   Node 二进制），上交超时，故不收录。
/// - 官方源在国内很慢，但在境外是最快的，保留作为兜底并参与测速。
pub fn node_mirrors(version: &str, platform: &str, arch: &str) -> Vec<Mirror> {
    let node_os = if platform == "win32" { "win" } else { platform };
    let file_ext = if platform == "win32" { "zip" } else { "tar.gz" };
    let file = format!("node-{version}-{node_os}-{arch}.{file_ext}");

    let make = |name: &'static str, url: String| Mirror {
        name,
        probe_url: url.clone(),
        base_url: url,
    };
    vec![
        make(
            "huawei-repo",
            format!("https://repo.huaweicloud.com/nodejs/{version}/{file}"),
        ),
        make(
            "aliyun",
            format!("https://mirrors.aliyun.com/nodejs-release/{version}/{file}"),
        ),
        make(
            "npmmirror",
            format!("https://cdn.npmmirror.com/binaries/node/{version}/{file}"),
        ),
        make(
            "tencent",
            format!("https://mirrors.cloud.tencent.com/nodejs-release/{version}/{file}"),
        ),
        make(
            "nju",
            format!("https://mirror.nju.edu.cn/nodejs-release/{version}/{file}"),
        ),
        make(
            "ustc",
            format!("https://mirrors.ustc.edu.cn/node/{version}/{file}"),
        ),
        make(
            "official",
            format!("https://nodejs.org/dist/{version}/{file}"),
        ),
    ]
}

/// 测速结果。`bytes_per_sec` 是主要排序依据，`ttf_ms` 仅用于打平时比较。
#[derive(Debug, Clone)]
pub struct SpeedResult {
    pub name: &'static str,
    pub ttf_ms: Option<u64>,
    pub bytes_per_sec: Option<u64>,
    pub base_url: String,
}

/// 并行测速所有候选源，返回按实测带宽降序排序的结果（最快的在前）。
pub async fn speed_test(client: &reqwest::Client, mirrors: &[Mirror]) -> Vec<SpeedResult> {
    let mut handles = Vec::new();
    for m in mirrors {
        let client = client.clone();
        let probe_url = m.probe_url.clone();
        let base_url = m.base_url.clone();
        let name = m.name;
        handles.push(tokio::spawn(async move {
            let (ttf_ms, bytes_per_sec) = probe_throughput(&client, &probe_url).await;
            SpeedResult { name, ttf_ms, bytes_per_sec, base_url }
        }));
    }
    let mut results: Vec<SpeedResult> = Vec::new();
    for h in handles {
        if let Ok(r) = h.await {
            results.push(r);
        }
    }
    // 带宽高的排前面；带宽相同（或都失败）时用首包时间兜底。
    results.sort_by(|a, b| {
        b.bytes_per_sec
            .unwrap_or(0)
            .cmp(&a.bytes_per_sec.unwrap_or(0))
            .then_with(|| a.ttf_ms.unwrap_or(u64::MAX).cmp(&b.ttf_ms.unwrap_or(u64::MAX)))
    });
    results
}

/// 在时间窗口内实际拉取数据，返回（首包毫秒数, 字节每秒）。
/// 任一环节失败都返回 `(None, None)`，表示该源不可用。
async fn probe_throughput(client: &reqwest::Client, url: &str) -> (Option<u64>, Option<u64>) {
    use futures::StreamExt;

    let start = Instant::now();
    let request = client
        .get(url)
        .header("Range", format!("bytes=0-{}", PROBE_BYTES - 1))
        .timeout(PROBE_CONNECT_TIMEOUT + PROBE_WINDOW);
    let Ok(response) = request.send().await else {
        return (None, None);
    };
    if !response.status().is_success() && response.status().as_u16() != 206 {
        return (None, None);
    }

    let mut stream = response.bytes_stream();
    let mut received: u64 = 0;
    let mut first_byte: Option<Instant> = None;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else { break };
        if first_byte.is_none() {
            first_byte = Some(Instant::now());
        }
        received += chunk.len() as u64;
        if received >= PROBE_BYTES || start.elapsed() >= PROBE_CONNECT_TIMEOUT + PROBE_WINDOW {
            break;
        }
    }

    let ttf_ms = first_byte.map(|t| t.duration_since(start).as_millis() as u64);
    if received == 0 {
        return (ttf_ms, None);
    }
    // 从首包开始计时，排除 DNS/TLS 握手，得到更接近稳定带宽的值。
    let transfer_start = first_byte.unwrap_or(start);
    let elapsed = transfer_start.elapsed().as_secs_f64().max(0.001);
    let bps = (received as f64 / elapsed) as u64;
    (ttf_ms, Some(bps))
}

/// 选最快的可用源（有实测带宽的优先；都没有则回退第一个）。
pub fn pick_fastest(results: &[SpeedResult]) -> Option<&SpeedResult> {
    results
        .iter()
        .find(|r| r.bytes_per_sec.is_some())
        .or_else(|| results.iter().find(|r| r.ttf_ms.is_some()))
        .or_else(|| results.first())
}

/// 选最终下载源。
///
/// 仅用于 npm registry：内核由 pnpm 下载，没法交给竞速下载器接管，
/// 因此这里保留"先测速再选源"。Node 运行时走 `downloader::race_sources`。
///
/// 缓存源不再无条件复用：缓存只用于在本次测速结果里"打平时优先"，
/// 因为网络环境会变，旧的最快源今天可能是最慢的。
pub async fn choose_registry(
    client: &reqwest::Client,
    _kind: &str,
    cached: Option<String>,
    mirrors: Vec<Mirror>,
) -> Option<String> {
    let results = speed_test(client, &mirrors).await;
    let fastest = pick_fastest(&results)?;

    // 缓存源仍然可用、且与本次最快源速度差在 15% 以内时保留缓存，避免来回抖动。
    if let Some(cached) = cached {
        if let Some(cached_result) = results.iter().find(|r| r.base_url == cached) {
            if let (Some(cached_bps), Some(best_bps)) =
                (cached_result.bytes_per_sec, fastest.bytes_per_sec)
            {
                if cached_bps as f64 >= best_bps as f64 * 0.85 {
                    return Some(cached);
                }
            }
        }
    }
    Some(fastest.base_url.clone())
}

/// 供前端查询当前测速状态（调试/设置页展示）
#[derive(Debug, Serialize, serde::Deserialize)]
pub struct ProbeOut {
    pub name: String,
    pub ttf_ms: Option<u64>,
    pub bytes_per_sec: Option<u64>,
    pub base: String,
}

pub fn probe_out(results: &[SpeedResult]) -> Vec<ProbeOut> {
    results
        .iter()
        .map(|r| ProbeOut {
            name: r.name.to_string(),
            ttf_ms: r.ttf_ms,
            bytes_per_sec: r.bytes_per_sec,
            base: r.base_url.clone(),
        })
        .collect()
}

use serde::Serialize;

#[cfg(test)]
mod tests {
    use super::*;

    fn result(name: &'static str, bps: Option<u64>, ttf: Option<u64>) -> SpeedResult {
        SpeedResult { name, ttf_ms: ttf, bytes_per_sec: bps, base_url: format!("https://{name}/") }
    }

    #[test]
    fn node_mirrors_cover_every_platform_layout() {
        for (platform, arch, expected) in [
            ("darwin", "arm64", "node-v22.22.0-darwin-arm64.tar.gz"),
            ("win32", "x64", "node-v22.22.0-win-x64.zip"),
            ("linux", "x64", "node-v22.22.0-linux-x64.tar.gz"),
        ] {
            let mirrors = node_mirrors("v22.22.0", platform, arch);
            assert!(mirrors.len() >= 5, "候选源不应少于 5 个");
            for mirror in &mirrors {
                assert!(
                    mirror.base_url.ends_with(expected),
                    "{} 的 URL 应以 {expected} 结尾，实际 {}",
                    mirror.name,
                    mirror.base_url
                );
                assert_eq!(mirror.probe_url, mirror.base_url, "测速应打真实下载地址");
            }
        }
    }

    #[test]
    fn official_source_is_kept_for_overseas_users() {
        let mirrors = node_mirrors("v22.22.0", "darwin", "arm64");
        assert!(mirrors.iter().any(|m| m.base_url.contains("nodejs.org")));
        let npm = npm_mirrors();
        assert!(npm.iter().any(|m| m.base_url.contains("registry.npmjs.org")));
    }

    #[test]
    fn throughput_beats_latency_when_ranking() {
        // 首包慢但带宽高的源应该排在首包快、带宽低的源前面。
        let mut results = vec![
            result("slow-but-wide", Some(8_000_000), Some(400)),
            result("fast-but-narrow", Some(50_000), Some(20)),
        ];
        results.sort_by(|a, b| {
            b.bytes_per_sec
                .unwrap_or(0)
                .cmp(&a.bytes_per_sec.unwrap_or(0))
                .then_with(|| a.ttf_ms.unwrap_or(u64::MAX).cmp(&b.ttf_ms.unwrap_or(u64::MAX)))
        });
        assert_eq!(pick_fastest(&results).unwrap().name, "slow-but-wide");
    }

    #[test]
    fn unreachable_sources_are_skipped() {
        let results = vec![
            result("dead", None, None),
            result("alive", Some(1_000_000), Some(100)),
        ];
        assert_eq!(pick_fastest(&results).unwrap().name, "alive");
    }

    #[test]
    fn falls_back_to_latency_then_first_entry() {
        let latency_only = vec![result("dead", None, None), result("probe-ok", None, Some(120))];
        assert_eq!(pick_fastest(&latency_only).unwrap().name, "probe-ok");

        let all_dead = vec![result("first", None, None), result("second", None, None)];
        assert_eq!(pick_fastest(&all_dead).unwrap().name, "first");
    }
}
