# DSH Desk

DeepSeek Harness 桌面客户端 —— 轻量、免装环境、安装即用、自动跟随官方升级。

## 特性

- **零前置依赖**：安装包仅 ~5MB，Node 运行时与官方内核首次启动自动下载（动态测速选最快镜像源）
- **桌面化体验**：原生窗口内嵌 DeepSeek Harness 界面，托盘常驻，关闭窗口最小化到托盘
- **官方内核自动升级**：后台每 6 小时检测官方 `@deepseek-ai/dsh` 新版本，发现新版弹原生对话框，升级后自动重启服务
- **插件市场**：预装 dsh-market，插件市场出现在 dsh 自带设置里，可浏览/安装 GitHub 社区插件
- **看门狗**：dsh 服务意外退出自动重启
- **单实例**：重复启动恢复已有窗口

## 技术栈

- **壳**：Tauri 2 (Rust + WebView)
- **前端**：React + TypeScript
- **内核**：官方 `@deepseek-ai/dsh`（npm 包，原样运行）
- **升级引擎**：npm overlay（检测 → staging 安装 → 原子切换 → 失败回退）

## 开发

前置：Rust 工具链 + Node.js + pnpm（见 `TOOLCHAIN.md`）。

```bash
pnpm install
pnpm tauri dev      # 开发模式
pnpm tauri build    # 打包（macOS DMG / Windows NSIS）
```

## 数据目录

- macOS：`~/Library/Application Support/com.dshdesk.app/`
- 内含：`runtime/`（Node）、`agent/`（官方内核）、`dsh-home/`（profile 与插件）、`logs/`（日志）

## 构建产物

- macOS：`src-tauri/target/release/bundle/macos/DSH Desk.app` + `.dmg`
- Windows：需在 Windows / GitHub Actions 上构建

## 已知限制

- macOS 未签名，首次打开需右键 → 打开 → 仍要打开（公证需 Apple Developer 账号）
- 托盘左键恢复窗口在 macOS 上待修复（见 `TECHNICAL_DESIGN.md`）
