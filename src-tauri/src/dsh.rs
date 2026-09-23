//! dsh 进程管理：spawn、健康检查、端口解析、守护、停止。
use crate::paths::Paths;
use std::process::{Child, Command, Stdio};

/// Hide helper consoles on Windows without changing stdio inheritance.
pub fn suppress_console(cmd: &mut Command) -> &mut Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000 | 0x0000_0008);
    }
    cmd
}

/// 启动 dsh 服务。返回 (child, 提示文本)。
/// 用 `dsh web`（node 直接跑 bin.js）监听随机端口。
pub fn spawn_dsh(paths: &Paths, _profile: &str) -> Result<Child, String> {
    let node_exe = crate::runtime::node_bin(paths);
    let entry = crate::kernel::kernel_bin(paths);
    if !crate::runtime::node_ready(paths) {
        return Err("Node 运行时缺失".to_string());
    }
    if !entry.exists() {
        return Err("dsh 内核缺失，请先完成初始化".to_string());
    }

    // dsh 默认 DSH_HOME = ~/.dsh；桌面端用自己的 profile 目录避免污染 CLI
    let dsh_home = paths.root.join("dsh-home");
    std::fs::create_dir_all(&dsh_home).map_err(|e| e.to_string())?;

    // profile 里若残留指向已删除目录的本地插件，dsh 会因无法解析 bundle 直接退出。
    // 启动前先清理，保证内核始终能拉起来。
    let pruned = crate::kernel::prune_missing_profile_bundles(paths);
    if !pruned.is_empty() {
        crate::commands::log_to_file(
            paths,
            &format!("spawn_dsh: 清理失效 profile bundle: {}", pruned.join(", ")),
        );
    }

    // 反过来，装着却没登记在 dependencies 的插件要补回去，否则插件市场看不见它，
    // 用户再也无法通过市场更新或卸载。必须排在 prune 之后：prune 先把目录已消失的
    // 条目从 bundles 里摘掉，这里才不会为它们凭空造出依赖。
    let restored = crate::kernel::restore_unregistered_profile_plugins(paths);
    if !restored.is_empty() {
        crate::commands::log_to_file(
            paths,
            &format!(
                "spawn_dsh: 恢复插件市场登记: {}（此前只在 bundles 里，市场无法管理）",
                restored.join(", ")
            ),
        );
    }

    // 插件市场是 dsh 首次启动时按需写入 profile 的，装好那一刻还没打过补丁，
    // 于是首次打开市场会闪一次终端窗口。每次启动前重新打一次补丁即可覆盖，
    // 补丁函数是幂等的，已打过则直接返回。
    if crate::kernel::suppress_market_console(paths) {
        crate::commands::log_to_file(paths, "spawn_dsh: 已为插件市场补上无窗口启动参数");
    }

    // 旧版插件市场会让新内核直接启动失败（具名导入的符号已被删除，
    // 报 SyntaxError 而不是可降级的缺失服务）。内核起不来 = 界面打不开 =
    // 用户没有任何入口去更新市场，只能由我们在启动前替他升级。
    //
    // 放在这里而不是安装内核时：升级内核后市场还是旧的，同样会撞上；
    // 而且用户可能从任意历史版本升上来，只有每次启动都检查才兜得住。
    if crate::kernel::market_breaks_kernel_boot(paths) {
        crate::commands::log_to_file(
            paths,
            "spawn_dsh: 检测到插件市场与当前内核不兼容（会导致启动失败），正在升级",
        );
        let registry = crate::paths::Settings::load(paths)
            .preferred_registry
            .unwrap_or_else(|| "https://registry.npmjs.org".to_string());
        match crate::kernel::preinstall_market(paths, &registry) {
            Ok(()) if crate::kernel::market_breaks_kernel_boot(paths) => {
                // 升级跑完了但特征还在：装到的仍是旧版。留着它内核必然起不来，
                // 摘掉它至少能让用户进到界面里，再自己处理。
                let removed = crate::kernel::disable_market(paths);
                crate::commands::log_to_file(
                    paths,
                    &format!("spawn_dsh: 市场升级后仍不兼容，已临时停用（成功={removed}）"),
                );
            }
            Ok(()) => {
                crate::commands::log_to_file(paths, "spawn_dsh: 插件市场已升级到兼容版本");
                crate::kernel::suppress_market_console(paths);
            }
            Err(error) => {
                // 多半是离线。停用市场保住主界面，联网后下次启动会自动装回来。
                let removed = crate::kernel::disable_market(paths);
                crate::commands::log_to_file(
                    paths,
                    &format!("spawn_dsh: 市场升级失败（{error}），已临时停用（成功={removed}）"),
                );
            }
        }
    }

    // 内核 0.1.5 起 web-app 自己插入 id: file-upload（官方
    // @deepseek-ai/dsh-client-file-upload）。社区 dsh-file-upload 也用这个
    // id，两边一并存 cordis 直接 TypeError，内核退出 1。重装内核清不掉
    // profile 插件，界面就会一直以为内核坏了、反复重装。
    //
    // 两边干的是同一件事（会话文件上传），社区多出来的语音/OCR 在桌面
    // WebView 里也用不了，所以直接摘掉社区插件，让官方实现接手。
    if crate::kernel::file_upload_plugin_collides(paths) {
        let removed = crate::kernel::disable_community_file_upload(paths);
        crate::commands::log_to_file(
            paths,
            &format!(
                "spawn_dsh: 内核已内置 file-upload，与社区 dsh-file-upload 冲突，已停用社区插件（成功={removed}）"
            ),
        );
    }

    // 构建脚本白名单同理，也必须每次启动都过一遍。
    //
    // pnpm 是在用户第一次安装带原生依赖的插件时，才把 `sharp: set this to true
    // or false` 这类待决占位行写进 pnpm-workspace.yaml 的。只在装内核时放开一次
    // 是不够的——那时占位行还不存在，之后再也没人来改，于是占位行永久留下，
    // 每次安装都以 ERR_PNPM_IGNORED_BUILDS 失败。桌面端没有终端可以让用户自己
    // 运行 pnpm approve-builds，这条路必须由我们兜住。
    if crate::kernel::allow_plugin_build_scripts(paths) {
        crate::commands::log_to_file(paths, "spawn_dsh: 已放开插件原生依赖的构建脚本");
    }

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
        // 把 node 目录放 PATH 首位，让 dsh 能调用 pnpm（插件市场需要）
        .env("PATH", prepend_path(node_exe.parent().unwrap()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        // CREATE_NO_WINDOW only suppresses console allocation when the child does
        // not inherit a console. Detach stdio at the process boundary as well.
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000 | 0x0000_0008);
    }

    let child = cmd.spawn().map_err(|e| format!("启动 dsh 失败: {e}"))?;
    Ok(child)
}

