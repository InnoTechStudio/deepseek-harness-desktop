# DeepSeek Harness 桌面客户端 —— 可行性研究与方案（v2）

> 更新：2026-08-21　｜　状态：研究完成，方案已确认（自研 Tauri 壳 + 官方 npm overlay 升级）

## 1. 需求回顾（v2 更新）

上一版调研发现：官方 `deepseek-ai/deepseek-harness`（179k★）只发布源码/npm，社区已有 `anywhere-labs/deepseek-harness-desktop`（DSH Desktop，Electron）完整覆盖三条需求，但**安装包太大**（macOS 279MB / Windows 209MB）。

本次新增需求：
- **体积要小** → 转向 **Tauri 2 壳**（不用 Electron，安装包可到 5–10MB 级别）
- **深度调研更多成熟项目**，找可二次开发的基底
- **关键：官方 DeepSeek Harness 一旦有新版本，底层 dsh 要能自动跟进升级**，而不是只升级壳

## 2. 深度调研结果：社区成熟项目全景

| 项目 | 技术栈 | 安装包大小 | 官方 dsh 自动更新 | 说明 |
|---|---|---|---|---|
| anywhere-labs/deepseek-harness-desktop | Electron | 209–279MB | ❌ 钉死版本，随壳升级 | 功能最全但太大 |
| **zouyuxuan122/Deepseek-Harness-EAC** | Electron | ~240MB | ✅ **npm overlay 官方更新** | Windows/Linux，官方 agent 自动检测 `@deepseek-ai/dsh` 新版，用户同意后装到 overlay，失败可回退 |
| **myYangyunfan/dsh_desktop** | **Tauri 2** | 68–197MB | 手动 | v0.5.0 起 Tauri，包内置完整官方 dsh |
| **hairyf/deepseek-harness-desktop** | **Tauri 2** | **4.7–7MB** | ✅ **每次启动同步上游最新内核** | 体积最小；但内核走自建 `deepseek-harness-pkg` 分发源 + **非商用条款** |
| dataelement/dsh-desktop | Electron/Tauri 混 | 146–191MB | 自动（壳） | 已公证 macOS，钉死 rc.1 |
| lencx/Minke | Tauri | 145–180MB | — | 通用，体积反而大 |
| ningbainb/deepseek-harness-desktop | Electron | 155–218MB | 自动（壳） | Windows only |

**关键发现（决定路线）：**
1. **Tauri 2 才是体积出路**：hairyf 做到了 4.7–7MB 安装包（因为内核在首次启动时才下载 ~40MB，不用打进包）。
2. **官方 dsh 有现成的 npm 包**：`@deepseek-ai/dsh`（dist-tag `latest` = `0.1.1-rc.2`），内部带 Cordis 全家桶。**EAC 的升级引擎（updater.js）已经实现了"自动检测 npm 新版本 → 用户同意 → 安装到数据目录 overlay → 原子切换 → 失败回退"这条你想要的链路**。
3. 没有现成项目同时满足「Tauri 小体积 + 官方 npm 自动更新 + 全平台」，这正是我们要做的缝合点。

## 3. 方案（已确认）

**自研轻量 Tauri 2 壳 + 内置官方 dsh 运行时 + EAC 式 npm overlay 自动更新引擎**

架构（参考 EAC + hairyf 的成熟做法）：

```
Tauri 2 (Rust) WebView 壳          ← 安装包本体，最小
 ├─ 首次启动：下载 Node runtime + @deepseek-ai/dsh 到数据目录
 ├─ 启动 dsh web --host 127.0.0.1 --port 0（官方内核，原样运行）
 ├─ 轮询端口 HTTP 200 → 加载 Web UI 到原生窗口
 ├─ 升级引擎（借鉴 EAC updater.js）：
 │    自动检测 @deepseek-ai/dsh 新版本（npm registry）
 │    用户同意 → 装到数据目录 overlay → 原子切换 → 失败回退内置版
 ├─ 托盘 + 后台常驻 + 桌面通知
 └─ 壳自身自更新（GitHub Releases 自动更新，Tauri 内置能力）
```

**对应三条需求：**
1. **环境依赖**：内置 Node runtime（hairyf 用 Node v22.22.0）打进包，用户免装；启动时只需联网下载官方内核（~40MB），此后全离线。
2. **桌面化**：Tauri 原生窗口 + 托盘 + 常驻，不是网页。
3. **自动升级**：两层升级——**官方 dsh 内核**走 npm overlay（这正是你要求的"基于官方 DeepSeek Harness 自动检测升级"），**壳自身**走 Tauri 自更新。

**为什么能"基于官方项目自动检测"：** 官方 dsh 的发布渠道就是 npm `@deepseek-ai/dsh`，EAC 已实现检测该包新版本并 overlay 升级。我们照此实现，即"检测官方项目 → 升级官方底层"，无需依赖任何第三方分发源。

## 4. 目标平台：macOS + Windows

- 本机 Mac arm64 → 先在 macOS 上把整套流程跑通、产出可分发 DMG
- Windows x64 → 后续在 GitHub Actions / Windows 上构建 NSIS/portable 安装包
- Linux 留作后续可选

## 5. 待办 / 风险

- **macOS 公证**：Tauri 壳未公证需 Gatekeeper 放行一次；如要免右键打开需 Apple Developer 账号做 notarize（成本 $99/年，可选）。
- **Rust 工具链**：本机需装 rustup/cargo 才能编译 Tauri；Node 构建产物可在 CI 完成。
- **上游仍处预览期**：`@deepseek-ai/dsh` 在快速迭代、承诺破坏性变更；overlay 机制 + 失败回退正是为对抗这种不稳定性而设计。

## 参考

- 官方 dsh：https://github.com/deepseek-ai/deepseek-harness（npm `@deepseek-ai/dsh`）
- EAC（npm overlay 升级引擎参考）：https://github.com/zouyuxuan122/Deepseek-Harness-EAC
- hairyf Tauri 壳（体积/自更新参考）：https://github.com/hairyf/deepseek-harness-desktop
- myYangyunfan Tauri 版（内置官方 dsh 参考）：https://github.com/myYangyunfan/dsh_desktop
- Tauri 2：https://tauri.app
