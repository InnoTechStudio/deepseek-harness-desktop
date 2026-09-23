# 01 · 环境

## 最大的坑：机器上没有系统 node

这台 macOS 上**没有全局安装 node / npm / pnpm**。验证过：

```bash
$ env -i /bin/zsh -lc 'which node pnpm'
node not found
pnpm not found
```

项目用的是**应用自己下载的内置 runtime**，位置固定：

```
~/Library/Application Support/com.dshdesk.app/runtime/
├── bin/node        ← v22.22.0
├── bin/pnpm        ← 11.25.0
├── bin/npm
└── lib/node_modules/pnpm/bin/pnpm.mjs   ← 绕开 shebang 时直接用 node 跑这个
```

所以**每个新 shell 都必须先前置 PATH**：

```bash
export PATH="$HOME/Library/Application Support/com.dshdesk.app/runtime/bin:$PATH"
```

不做这一步的症状：`(eval):1: command not found: pnpm`。
很容易误判成"环境坏了"去装 Homebrew node——不要那样做，
装了反而会和内核用的 pnpm store 打架。

### 为什么有时要直接用 node 跑 pnpm.mjs

`bin/pnpm` 的 shebang 是 `#!/usr/bin/env node`。如果 PATH 里没有 node（比如在
子进程、或 Windows 上），`env node` 找不到解释器，pnpm 直接退出。稳妥写法：

```bash
NODE="$HOME/Library/Application Support/com.dshdesk.app/runtime/bin/node"
PNPM="$HOME/Library/Application Support/com.dshdesk.app/runtime/lib/node_modules/pnpm/bin/pnpm.mjs"
"$NODE" "$PNPM" install
```

Rust 侧的 `kernel.rs` 就是这么做的，原因同上。

## 应用的数据目录

```
~/Library/Application Support/com.dshdesk.app/
├── settings.json          ← 开关状态（auto_launch / prevent_sleep / desktop_notifications …）
├── logs/app.log           ← 主日志，排障第一站
├── logs/tray.log
├── runtime/               ← 内置 node
├── agent/                 ← 当前生效的 dsh 内核（overlay）
├── agent-previous/        ← 上一版，供回退
├── agent-staging/         ← 安装中的临时目录，失败零影响
└── dsh-home/profiles/web/ ← DSH profile
    ├── package.json           ← dependencies + dsh.profile.bundles
    ├── pnpm-workspace.yaml    ← allowBuilds 构建脚本白名单
    ├── cordis.yml / cordis.patch.yml
    ├── node_modules/          ← 插件实际安装位置
    └── .dsh-market/           ← 插件市场自己的状态与日志
```

Windows 对应位置是 `%APPDATA%\com.dshdesk.app\`（即
`C:\Users\<用户>\AppData\Roaming\com.dshdesk.app\`）。

## Rust 工具链

```bash
cd src-tauri
cargo --version          # 走 rsproxy 镜像，见 ~/.cargo/config.toml
cargo test --lib         # 应为 70 passed（数量会随开发增长）
```

macOS 上直接编译原生 aarch64；Windows x64 **不在 macOS 上交叉编译**，
在虚拟机里原生编译（见 `03`）。曾尝试 `cross` + Docker，
链接 WebView2 时问题很多，不如虚拟机可靠。