fn prepend_path(node_bin_dir: &std::path::Path) -> std::ffi::OsString {
    let mut paths = vec![node_bin_dir.to_path_buf()];
    if let Ok(existing) = std::env::var("PATH") {
        for p in std::env::split_paths(&existing) {
            paths.push(p);
        }
    }
    std::env::join_paths(paths).unwrap_or_default()
}

/// 内核启动时打印的服务地址。
///
/// 必须整条保留，不能只留端口自己拼。新版内核给 Web 界面加了浏览器鉴权：
/// 打印的 URL 带一次性令牌，带令牌访问 `/` 才会签发 cookie 并跳转到干净的 `/`；
/// 没有令牌也没有 cookie 就返回 401
/// "dsh web authentication required; reopen the URL printed by dsh web."
///
/// 我们不解析也不理解令牌的具体形式，原样转发给 iframe。这样内核以后换成
/// 别的参数名、加别的字段，桌面端都不用跟着改。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DshEndpoint {
    /// 完整地址，含内核给的全部查询参数。
    pub url: String,
    /// 端口。cookie 按 authority（主机+端口）绑定，换端口即失效，
    /// 所以它只用于健康检查与展示，不用于拼访问地址。
    pub port: u16,
}

/// 从进程输出里解析内核打印的服务地址。
///
/// 只认 `http://127.0.0.1:<port>` 开头的 token，整条带回。
///
/// 刻意不提供"只取端口"的变体：那正是这个 bug 的成因——拿端口自己拼地址会丢掉
/// 鉴权令牌。要端口就用返回值里的 `port` 字段，它只该用于健康检查和展示。
pub fn parse_endpoint_from_line(line: &str) -> Option<DshEndpoint> {
    /// 日志里包裹 URL 的标点，前后都要剥掉，否则会被当成地址的一部分。
    const WRAPPERS: [char; 10] = [',', '。', '、', '(', ')', '（', '）', '"', '\'', '`'];

    for raw in line.split_whitespace() {
        let token = raw.trim_start_matches(WRAPPERS);
        let Some(rest) = token.strip_prefix("http://127.0.0.1:") else {
            continue;
        };
        // 端口到第一个非数字为止；其余（路径、?query）都属于 URL 的一部分。
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let Ok(port) = digits.parse::<u16>() else {
            continue;
        };
        let url = token.trim_end_matches(WRAPPERS);
        return Some(DshEndpoint { url: url.to_string(), port });
    }
    None
}

