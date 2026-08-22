import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import "./App.css";

type KernelState = {
  nodeReady: boolean;
  kernelReady: boolean;
  installedVersion: string | null;
  running: boolean;
  port: number | null;
  platform: string;
};

type DownloadProgress = {
  kind: string;
  received: number;
  total: number | null;
  percent: number | null;
};

const STEPS = [
  { id: "prep", title: "准备运行环境", desc: "下载并配置 Node 运行时" },
  { id: "kernel", title: "安装 DeepSeek Harness 内核", desc: "自动下载官方最新版内核" },
  { id: "launch", title: "启动服务", desc: "启动本地 dsh 服务" },
];

function App() {
  const [, setState] = useState<KernelState | null>(null);
  const [booting, setBooting] = useState(true);
  const [step, setStep] = useState(0);
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [logLines, setLogLines] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [dshUrl, setDshUrl] = useState<string | null>(null);

  const refreshState = async () => {
    try {
      const s = await invoke<KernelState>("get_state");
      setState(s);
      if (s.kernelReady && s.running && s.port) {
        setDshUrl(`http://127.0.0.1:${s.port}/`);
      }
      return s;
    } catch (e) {
      setError(String(e));
      return null;
    }
  };

  useEffect(() => {
    refreshState();
    const unsubs: Promise<UnlistenFn>[] = [];
    unsubs.push(
      listen<DownloadProgress>("download-progress", (e) => {
        setProgress(e.payload);
      }),
    );
    unsubs.push(
      listen<string>("kernel-log", (e) => {
        setLogLines((prev) => [...prev.slice(-80), e.payload]);
      }),
    );
    return () => {
      unsubs.forEach((p) => p.then((u) => u()));
    };
  }, []);

  useEffect(() => {
    // 自动初始化：node → kernel → launch
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
          await invoke("bootstrap");
        }
        if (!s.kernelReady) {
          setStep(1);
          await invoke("install_kernel");
        }
        setStep(2);
        const port = await invoke<number>("start_dsh");
        setDshUrl(`http://127.0.0.1:${port}/`);
        setBooting(false);
        refreshState();
      } catch (e) {
        setError(String(e));
        setBooting(false);
      }
    };
    run();
  }, []);

  // 主界面：纯全屏内嵌 dsh Web UI（无外部状态栏/设置面板，设置走 dsh 自带）
  if (!booting && dshUrl) {
    return (
      <div className="main-view">
        <iframe src={dshUrl} title="DeepSeek Harness" className="main-iframe" />
      </div>
    );
  }

  // 引导页 / 错误页
  return (
    <div className="boot-view">
      <div className="boot-card">
        <h1>DSH Desk</h1>
        <p className="subtitle">DeepSeek Harness 桌面客户端</p>
        {booting ? (
          <>
            <ol className="steps">
              {STEPS.map((s, i) => (
                <li key={s.id} className={i === step ? "active" : i < step ? "done" : ""}>
                  <span className="step-title">{s.title}</span>
                  <span className="step-desc">{s.desc}</span>
                </li>
              ))}
            </ol>
            {progress && progress.percent != null && (
              <div className="progress">
                <div className="progress-track">
                  <div className="bar" style={{ width: `${progress.percent}%` }} />
                </div>
                <span className="pct">{progress.percent.toFixed(0)}%</span>
              </div>
            )}
            {progress && progress.percent == null && <div className="hint">正在准备…</div>}
            <div className="log small">{logLines.slice(-5).join("\n")}</div>
          </>
        ) : error ? (
          <div className="error-box">
            <div className="error">{error}</div>
            <button onClick={() => location.reload()}>重试</button>
            <button onClick={() => invoke("open_data_dir")}>查看数据目录</button>
          </div>
        ) : (
          <div>
            <p>初始化完成，正在打开…</p>
          </div>
        )}
      </div>
    </div>
  );
}

export default App;
