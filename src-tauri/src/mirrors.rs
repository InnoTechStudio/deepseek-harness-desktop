//! 动态测速选源：并行探测各镜像首包时间，选最快的下载源。
//!
//! 测速只发一个 256B 的 HTTP Range 请求，只取首包到达时间；
//! 并行 + 短超时，总耗时 <1s，对用户几乎无感。
//! 候选源只收录"完整、快、稳定"的厂商级镜像。

use std::time::{Duration, Instant};

/// 单个候选源
#[derive(Debug, Clone)]
pub struct Mirror {
    pub name: &'static str,
    /// 探测用的 URL（256B Range），返回快 = 该源快
    pub probe_url: String,
    /// 实际下载/查版本的 URL 模板
    pub base_url: String,
}

/// npm 镜像候选源（用于版本检测 + 内核 tarball 下载）
pub fn npm_mirrors() -> Vec<Mirror> {
    let pkg = "@deepseek-ai/dsh";
    let encoded = pkg.replace('/', "%2f");
    vec![
        Mirror {
            name: "huawei",
            probe_url: format!("https://repo.huaweicloud.com/repository/npm/{encoded}"),
            base_url: "https://repo.huaweicloud.com/repository/npm/".into(),
        },
        Mirror {
            name: "aliyun",
            probe_url: format!("https://registry.npmmirror.com/{encoded}"),
            base_url: "https://registry.npmmirror.com/".into(),
        },
        Mirror {
            name: "tencent",
            probe_url: format!("https://mirrors.cloud.tencent.com/npm/{encoded}"),
            base_url: "https://mirrors.cloud.tencent.com/npm/".into(),
        },
        Mirror {
            name: "npmjs",
            probe_url: format!("https://registry.npmjs.org/{encoded}"),
            base_url: "https://registry.npmjs.org/".into(),
        },
    ]
}

/// Node runtime 镜像候选源（下载 node tarball）
pub fn node_mirrors(version: &str, platform: &str, arch: &str) -> Vec<Mirror> {
    // Node 目录布局：nodejs.org/dist/<v>/ ；npmmirror: cdn.npmmirror.com/binaries/node/<v>/
    // 平台: darwin/win32/linux ；arch: arm64/x64
    let _os = match platform {
        "win32" => "win",
        _ => platform,
    };
    let node_os = if platform == "win32" { "win" } else { platform };
    let _file = format!("node-{version}-{node_os}-{arch}.tar.gz");
    // Windows 用 zip
    let file_ext = if platform == "win32" { "zip" } else { "tar.gz" };
    let file = format!("node-{version}-{node_os}-{arch}.{file_ext}");

    // 镜像 URL 结构
    // 官方: https://nodejs.org/dist/<v>/<file>
    // npmmirror: https://cdn.npmmirror.com/binaries/node/<v>/<file>
    // 腾讯: https://mirrors.cloud.tencent.com/nodejs-release/<v>/<file>
    // 华为: https://mirrors.huaweicloud.com/nodejs/<v>/<file>
    // probe 用 SHASUMS256.txt（很小，且确认目录存在）
    vec![
        Mirror {
            name: "npmmirror-node",
            probe_url: format!("https://cdn.npmmirror.com/binaries/node/{version}/SHASUMS256.txt"),
            base_url: format!("https://cdn.npmmirror.com/binaries/node/{version}/{file}"),
        },
        Mirror {
            name: "tencent-node",
            probe_url: format!(
                "https://mirrors.cloud.tencent.com/nodejs-release/{version}/SHASUMS256.txt"
            ),
            base_url: format!(
                "https://mirrors.cloud.tencent.com/nodejs-release/{version}/{file}"
            ),
        },
        Mirror {
            name: "huawei-node",
            probe_url: format!("https://mirrors.huaweicloud.com/nodejs/{version}/SHASUMS256.txt"),
            base_url: format!("https://mirrors.huaweicloud.com/nodejs/{version}/{file}"),
        },
        Mirror {
            name: "nodejs-official",
            probe_url: format!("https://nodejs.org/dist/{version}/SHASUMS256.txt"),
            base_url: format!("https://nodejs.org/dist/{version}/{file}"),
        },
    ]
}

/// 测速结果
#[derive(Debug, Clone)]
pub struct SpeedResult {
    pub name: &'static str,
    pub ttf_ms: Option<u64>,
    pub base_url: String,
}

/// 并行测速所有候选源，返回按 ttf 升序排序的结果（最快的在前）
pub async fn speed_test(client: &reqwest::Client, mirrors: &[Mirror]) -> Vec<SpeedResult> {
    let mut handles = Vec::new();
    for m in mirrors {
        let client = client.clone();
        let probe_url = m.probe_url.clone();
        let base_url = m.base_url.clone();
        let name = m.name;
        handles.push(tokio::spawn(async move {
            let start = Instant::now();
            let ok = probe(&client, &probe_url).await;
            let ms = ok.then(|| start.elapsed().as_millis() as u64);
            SpeedResult {
                name,
                ttf_ms: ms,
                base_url,
            }
        }));
    }
    let mut results: Vec<SpeedResult> = Vec::new();
    for h in handles {
        if let Ok(r) = h.await {
            results.push(r);
        }
    }
    results.sort_by_key(|r| r.ttf_ms.unwrap_or(u64::MAX));
    results
}

/// 向某个 URL 发 256B Range 请求，首包到达即成功
async fn probe(client: &reqwest::Client, url: &str) -> bool {
    let Ok(req) = client
        .get(url)
        .header("Range", "bytes=0-255")
        .timeout(Duration::from_millis(3000))
        .build()
    else {
        return false;
    };
    match client.execute(req).await {
        Ok(mut resp) => {
            // 只读一点点就断开，够拿到首包时间
            let _ = resp.chunk().await;
            true
        }
        Err(_) => false,
    }
}

/// 选最快的可用源（有 ttf 的都算可用；都没有则回退第一个）
pub fn pick_fastest(results: &[SpeedResult]) -> Option<&SpeedResult> {
    results
        .iter()
        .find(|r| r.ttf_ms.is_some())
        .or_else(|| results.first())
}

/// 依据历史缓存 + 本次测速，选最终源（先试缓存，失败则用本次最快）
pub async fn choose_registry(
    client: &reqwest::Client,
    _kind: &str,
    cached: Option<String>,
    mirrors: Vec<Mirror>,
) -> Option<String> {
    // 有缓存且缓存源在候选里 → 直接用缓存（省测速）
    if let Some(c) = cached {
        if mirrors.iter().any(|m| m.base_url == c) {
            return Some(c);
        }
    }
    let results = speed_test(client, &mirrors).await;
    pick_fastest(&results).map(|r| r.base_url.clone())
}

/// 供前端查询当前测速状态（调试/设置页展示）
#[derive(Debug, Serialize, serde::Deserialize)]
pub struct ProbeOut {
    pub name: String,
    pub ttf_ms: Option<u64>,
    pub base: String,
}

pub fn probe_out(results: &[SpeedResult]) -> Vec<ProbeOut> {
    results
        .iter()
        .map(|r| ProbeOut {
            name: r.name.to_string(),
            ttf_ms: r.ttf_ms,
            base: r.base_url.clone(),
        })
        .collect()
}

use serde::Serialize;
