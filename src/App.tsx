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

type UpdateInfo = {
  installed: string | null;
  latest: string | null;
  updateAvailable: boolean;
  registry: string;
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
  const [state, setState] = useState<KernelState | null>(null);
  const [booting, setBooting] = useState(true);
  const [initBusy, setInitBusy] = useState(false);
  const [step, setStep] = useState(0);
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [logLines, setLogLines] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [dshUrl, setDshUrl] = useState<string | null>(null);
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [updating, setUpdating] = useState(false);

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
    unsubs.push(
      listen("menu-check-update", () => {
        setShowSettings(true);
        checkUpdate();
      }),
    );
    return () => {
      unsubs.forEach((p) => p.then((u) => u()));
    };
  }, []);

  const checkUpdate = async () => {
    try {
      const u = await invoke<UpdateInfo>("check_update");
      setUpdateInfo(u);
    } catch (e) {
      setError(String(e));
    }
  };

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
      setInitBusy(true);
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
      } finally {
        setInitBusy(false);
      }
    };
    run();
  }, []);

  const doUpdate = async () => {
    setUpdating(true);
    setError(null);
    try {
      await invoke<string>("apply_update");
      setUpdateInfo(null);
      await refreshState();
      const port = await invoke<number>("start_dsh");
      setDshUrl(`http://127.0.0.1:${port}/`);
    } catch (e) {
      setError(String(e));
    } finally {
      setUpdating(false);
    }
  };

  const doRollback = async () => {
    setError(null);
    try {
      await invoke("rollback");
      await refreshState();
      const port = await invoke<number>("start_dsh");
      setDshUrl(`http://127.0.0.1:${port}/`);
    } catch (e) {
      setError(String(e));
    }
  };

  const doSkip = async () => {
    if (updateInfo?.latest) {
      await invoke("skip_version", { version: updateInfo.latest });
      setUpdateInfo(null);
    }
  };

  // 主界面：直接内嵌 dsh Web UI
  if (!booting && dshUrl) {
    return (
      <div className="main-view">
        <iframe src={dshUrl} title="DeepSeek Harness" className="main-iframe" />
        <div className="statusbar">
          <span>内核 v{state?.installedVersion ?? "?"}</span>
          <span className="status-actions">
            <button onClick={() => setDshUrl(`${dshUrl}settings/`)}>插件市场</button>
            <button onClick={() => setShowSettings((v) => !v)}>设置</button>
          </span>
        </div>
        {showSettings && (
          <div className="settings-panel">
            <h3>设置</h3>
            <div className="row">
              <button onClick={checkUpdate} disabled={updating}>
                检查更新
              </button>
              {updateInfo?.updateAvailable && (
                <>
                  <span>
                    发现新版 v{updateInfo.latest}（当前 v{updateInfo.installed}）
                  </span>
                  <button onClick={doUpdate} disabled={updating}>
                    立即更新
                  </button>
                  <button onClick={doSkip}>跳过此版本</button>
                </>
              )}
            </div>
            <div className="row">
              <button onClick={doRollback}>回退到上一版本</button>
              <button onClick={() => invoke("open_data_dir")}>打开数据目录</button>
            </div>
            {error && <div className="error">{error}</div>}
            <div className="log">{logLines.join("\n")}</div>
          </div>
        )}
      </div>
    );
  }

  // 引导页 / 错误页
  return (
    <div className="boot-view">
      <div className="boot-card">
        <h1>DSH Desk</h1>
        <p className="subtitle">DeepSeek Harness 桌面客户端</p>
        {booting || initBusy ? (
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
                <div className="bar" style={{ width: `${progress.percent}%` }} />
                <span>{progress.percent.toFixed(0)}%</span>
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
