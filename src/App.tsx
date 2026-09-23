import React, { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./App.css";

type KernelState = {
  nodeReady: boolean;
  kernelReady: boolean;
  installedVersion: string | null;
  running: boolean;
  port: number | null;
  /** 内核打印的完整访问地址，含鉴权令牌。加载 iframe 必须用它。 */
  url: string | null;
  platform: string;
};

type DownloadProgress = {
  kind: string;
  received: number;
  total: number | null;
  percent: number | null;
};

type UpdateInfo = {
  installed: string | null;
  latest: string;
  updateAvailable: boolean;
  clientUpdate?: {
    current_version: string;
    latest_version: string;
    update_available: boolean;
    download_url?: string;
    release_notes?: string;
  };
};

type BootstrapProgress = {
  step: number;
  percent: number;
  message: string;
};

/** 客户端版本号，构建时由 Vite 从 package.json 注入。 */
const APP_VERSION = __APP_VERSION__;

const STEPS = [  { id: "prep", title: "准备运行环境", desc: "下载并配置本地运行环境" },
  { id: "kernel", title: "安装 DeepSeek Harness 内核", desc: "获取官方最新版内核" },
  { id: "launch", title: "启动本地服务", desc: "启动并等待服务就绪" },
];

function WindowsWindowControls({ darkMode }: { darkMode: boolean }) {
  const runWindowAction = (event: React.PointerEvent<HTMLButtonElement>, command: string) => {
    event.preventDefault();
    event.stopPropagation();
    void invoke(command).catch((error) => console.error("Failed to control window:", error));
  };

  return (
    <div
      className="windows-window-controls"
      data-theme={darkMode ? "dark" : "light"}
      aria-label="窗口控制"
      onPointerDown={(event) => event.stopPropagation()}
    >
      <button type="button" aria-label="最小化" title="最小化"
        onPointerDown={(event) => runWindowAction(event, "window_minimize")}>
        <span className="window-control-minimize" aria-hidden="true" />
      </button>
      <button type="button" aria-label="最大化或还原" title="最大化或还原"
        onPointerDown={(event) => runWindowAction(event, "window_toggle_maximize")}>
        <span className="window-control-maximize" aria-hidden="true" />
      </button>
      <button type="button" aria-label="关闭" title="关闭" className="window-control-close"
        onPointerDown={(event) => runWindowAction(event, "window_close")}>
        <span className="window-control-close-icon" aria-hidden="true" />
      </button>
    </div>
  );
}

function App() {
  const [state, setState] = useState<KernelState | null>(null);
  const [booting, setBooting] = useState(true);
  const [step, setStep] = useState(0);
  const [bootstrapProgress, setBootstrapProgress] = useState<BootstrapProgress>({ step: 0, percent: 0, message: "准备运行环境" });
  const [logLines, setLogLines] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [dshUrl, setDshUrl] = useState<string | null>(null);
  const [recovering, setRecovering] = useState(false);
  const [iframeKey, setIframeKey] = useState(0);
  const [aboutOpen, setAboutOpen] = useState(false);
  const [feedbackOpen, setFeedbackOpen] = useState(false);
  const [updateDialogOpen, setUpdateDialogOpen] = useState(false);
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [updateChecking, setUpdateChecking] = useState(false);
  const [darkMode, setDarkMode] = useState(false);
  const [themeOverride, setThemeOverride] = useState(false);
  const [updateAvailable, setUpdateAvailable] = useState(false);
  const iframeRef = useRef<HTMLIFrameElement>(null);

  const checkAndShowUpdates = async () => {
    if (updateChecking) return;
    setUpdateChecking(true);
    try {
      const [clientInfo, kernelInfo] = await Promise.all([
        invoke<UpdateInfo["clientUpdate"]>("check_client_update"),
        invoke<UpdateInfo>("check_update"),
      ]);
      setUpdateInfo({ ...kernelInfo, clientUpdate: clientInfo });
      setUpdateDialogOpen(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setUpdateChecking(false);
    }
  };

  const refreshState = async () => {
    try {
      const s = await invoke<KernelState>("get_state");
      setState(s);
      // 必须用内核给的完整地址，不能按端口自己拼：新版内核要靠 URL 里的
      // 一次性令牌换鉴权 cookie，拼出来的地址会被判 401，界面显示
      // "dsh web authentication required"。
      if (s.kernelReady && s.running && s.url) {
        setDshUrl(s.url);
      }
      return s;
    } catch (e) {
      setError(String(e));
      setBooting(false);
      return null;
    }
  };

  // WKWebView 会把未 preventDefault 的文件拖放当成导航，图片变成全屏。
  // iframe 里的高级外壳也会拦一层；宿主这边再兜一次，免得拖到标题栏/
  // iframe 以外的区域时整个窗口被换成 file URL。
  useEffect(() => {
    const blockNavigation = (event: DragEvent) => {
      const types = event.dataTransfer?.types;
      if (types === undefined || !types.includes("Files")) return;
      event.preventDefault();
    };
    document.addEventListener("dragover", blockNavigation, true);
    document.addEventListener("drop", blockNavigation, true);
    return () => {
      document.removeEventListener("dragover", blockNavigation, true);
      document.removeEventListener("drop", blockNavigation, true);
    };
  }, []);

  // 检测系统主题
  // 系统外观偏好。DSH 报过自己的主题后就以 DSH 为准，不再被系统偏好覆盖。
  useEffect(() => {
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    if (!themeOverride) setDarkMode(mediaQuery.matches);
    const handler = (e: MediaQueryListEvent) => {
      if (!themeOverride) setDarkMode(e.matches);
    };
    mediaQuery.addEventListener("change", handler);
    return () => mediaQuery.removeEventListener("change", handler);
  }, [themeOverride]);

  // 监听 iframe 内主题变化、受限的原生窗口动作，以及设置读写请求
  useEffect(() => {
    /** DSH 里的设置分区只能通过这些命令访问宿主，白名单避免页面调用任意 command。 */
    const COMMANDS: Record<string, string> = {
      "get-settings": "get_settings",
      "set-settings": "set_settings",
      "get-state": "get_state",
      "send-test-notification": "send_test_notification",
      "open-notification-settings": "open_notification_settings",
      "check-client-update": "check_client_update",
      "open-path": "open_path",
    };

    const respond = (source: MessageEventSource, id: string, ok: boolean, body: unknown) => {
      const payload = ok ? { data: body } : { error: String(body).replace(/^Error:\s*/, "") };
      (source as Window).postMessage({ type: "dsh-desktop:response", id, ok, ...payload }, "*");
    };

    const handleMessage = (e: MessageEvent) => {
      const message = e.data;
      if (!message || typeof message !== "object" || typeof message.type !== "string") return;

      const iframeWindow = iframeRef.current?.contentWindow;
      const expectedOrigin = dshUrl ? new URL(dshUrl).origin : null;
      if (!iframeWindow || e.source !== iframeWindow) return;
      if (expectedOrigin && e.origin !== expectedOrigin) return;

      if (message.type === "theme-change" && typeof message.dark === "boolean") {
        // DSH 内部主题优先于系统偏好：宿主自绘的 Windows 控件必须跟随它，
        // 否则在 DSH 里切到深色后，控件图标会是深色描在深色标题栏上。
        setThemeOverride(message.dark);
        setDarkMode(message.dark);
      } else if (message.type === "dsh-desktop:start-dragging") {
        void getCurrentWindow().startDragging().catch((error) => console.error("Failed to drag window:", error));
      } else if (message.type === "dsh-desktop:request-update-state") {
        // iframe 就绪或刷新后主动索要一次，避免头 20 秒按钮状态未知。
        void invoke("refresh_update_availability").catch(() => { /* 离线时保持隐藏 */ });
      } else if (message.type === "dsh-desktop:check-updates"
        || message.type === "dsh-desktop:open-check-updates") {
        void checkAndShowUpdates();
      } else if (message.type === "dsh-desktop:open-about") {
        setAboutOpen(true);
      } else if (message.type === "dsh-desktop:open-feedback") {
        setFeedbackOpen(true);
      } else if (message.type === "dsh-desktop:request") {
        const { id, command, payload } = message as { id?: unknown; command?: unknown; payload?: unknown };
        if (typeof id !== "string" || typeof command !== "string" || !e.source) return;
        const invokeName = COMMANDS[command];
        if (!invokeName) {
          respond(e.source, id, false, `不支持的命令: ${command}`);
          return;
        }
        void invoke(invokeName, (payload ?? {}) as Record<string, unknown>)
          .then((data) => {
            if (e.source) respond(e.source, id, true, data);
          })
          .catch((error) => { if (e.source) respond(e.source, id, false, error); });
      }
    };
    window.addEventListener("message", handleMessage);
    return () => window.removeEventListener("message", handleMessage);
  }, [dshUrl]);

  useEffect(() => {
    refreshState();
    const unsubs: Promise<UnlistenFn>[] = [];
    unsubs.push(listen<DownloadProgress>("download-progress", (e) => {
      const percent = e.payload.percent ?? 0;
      setBootstrapProgress({ step: 0, percent: percent * 0.30, message: "下载 Node 运行时" });
    }));
    unsubs.push(listen<BootstrapProgress>("bootstrap-progress", (e) => {
      setBootstrapProgress(e.payload);
      setStep(e.payload.step);
    }));
    unsubs.push(
      listen<string>("kernel-log", (e) => setLogLines((prev) => [...prev.slice(-80), e.payload])),
    );
    unsubs.push(listen("refresh-dsh", () => setIframeKey((key) => key + 1)));
    unsubs.push(
      listen<{ port: number; url?: string; healthy: boolean }>("dsh-restarted", (e) => {
        if (!e.payload.healthy) return;
        setRecovering(false);
        // 内核每次启动的令牌都不同，端口也可能变，必须换用它这次打印的地址。
        // 拿不到时回落到 refreshState()（它会从 get_state 取），
        // 而不是按端口拼一个必然 401 的地址。
        if (e.payload.url) {
          setDshUrl(e.payload.url);
          setIframeKey((key) => key + 1);
        }
        refreshState();
      }),
    );
    unsubs.push(listen("dsh-restarting", () => setRecovering(true)));
    unsubs.push(listen("show-about-dialog", () => setAboutOpen(true)));
    unsubs.push(
      listen<UpdateInfo>("show-update-result", (e) => {
        setUpdateInfo(e.payload);
        setUpdateDialogOpen(true);
      }),
    );
    // 标题栏"新版本"按钮的显示依据。每轮检查都会上报，含"没有更新"，
    // 所以用户装完新版后按钮会自己消失。
    unsubs.push(
      listen<{ available: boolean }>("update-availability", (e) => {
        setUpdateAvailable(e.payload.available === true);
      }),
    );
    return () => {
      unsubs.forEach((p) => p.then((u) => u()));
    };
  }, []);

  useEffect(() => {
    const run = async () => {
      const s = await refreshState();
      if (!s) return;
      if (s.nodeReady && s.kernelReady && s.running) {
        setBooting(false);
        return;
      }
      setBooting(true);
      try {
        if (!s.nodeReady) {
          setStep(0);
          setBootstrapProgress({ step: 0, percent: 0, message: "准备 Node 运行时" });
          await invoke("bootstrap");
        } else {
          setBootstrapProgress({ step: 0, percent: 45, message: "Node 运行时已就绪" });
        }
        if (!s.kernelReady) {
          setStep(1);
          setBootstrapProgress({ step: 1, percent: 45, message: "准备 DSH 内核" });
          await invoke("install_kernel");
        } else {
          setBootstrapProgress({ step: 1, percent: 98, message: "DSH 内核已就绪" });
        }
        setStep(2);
        setBootstrapProgress({ step: 2, percent: 98, message: "启动 DSH 服务" });
        await invoke<number>("start_dsh");
        // 地址统一从 get_state 取（它带内核给的鉴权令牌），不按端口拼。
        const started = await refreshState();
        if (started?.url) setDshUrl(started.url);
        setBooting(false);
      } catch (e) {
        setError(String(e));
        setBooting(false);
      }
    };
    run();
  }, []);

  // 更新状态变化时同步给 DSH 里的高级外壳，由它渲染标题栏按钮。
  useEffect(() => {
    const target = iframeRef.current?.contentWindow;
    if (!target) return;
    target.postMessage({ type: "dsh-desktop:update-availability", available: updateAvailable }, "*");
  }, [updateAvailable, iframeKey]);

  if (!booting && dshUrl) {
    return (
      <div className={`main-view ${darkMode ? "dark" : ""}`}>
        {recovering && (
          <div className="restart-overlay">
            <div className="restart-spinner"></div>
            <div className="restart-text">正在重启 dsh 服务…</div>
          </div>
        )}
        {/*
          allow 必须显式写出：iframe 与宿主跨源（宿主是 tauri://localhost，
          内核走 http://127.0.0.1:<代理端口>），Permissions-Policy 默认只授权给
          同源文档，跨源 iframe 一律被拒。

          Windows 的 WebView2 是 Chromium，严格执行这条策略，所以内核里的复制按钮
          调 navigator.clipboard.writeText() 会直接被拒、静默失败；macOS 的
          WKWebView 不执行 Permissions-Policy，同一份代码照常能复制——
          这就是「Mac 能复制、Windows 不能」的原因，不是内核的问题。
        */}
        <iframe
          ref={iframeRef}
          key={iframeKey}
          src={dshUrl}
          title="DeepSeek Harness"
          className="main-iframe"
          allow="clipboard-read; clipboard-write; microphone; camera"
        />
        {state?.platform === "win32" && <WindowsWindowControls darkMode={darkMode} />}
        {aboutOpen && <AboutDialog state={state} darkMode={darkMode} onClose={() => setAboutOpen(false)} onCheckUpdates={() => { setAboutOpen(false); checkAndShowUpdates(); }} />}
        {feedbackOpen && <FeedbackDialog darkMode={darkMode} onClose={() => setFeedbackOpen(false)} />}
        {updateDialogOpen && updateInfo && (
          <UpdateDialog
            info={updateInfo}
            darkMode={darkMode}
            onClose={() => setUpdateDialogOpen(false)}
          />
        )}
      </div>
    );
  }

  return (
    <div className={`boot-view ${darkMode ? "dark" : ""}`}>
      {/* 引导阶段还没有 DSH 界面提供拖动区，宿主自己铺一条标题栏。
          macOS 需要避开左上角交通灯，否则会挡住关闭按钮。 */}
      <div
        className="boot-titlebar"
        data-platform={state?.platform ?? "unknown"}
        data-tauri-drag-region
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          void getCurrentWindow().startDragging().catch((e) => console.error("Failed to drag window:", e));
        }}
        onDoubleClick={() => void invoke("window_toggle_maximize").catch((e) => console.error(e))}
      />
      {state?.platform === "win32" && <WindowsWindowControls darkMode={darkMode} />}
      <div className="boot-container">
        <div className="boot-header">
          <h1>DeepSeek Harness</h1>
          <p className="boot-subtitle">DeepSeek Harness 桌面客户端</p>
        </div>
        {booting ? (
          <div className="boot-content">
            <div className="boot-steps">
              {STEPS.map((s, i) => (
                <div key={s.id} className={`boot-step ${i === step ? "active" : i < step ? "done" : ""}`}>
                  <div className="step-indicator">
                    {i < step ? "✓" : i === step ? <div className="step-spinner"></div> : i + 1}
                  </div>
                  <div className="step-info">
                    <div className="step-title">{s.title}</div>
                    <div className="step-desc">{s.desc}</div>
                  </div>
                </div>
              ))}
            </div>
            <div className="boot-progress">
              <div className="progress-bar">
                <div className="progress-fill" style={{ width: `${bootstrapProgress.percent}%` }} />
              </div>
              <span className="progress-text">{bootstrapProgress.percent.toFixed(0)}%</span>
            </div>
            <div className="boot-progress-message">{bootstrapProgress.message}</div>
            {logLines.length > 0 && (
              <div className="boot-log">{logLines.slice(-3).join("\n")}</div>
            )}
          </div>
        ) : error ? (
          <div className="boot-error">
            <div className="error-icon">⚠️</div>
            <div className="error-message">{error}</div>
            <button onClick={() => location.reload()} className="retry-button">
              重试
            </button>
          </div>
        ) : (
          <div className="boot-success">初始化完成，正在打开…</div>
        )}
      </div>
    </div>
  );
}

