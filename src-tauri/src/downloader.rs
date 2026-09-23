//! 下载器：竞速选源 + 断点续传 + 进度回调。
//!
//! 不做分片。实测（48 MB 的 Node 包，华为云镜像）证明多连接分片没有收益：
//! 家用千兆单连接 36 MB/s、8 连接 32 MB/s；手机热点单连接 9.8 MB/s、
//! 8 连接 9.2 MB/s。两种链路上分片都更慢，因为多路 TLS 握手和并发写盘
//! 的开销盖过了并发收益。真正决定速度的是选对镜像——最快源与最慢源
//! 之间实测差了 30 倍以上。
use futures::StreamExt;
use reqwest::header::{CONTENT_RANGE, RANGE};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

/// 下载进度回调（发给前端）
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub kind: String,       // "node" | "kernel"
    pub received: u64,      // 已下载字节
    pub total: Option<u64>, // 总字节（未知为 None）
    pub percent: Option<f64>,
}

/// 诊断信息回调（写入 app.log，不面向用户）。
pub type LogFn = Option<Arc<dyn Fn(String) + Send + Sync>>;

/// 竞速阶段每个源最多下载的数据量。
///
/// 这个值同时决定测量精度和浪费的流量：太小则测量落在 TCP 慢启动区间、
/// 高带宽源被低估；太大则落败源白下载的数据变多。2 MB 在千兆链路上约
/// 170 ms 完成，足以跨过慢启动。胜出源的这部分会直接续传，不算浪费。
const RACE_SLICE_BYTES: u64 = 2 * 1024 * 1024;
/// 计算带宽时跳过的预热字节数。
///
/// TCP 慢启动期间拥塞窗口还在指数增长，此时的速率远低于稳态。不跳过
/// 会把真正的高带宽源误判成中等速度，这正是旧版测速排名不稳的原因。
const RACE_WARMUP_BYTES: u64 = 256 * 1024;
/// 已有这么多源跑完整个分片时，仍在传输的源不可能更快，直接取消。
/// 这让快网络下的竞速在几百毫秒内结束，用户几乎无感。
const RACE_SETTLED_WINNERS: usize = 2;
/// 单个源的时限。快源约 200 ms 就能跑完 2 MB；到点后按已下载量排名，
/// 慢链路也能选出相对最快的那个，而不是干等到全部超时。
const RACE_DEADLINE: Duration = Duration::from_millis(1200);
/// 进度回调节流间隔。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(120);

/// 一个候选源的竞速结果。
#[derive(Debug)]
pub struct RaceOutcome {
    /// 胜出的完整下载地址。
    pub url: String,
    /// 稳态带宽（跳过慢启动后计算），仅用于日志。
    pub bytes_per_sec: Option<u64>,
    /// 竞速期间已落盘的字节数，正式下载从这里续传。
    pub warm_bytes: u64,
}

/// 单个源的竞速产出。
struct Contender {
    url: String,
    /// 竞速分片的落盘路径，胜出者会被重命名为正式目标。
    scratch: PathBuf,
    downloaded: u64,
    steady_bps: Option<u64>,
}

