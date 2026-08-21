//! dsh 进程管理：spawn、健康检查、端口解析、守护、停止。
use crate::paths::Paths;
use std::process::{Child, Command, Stdio};

/// 启动 dsh 服务。返回 (child, 提示文本)。
/// 用 `dsh web`（node 直接跑 bin.js）监听随机端口。
pub fn spawn_dsh(paths: &Paths, _profile: &str) -> Result<Child, String> {
    let node_exe = crate::runtime::node_bin(paths);
    let entry = crate::kernel::kernel_bin(paths);
    if !node_exe.exists() {
        return Err("Node 运行时缺失".to_string());
    }
    if !entry.exists() {
        return Err("dsh 内核缺失，请先完成初始化".to_string());
    }

    // dsh 默认 DSH_HOME = ~/.dsh；桌面端用自己的 profile 目录避免污染 CLI
    let dsh_home = paths.root.join("dsh-home");
    std::fs::create_dir_all(&dsh_home).map_err(|e| e.to_string())?;

    let mut cmd = Command::new(&node_exe);
    cmd.arg(&entry)
        .args([
            "web",
            "--host",
            "127.0.0.1",
            "--port",
            "0",
            "--no-open",
        ])
        .env("DSH_HOME", &dsh_home)
        .env("NODE_OPTIONS", "--no-warnings")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd.spawn().map_err(|e| format!("启动 dsh 失败: {e}"))?;
    Ok(child)
}

/// 从进程输出里解析 "http://127.0.0.1:<port>"。
/// 空字符串占位：真正的解析在异步读取流中进行，这里提供同步 fallback 通过尝试常见端口。
pub fn parse_port_from_line(line: &str) -> Option<u16> {
    for token in line.split_whitespace() {
        if let Some(rest) = token.strip_prefix("http://127.0.0.1:") {
            if let Some(port_str) = rest.split(['/', ' ', '\t', '\r']).next() {
                if let Ok(p) = port_str.parse::<u16>() {
                    return Some(p);
                }
            }
        }
    }
    None
}