/// 兼容旧调用：只要端口。
#[cfg(test)]
pub fn parse_port_from_line(line: &str) -> Option<u16> {
    parse_endpoint_from_line(line).map(|endpoint| endpoint.port)
}


#[cfg(test)]
mod endpoint_tests {
    use super::*;

    /// 这是本 bug 的回归测试：新版内核打印带令牌的 URL，
    /// 只取端口自己拼地址会丢掉令牌，界面显示
    /// "dsh web authentication required"。
    #[test]
    fn token_query_is_preserved() {
        let line = "  dsh web listening on http://127.0.0.1:52341/?dsh_token=abc123DEF";
        let endpoint = parse_endpoint_from_line(line).expect("应解析出地址");
        assert_eq!(endpoint.port, 52341);
        assert_eq!(endpoint.url, "http://127.0.0.1:52341/?dsh_token=abc123DEF");
        assert!(endpoint.url.contains("abc123DEF"), "令牌必须留在 URL 里");
    }

    #[test]
    fn plain_url_without_token_still_works() {
        // 旧内核不带鉴权，同一套解析要照常可用。
        let endpoint = parse_endpoint_from_line("open http://127.0.0.1:3080/").expect("应解析");
        assert_eq!(endpoint.port, 3080);
        assert_eq!(endpoint.url, "http://127.0.0.1:3080/");
    }

    #[test]
    fn port_only_url_is_accepted() {
        let endpoint = parse_endpoint_from_line("http://127.0.0.1:8080").expect("应解析");
        assert_eq!(endpoint.port, 8080);
        assert_eq!(endpoint.url, "http://127.0.0.1:8080");
    }

    #[test]
    fn multiple_query_params_survive() {
        // 内核以后可能加别的字段，整条带走即可，不要按参数名解析。
        let line = "http://127.0.0.1:1234/?token=x&profile=web-desktop";
        let endpoint = parse_endpoint_from_line(line).expect("应解析");
        assert_eq!(endpoint.url, "http://127.0.0.1:1234/?token=x&profile=web-desktop");
    }

    #[test]
    fn wrapping_punctuation_is_trimmed() {
        // 中文日志常写成「地址：http://...。」，句读不能进 URL。
        let endpoint = parse_endpoint_from_line("服务地址 http://127.0.0.1:5000/?t=1。").expect("应解析");
        assert_eq!(endpoint.url, "http://127.0.0.1:5000/?t=1");
        // 前置引号同样要剥掉，否则 strip_prefix 匹配不上、整条被跳过。
        let quoted = parse_endpoint_from_line("see \"http://127.0.0.1:5000/\",").expect("应解析");
        assert_eq!(quoted.url, "http://127.0.0.1:5000/");
        let paren = parse_endpoint_from_line("(http://127.0.0.1:7000/?t=2)").expect("应解析");
        assert_eq!(paren.url, "http://127.0.0.1:7000/?t=2");
    }

    #[test]
    fn non_matching_lines_yield_nothing() {
        assert_eq!(parse_endpoint_from_line("starting dsh…"), None);
        assert_eq!(parse_endpoint_from_line("http://127.0.0.1:notaport/"), None);
        assert_eq!(parse_endpoint_from_line(""), None);
    }

    #[test]
    fn port_helper_agrees_with_endpoint() {
        let line = "http://127.0.0.1:6001/?token=zz";
        assert_eq!(parse_port_from_line(line), Some(6001));
        assert_eq!(parse_port_from_line(line), parse_endpoint_from_line(line).map(|e| e.port));
    }
}