/// 并发竞速多个候选源，返回最快的那个及其已下载的数据。
///
/// 与"先测速再下载"相比，竞速把测量和下载合成一步：胜出源在竞速阶段
/// 下载的数据直接作为正式下载的开头续传，不重复传输。
///
/// `candidates` 是同一个文件在不同镜像上的完整 URL。
pub async fn race_sources(
    client: &reqwest::Client,
    candidates: &[String],
    scratch_dir: &Path,
    on_log: &LogFn,
) -> Option<RaceOutcome> {
    if candidates.is_empty() {
        return None;
    }
    // 只有一个候选时没有可比对象，直接返回，省掉一次多余的请求。
    if candidates.len() == 1 {
        return Some(RaceOutcome {
            url: candidates[0].clone(),
            bytes_per_sec: None,
            warm_bytes: 0,
        });
    }

    tokio::fs::create_dir_all(scratch_dir).await.ok()?;
    let finished = Arc::new(AtomicU64::new(0));
    let mut running = tokio::task::JoinSet::new();

    for (index, url) in candidates.iter().enumerate() {
        let client = client.clone();
        let url = url.clone();
        let scratch = scratch_dir.join(format!("race-{index}.part"));
        let finished = finished.clone();
        running.spawn(async move { run_contender(&client, url, scratch, finished).await });
    }

    // 一旦有足够多的源跑完整个分片，剩下的源不可能更快，直接放弃等待。
    // 否则一个卡住的源会把整个竞速拖到时限，明显延长启动时间。
    let mut results: Vec<Contender> = Vec::with_capacity(candidates.len());
    while let Some(joined) = running.join_next().await {
        if let Ok(Some(contender)) = joined {
            results.push(contender);
        }
        let settled = results.iter().filter(|c| c.steady_bps.is_some()).count();
        if settled >= RACE_SETTLED_WINNERS {
            break;
        }
    }
    // 中止仍在传输的源，并回收它们的临时分片。
    running.abort_all();
    while running.join_next().await.is_some() {}
    if let Ok(mut entries) = tokio::fs::read_dir(scratch_dir).await {
        let keep: Vec<_> = results.iter().map(|c| c.scratch.clone()).collect();
        while let Ok(Some(entry)) = entries.next_entry().await {
            if !keep.contains(&entry.path()) {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }

    // 稳态带宽优先；都没测到（链路太慢，连预热都没跑完）时按已下载量排。
    results.sort_by(|a, b| {
        b.steady_bps
            .unwrap_or(0)
            .cmp(&a.steady_bps.unwrap_or(0))
            .then_with(|| b.downloaded.cmp(&a.downloaded))
    });

    let winner = results.first()?;
    if let Some(log) = on_log {
        let summary = results
            .iter()
            .map(|c| {
                let host = c.url.split('/').nth(2).unwrap_or("?");
                match c.steady_bps {
                    Some(bps) => format!("{host} {} KB/s", bps / 1024),
                    None => format!("{host} {}KB(未达预热)", c.downloaded / 1024),
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        log(format!("race: {summary}"));
    }

    let outcome = RaceOutcome {
        url: winner.url.clone(),
        bytes_per_sec: winner.steady_bps,
        warm_bytes: winner.downloaded,
    };
    let winner_scratch = winner.scratch.clone();

    // 清理落败源的分片，只保留胜出者的。
    for contender in results.iter().skip(1) {
        let _ = tokio::fs::remove_file(&contender.scratch).await;
    }
    if outcome.warm_bytes > 0 {
        let keep = scratch_dir.join("race-winner.part");
        if tokio::fs::rename(&winner_scratch, &keep).await.is_err() {
            let _ = tokio::fs::remove_file(&winner_scratch).await;
            return Some(RaceOutcome { warm_bytes: 0, ..outcome });
        }
    }
    Some(outcome)
}

/// 下载一个竞速分片，测量跳过慢启动后的稳态带宽。
async fn run_contender(
    client: &reqwest::Client,
    url: String,
    scratch: PathBuf,
    finished: Arc<AtomicU64>,
) -> Option<Contender> {
    let started = Instant::now();
    let response = client
        .get(&url)
        .header(RANGE, format!("bytes=0-{}", RACE_SLICE_BYTES - 1))
        .timeout(RACE_DEADLINE)
        .send()
        .await
        .ok()?;
    let status = response.status().as_u16();
    if status != 206 && !response.status().is_success() {
        return None;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&scratch)
        .await
        .ok()?;

    let mut stream = response.bytes_stream();
    let mut downloaded = 0u64;
    // 预热结束的时间点与字节数，之后的传输才用于计算稳态带宽。
    let mut steady_start: Option<(Instant, u64)> = None;
    let mut steady_bps = None;
    let mut completed_slice = false;

    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else { break };
        if file.write_all(&chunk).await.is_err() {
            break;
        }
        downloaded += chunk.len() as u64;

        if steady_start.is_none() && downloaded >= RACE_WARMUP_BYTES {
            steady_start = Some((Instant::now(), downloaded));
        }
        if let Some((mark, mark_bytes)) = steady_start {
            let elapsed = mark.elapsed().as_secs_f64();
            if elapsed > 0.0 && downloaded > mark_bytes {
                steady_bps = Some(((downloaded - mark_bytes) as f64 / elapsed) as u64);
            }
        }

        if downloaded >= RACE_SLICE_BYTES {
            completed_slice = true;
            break;
        }
        // 已有足够多的源跑完，或整体超时 → 放弃本源，避免拖慢启动。
        if finished.load(Ordering::Relaxed) as usize >= RACE_SETTLED_WINNERS
            || started.elapsed() >= RACE_DEADLINE
        {
            break;
        }
    }
    let _ = file.flush().await;
    if completed_slice {
        finished.fetch_add(1, Ordering::Relaxed);
    }
    if downloaded == 0 {
        let _ = tokio::fs::remove_file(&scratch).await;
        return None;
    }
    Some(Contender { url, scratch, downloaded, steady_bps })
}

/// 把 URL 下载到 dest，单连接 + 断点续传。
pub async fn download_with_progress(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    on_progress: Option<Arc<dyn Fn(DownloadProgress) + Send + Sync>>,
    kind: &str,
) -> Result<(), String> {
    download_resumable(client, url, dest, on_progress, kind, None).await
}

/// 与 `download_with_progress` 相同，额外接收诊断日志回调。
pub async fn download_with_diagnostics(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    on_progress: Option<Arc<dyn Fn(DownloadProgress) + Send + Sync>>,
    kind: &str,
    on_log: LogFn,
) -> Result<(), String> {
    download_resumable(client, url, dest, on_progress, kind, on_log).await
}

/// 单连接下载，已有部分文件时用 Range 续传。
///
/// 竞速阶段留下的数据会被当作已下载部分，因此正式下载从中断处继续，
/// 不重复传输已经拿到的字节。
async fn download_resumable(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    on_progress: Option<Arc<dyn Fn(DownloadProgress) + Send + Sync>>,
    kind: &str,
    on_log: LogFn,
) -> Result<(), String> {
    download_resumable_with_hash(client, url, dest, on_progress, kind, on_log, None)
        .await
        .map(|_| ())
}

/// 下载并按给定 sha256（十六进制）流式校验。与 `download_resumable` 同一通路：
/// 进度回调、日志、断点续传起点、写入模式都一致，只是多验一次哈希。
///
/// `expected_hex` 给 None 就只下不验。返回最终的 sha256 十六进制字符串。
pub async fn download_with_sha256(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    on_progress: Option<Arc<dyn Fn(DownloadProgress) + Send + Sync>>,
    kind: &str,
    on_log: LogFn,
    expected_hex: Option<&str>,
) -> Result<String, String> {
    download_resumable_with_hash(client, url, dest, on_progress, kind, on_log, expected_hex).await
}

/// `download_resumable` 之上加一项 sha256 校验。与 `download_resumable` 共用全部
/// 细节：进度回调、日志、断点续传起点、写入模式——只有 sha256 不一样。
///
/// 给一个 `expected_hex` 就到后比对，不符即报错；给 None 就只下不验。
async fn download_resumable_with_hash(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    on_progress: Option<Arc<dyn Fn(DownloadProgress) + Send + Sync>>,
    kind: &str,
    on_log: LogFn,
    expected_hex: Option<&str>,
) -> Result<String, String> {
    use sha2::Digest;

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    }

    // 断点续传：已下载部分已落盘，直接对整份计算 sha256。
    let mut hasher = sha2::Sha256::new();
    if let Ok(existing) = tokio::fs::read(dest).await {
        hasher.update(&existing);
        if let Some(log) = &on_log {
            log(format!("download[{kind}]: 已有 {} KB 待续传", existing.len() / 1024));
        }
    }
    let existing = match tokio::fs::metadata(dest).await {
        Ok(meta) => meta.len(),
        Err(_) => 0,
    };

    let mut request = client.get(url);
    if existing > 0 {
        request = request.header(RANGE, format!("bytes={existing}-"));
    }
    let response = request.send().await.map_err(|e| format!("下载失败: {e}"))?;
    let status = response.status();
    if !status.is_success() && status.as_u16() != 206 {
        return Err(format!("下载失败: HTTP {}", status.as_u16()));
    }

    // 服务端接受续传时确认起点一致；忽略 Range 时从头重下。
    let append = existing > 0 && status.as_u16() == 206;
    if append {
        let range = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| "断点响应缺少 Content-Range".to_string())?;
        let start = range
            .strip_prefix("bytes ")
            .and_then(|v| v.split('-').next())
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| "断点响应 Content-Range 无效".to_string())?;
        if start != existing {
            return Err(format!("断点起点不匹配: 期望 {existing}，收到 {start}"));
        }
        if let Some(log) = &on_log {
            log(format!("download[{kind}]: 从 {} KB 处续传", existing / 1024));
        }
    }

    let total = response
        .content_length()
        .map(|n| n + if append { existing } else { 0 });
    let mut file = if append {
        OpenOptions::new().create(true).append(true).open(dest).await
    } else {
        OpenOptions::new().create(true).write(true).truncate(true).open(dest).await
    }
        .map_err(|e| e.to_string())?;

    let mut received = if append { existing } else { 0 };
    let mut last_callback = Instant::now() - PROGRESS_INTERVAL;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("下载中断: {e}"))?;
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| format!("写入失败: {e}"))?;
        received += chunk.len() as u64;
        if last_callback.elapsed() >= PROGRESS_INTERVAL {
            last_callback = Instant::now();
            report(&on_progress, kind, received, total);
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    report(&on_progress, kind, received, total.or(Some(received)));

    let actual = hex::encode(hasher.finalize());
    if let Some(expected) = expected_hex {
        let expected = expected.trim().to_ascii_lowercase();
        if actual != expected {
            return Err(format!("安装包校验失败：期望 sha256 {}…，实际 {}…", &expected[..16.min(expected.len())], &actual[..16]));
        }
        if let Some(log) = &on_log {
            log(format!("download[{kind}]: sha256 校验通过 {}", &actual[..16]));
        }
    }
    Ok(actual)
}