function FeedbackDialog({ darkMode, onClose }: { darkMode: boolean; onClose: () => void }) {
  const [description, setDescription] = useState("");
  const [includeLogs, setIncludeLogs] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => { inputRef.current?.focus(); }, []);

  const submit = async () => {
    if (submitting || !description.trim()) return;
    setSubmitting(true);
    setError(null);
    try {
      // 环境信息、分类、时间与日志都由后端采集，前端只负责用户填写的部分。
      await invoke<string>("submit_feedback", { description, includeLogs });
      setSubmitted(true);
    } catch (e) {
      // 失败时保留输入内容，用户可以直接再点一次提交。
      setError(String(e).replace(/^Error:\s*/, ""));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className={`dialog-backdrop ${darkMode ? "dark" : ""}`} onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="dialog-box feedback-dialog">
        <div className="dialog-header">
          <h3>{submitted ? "反馈已提交" : "反馈问题"}</h3>
          <button className="dialog-close" onClick={onClose} aria-label="关闭">
            <svg width="24" height="24" viewBox="0 0 16 16" fill="currentColor">
              <path d="M4.646 4.646a.5.5 0 01.708 0L8 7.293l2.646-2.647a.5.5 0 01.708.708L8.707 8l2.647 2.646a.5.5 0 01-.708.708L8 8.707l-2.646 2.647a.5.5 0 01-.708-.708L7.293 8 4.646 5.354a.5.5 0 010-.708z"/>
            </svg>
          </button>
        </div>
        <div className="dialog-body">
          {submitted ? (
            <div className="feedback-success">
              <div className="update-success-icon">✓</div>
              <div className="feedback-success-text">已打开浏览器，请在打开的页面里提交 Issue 完成反馈。</div>
              <div className="feedback-success-text">如果浏览器没有自动打开，可以检查一下是否已登录 GitHub，或手动访问项目主页。</div>
            </div>
          ) : (
            <>
              <div className="feedback-hint">
                描述你遇到的问题或建议即可，系统版本、软件版本、运行状态等信息会自动附上。
              </div>
              <textarea
                ref={inputRef}
                className="feedback-input"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="例如：点击设置里的插件市场后会闪一下终端窗口"
                rows={7}
              />
              <label className="feedback-logs">
                <input type="checkbox" checked={includeLogs} onChange={(e) => setIncludeLogs(e.target.checked)} />
                <span>
                  附带运行日志
                  <em>（帮助定位问题，可能包含本机用户名和文件路径）</em>
                </span>
              </label>
              {error && <div className="setting-error" role="alert">{error}</div>}
              <button
                className="download-btn feedback-submit"
                onClick={() => void submit()}
                disabled={submitting || !description.trim()}
              >
                <span className="download-btn-label">{submitting ? "提交中…" : "提交反馈"}</span>
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function AboutDialog({ state, darkMode, onClose, onCheckUpdates }: {
  state: KernelState | null;
  darkMode: boolean;
  onClose: () => void;
  onCheckUpdates: () => void;
}) {
  return (
    <div className={`dialog-backdrop ${darkMode ? "dark" : ""}`} onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="dialog-box about-dialog">
        <button className="dialog-close" onClick={onClose}>
          <svg width="24" height="24" viewBox="0 0 16 16" fill="currentColor">
            <path d="M4.646 4.646a.5.5 0 01.708 0L8 7.293l2.646-2.647a.5.5 0 01.708.708L8.707 8l2.647 2.646a.5.5 0 01-.708.708L8 8.707l-2.646 2.647a.5.5 0 01-.708-.708L7.293 8 4.646 5.354a.5.5 0 010-.708z"/>
          </svg>
        </button>

        <div className="about-simple">
          <img src="/app-icon.png" alt="DeepSeek Harness" className="about-icon" />

          <h2 className="about-app-name">DeepSeek Harness</h2>

          <div className="about-versions">
            <div className="version-line">客户端 v{APP_VERSION} · 内核 v{state?.installedVersion || "未知"}</div>
          </div>

          <button
            onClick={onCheckUpdates}
            className="check-update-btn"
          >
            检查更新
          </button>

          <div className="about-copyright">Copyright © 2026 INNOTECH<br/>本程序依据 MIT License 发布</div>
        </div>
      </div>
    </div>
  );
}

function UpdateDialog({
  info,
  darkMode,
  onClose,
}: {
  info: UpdateInfo;
  darkMode: boolean;
  onClose: () => void;
}) {
  const hasClientUpdate = info.clientUpdate?.update_available;
  const hasKernelUpdate = info.updateAvailable;
  const hasAnyUpdate = hasClientUpdate || hasKernelUpdate;

  const [downloadState, setDownloadState] = React.useState<"idle" | "downloading" | "success" | "error">("idle");
  const [downloadPercent, setDownloadPercent] = React.useState(0);
  const [downloadError, setDownloadError] = React.useState<string | null>(null);
  /** 内核升级当前阶段的文字，例如"正在导入依赖 12/40"。 */
  const [kernelStage, setKernelStage] = React.useState<string | null>(null);

  React.useEffect(() => {
    let active = true;
    const unlisten = listen<{ percent?: number }>("client-update-progress", (event) => {
      if (active) setDownloadPercent(Math.round(event.payload.percent ?? 0));
    });
    return () => {
      active = false;
      unlisten.then((stop) => stop());
    };
  }, []);

  // 内核升级的进度与客户端下载分开：内核走 pnpm，阶段和百分比都由后端算好。
  React.useEffect(() => {
    let active = true;
    const unlisten = listen<{ percent?: number; label?: string }>("kernel-update-progress", (event) => {
      if (!active) return;
      setDownloadPercent(Math.round(event.payload.percent ?? 0));
      if (typeof event.payload.label === "string") setKernelStage(event.payload.label);
    });
    return () => {
      active = false;
      unlisten.then((stop) => stop());
    };
  }, []);

  const handleDownloadUpdate = async () => {
    setDownloadState("downloading");
    setDownloadPercent(0);
    setDownloadError(null);
    try {
      // 后端会自己拿当前平台清单里的下载地址和 sha256 校验。
      await invoke("download_client_update");
      setDownloadState("success");
    } catch (e) {
      setDownloadState("error");
      setDownloadError(String(e));
    }
  };

  const handleApplyKernelUpdate = async () => {
    setDownloadState("downloading");
    setDownloadPercent(0);
    setKernelStage("正在准备更新");
    setDownloadError(null);
    try {
      await invoke("apply_update");
      // 必须等重启结果：更新流程已经把旧内核杀掉且清空了运行状态，
      // 这一步失败就等于"更新完成但服务没了"，用户重启软件会被打回
      // 首次安装流程。以前这里是 setTimeout 即发即忘，失败无人知晓，
      // 界面照样显示"已更新"。
      setKernelStage("正在重启服务");
      setDownloadPercent(98);
      await invoke("restart_dsh");
      setDownloadPercent(100);
      setKernelStage(null);
      setDownloadState("success");
      // 重启成功后自动收起对话框：内核已经换新、iframe 也由 dsh-restarted
      // 事件刷过了，继续停在"已更新"上只会让用户不确定还要不要做什么。
      // 留一点时间让"已更新"可见，再关掉。
      window.setTimeout(onClose, 1200);
    } catch (e) {
      setDownloadState("error");
      setKernelStage(null);
      setDownloadError(String(e));
    }
  };

  return (
    <div className={`dialog-backdrop ${darkMode ? "dark" : ""}`} onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="dialog-box">
        <div className="dialog-header">
          <h3>软件更新</h3>
          <button className="dialog-close" onClick={onClose} aria-label="关闭">
            <svg width="24" height="24" viewBox="0 0 16 16" fill="currentColor">
              <path d="M4.646 4.646a.5.5 0 01.708 0L8 7.293l2.646-2.647a.5.5 0 01.708.708L8.707 8l2.647 2.646a.5.5 0 01-.708.708L8 8.707l-2.646 2.647a.5.5 0 01-.708-.708L7.293 8 4.646 5.354a.5.5 0 010-.708z"/>
            </svg>
          </button>
        </div>
        <div className="dialog-body">
          {hasAnyUpdate ? (
            <div className="update-content">
              {hasClientUpdate && (
                <div className="update-section client-update">
                  <div className="update-badge">客户端更新</div>
                  <div className="update-title">DeepSeek Harness 客户端</div>
                  <div className="version-comparison">
                    <span className="version-current">{info.clientUpdate!.current_version}</span>
                    <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor" className="version-arrow-icon">
                      <path d="M10.293 3.293a1 1 0 011.414 0l6 6a1 1 0 010 1.414l-6 6a1 1 0 01-1.414-1.414L14.586 11H3a1 1 0 110-2h11.586l-4.293-4.293a1 1 0 010-1.414z"/>
                    </svg>
                    <span className="version-new">{info.clientUpdate!.latest_version}</span>
                  </div>
                  {info.clientUpdate!.release_notes && (
                    <div className="update-notes">
                      <div className="notes-label">更新内容</div>
                      <div className="notes-text">{info.clientUpdate!.release_notes}</div>
                    </div>
                  )}
                  {info.clientUpdate!.download_url && (
                    <>
                      <button
                        onClick={() => handleDownloadUpdate()}
                        className={`download-btn ${downloadState === "success" ? "download-complete" : ""}`}
                        disabled={downloadState === "downloading" || downloadState === "success"}
                        style={{ "--download-progress": `${downloadPercent}%` } as React.CSSProperties}
                      >
                        <span className="download-btn-progress" />
                        <span className="download-btn-label">
                          {downloadState === "downloading" ? `更新中 ${downloadPercent}%` : downloadState === "success" ? "已下载" : "更新"}
                        </span>
                      </button>
                      {downloadState === "error" && <div className="download-feedback error">{downloadError}</div>}
                    </>
                  )}
                </div>
              )}
              {hasKernelUpdate && (
                <div className="update-section kernel-update">
                  {hasClientUpdate && <div className="update-divider"></div>}
                  <div className="update-badge kernel">内核更新</div>
                  <div className="update-title">DeepSeek Harness 内核</div>
                  <div className="version-comparison">
                    <span className="version-current">{info.installed || "未知"}</span>
                    <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor" className="version-arrow-icon">
                      <path d="M10.293 3.293a1 1 0 011.414 0l6 6a1 1 0 010 1.414l-6 6a1 1 0 01-1.414-1.414L14.586 11H3a1 1 0 110-2h11.586l-4.293-4.293a1 1 0 010-1.414z"/>
                    </svg>
                    <span className="version-new">{info.latest}</span>
                  </div>
                  <button
                    onClick={handleApplyKernelUpdate}
                    className={`download-btn ${downloadState === "success" ? "download-complete" : ""}`}
                    disabled={downloadState === "downloading" || downloadState === "success"}
                    style={{ "--download-progress": `${downloadPercent}%` } as React.CSSProperties}
                  >
                    <span className="download-btn-progress" />
                    <span className="download-btn-label">
                      {downloadState === "downloading"
                        ? `${kernelStage ?? "更新中"} ${downloadPercent}%`
                        : downloadState === "success" ? "已更新" : "更新"}
                    </span>
                  </button>
                  {downloadState === "error" && <div className="download-feedback error">{downloadError}</div>}
                </div>
              )}
            </div>
          ) : (
            <div className="update-content">
              <div className="update-success-icon">✓</div>
              <div className="update-message success">已是最新版本</div>
              <div className="update-detail">
                客户端 {info.clientUpdate?.current_version || APP_VERSION} · 内核 {info.latest}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export default App;
