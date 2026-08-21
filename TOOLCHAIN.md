# 本机工具链（2026-08-22 配置完成）

| 工具 | 版本 | 位置 / 说明 |
|---|---|---|
| rustc | 1.98.0 | ~/.cargo（rustup minimal，经 rsproxy 镜像安装） |
| cargo | 1.98.0 | ~/.cargo（crates.io 走 rsproxy-sparse 镜像，~/.cargo/config.toml） |
| Node | v22.22.0 | ~/.local/share/dshdesk/node-v22.22.0-darwin-arm64（npmmirror 下载，自带 npm 10.9.4 / corepack） |
| pnpm | 11.22.0 | 全局装在 node 目录 bin 下（npm install -g pnpm，经 npmmirror） |
| Tauri CLI | 2.x | 项目 devDependency（@tauri-apps/cli） |

## 使用说明
- 每次新 shell 需要：`export PATH="$HOME/.local/share/dshdesk/node-v22.22.0-darwin-arm64/bin:$PATH"`
  `export PATH="$HOME/.cargo/bin:$PATH"`
- cargo 构建已通过 ~/.cargo/config.toml 走 rsproxy 镜像（中国网络快）
- Node/npm/pnpm 安装均走 npmmirror 镜像

## 首次构建结果（M1 最小壳已跑通）
- 产物：`src-tauri/target/release/bundle/macos/dshdesk.app`（2.8MB DMG 版）
- 冒烟测试：应用能启动并创建窗口（pid 存活 + System Events 计数=1）
- 构建耗时：首次 ~2min（全量下载依赖），此后增量秒级