fn report(
    on_progress: &Option<Arc<dyn Fn(DownloadProgress) + Send + Sync>>,
    kind: &str,
    received: u64,
    total: Option<u64>,
) {
    if let Some(cb) = on_progress {
        cb(DownloadProgress {
            kind: kind.to_string(),
            received,
            total,
            percent: total.filter(|t| *t > 0).map(|t| (received as f64 / t as f64 * 100.0).min(100.0)),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warmup_is_smaller_than_race_slice() {
        // 预热必须留出足够的稳态测量区间，否则算不出带宽。
        assert!(RACE_WARMUP_BYTES < RACE_SLICE_BYTES / 2);
    }

    #[test]
    fn race_slice_crosses_tcp_slow_start() {
        // 慢启动约在 128~256 KB 内结束；分片必须显著大于这个量级。
        assert!(RACE_SLICE_BYTES >= 1024 * 1024);
    }

    #[test]
    fn deadline_stays_imperceptible() {
        // 竞速是启动路径的一部分，超过 1.5 秒用户就能感觉到卡顿。
        assert!(RACE_DEADLINE <= Duration::from_millis(1500));
    }

    #[tokio::test]
    async fn empty_candidate_list_yields_no_winner() {
        let client = reqwest::Client::new();
        let dir = std::env::temp_dir().join("dsh-race-empty");
        assert!(race_sources(&client, &[], &dir, &None).await.is_none());
    }

    #[tokio::test]
    async fn single_candidate_skips_the_race() {
        // 只有一个源时无需比较，应直接返回且不产生预热数据。
        let client = reqwest::Client::new();
        let dir = std::env::temp_dir().join("dsh-race-single");
        let only = vec!["https://example.invalid/node.tar.gz".to_string()];
        let outcome = race_sources(&client, &only, &dir, &None).await.unwrap();
        assert_eq!(outcome.url, only[0]);
        assert_eq!(outcome.warm_bytes, 0);
        assert!(outcome.bytes_per_sec.is_none());
    }

    #[tokio::test]
    async fn all_unreachable_candidates_yield_no_winner() {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(300))
            .build()
            .unwrap();
        let dir = std::env::temp_dir().join("dsh-race-dead");
        let dead = vec![
            "https://no-such-host-a.invalid/f".to_string(),
            "https://no-such-host-b.invalid/f".to_string(),
        ];
        assert!(race_sources(&client, &dead, &dir, &None).await.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 需要真实网络的手动验证：`cargo test -- --ignored race_ --nocapture`
#[cfg(test)]
mod net_tests {
    use super::*;

    fn node_candidates() -> Vec<String> {
        crate::mirrors::node_mirrors("v22.22.0", "darwin", "arm64")
            .into_iter()
            .map(|m| m.base_url)
            .collect()
    }

    #[tokio::test]
    #[ignore = "需要网络：验证竞速选源的耗时与选择结果"]
    async fn race_picks_a_fast_mirror_quickly() {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let dir = std::env::temp_dir().join(format!("dsh-race-live-{}", std::process::id()));
        let candidates = node_candidates();

        let started = Instant::now();
        let outcome = race_sources(&client, &candidates, &dir, &None)
            .await
            .expect("至少应有一个源可用");
        let elapsed = started.elapsed();

        eprintln!(
            "竞速耗时 {:.2}s，选中 {}（{} KB/s），预热 {} KB",
            elapsed.as_secs_f64(),
            outcome.url.split('/').nth(2).unwrap_or("?"),
            outcome.bytes_per_sec.map(|b| b / 1024).unwrap_or(0),
            outcome.warm_bytes / 1024
        );

        assert!(elapsed <= RACE_DEADLINE + Duration::from_millis(800), "竞速不应明显超过时限");
        assert!(outcome.warm_bytes > 0, "胜出源应留下可续传的数据");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    #[ignore = "需要网络：验证竞速+续传的端到端下载速度"]
    async fn race_then_resume_downloads_whole_file() {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let dir = std::env::temp_dir().join(format!("dsh-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("node.tar.gz");

        let started = Instant::now();
        let outcome = race_sources(&client, &node_candidates(), &dir, &None)
            .await
            .expect("至少应有一个源可用");
        // 把竞速数据当作已下载部分，验证续传衔接正确。
        let part = dir.join("race-winner.part");
        if part.exists() {
            std::fs::rename(&part, &dest).unwrap();
        }
        download_resumable(&client, &outcome.url, &dest, None, "node", None)
            .await
            .expect("续传下载应成功");
        let elapsed = started.elapsed();

        let size = std::fs::metadata(&dest).unwrap().len();
        eprintln!(
            "端到端 {} 字节 / {:.2}s = {:.0} KB/s（含竞速）",
            size,
            elapsed.as_secs_f64(),
            size as f64 / elapsed.as_secs_f64() / 1024.0
        );
        assert!(size > 40 * 1024 * 1024, "Node 包应约 48 MB，实际 {size}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
