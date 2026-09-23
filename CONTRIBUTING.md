# DeepSeek Harness Desktop

Tauri 2 桌面壳 + 官方 DSH npm 内核（overlay 自动更新），目标平台 macOS (aarch64) 与 Windows (x64)。

## 给维护者的必读须知

**动手编译或测试之前，先读 [`docs/build-and-test/README.md`](docs/build-and-test/README.md)。**

那份文档记录的是本项目实测跑通的方法，以及一批**不看文档必然踩中**的坑。
里面每条命令都在真机上验证过，不是推测。典型例子：

- 机器上**没有系统 node**，不显式加内置 runtime 的 PATH，`pnpm` 一定报 command not found
- Windows 虚拟机里的 PowerShell 脚本**含中文就必须写 UTF-8 BOM**，否则解析失败且报错完全指向别处
- `prlctl exec` 的 `cmd.exe` 和 `/c` **必须拆成两个参数**传，合并写会静默失败
- Windows 上 `tauri build --bundles nsis` 打包阶段会失败，需要**直接调 makensis** 绕过
- 虚拟机里以 `prlctl` 启动应用会落在 SYSTEM 会话，**数据目录和你以为的不是同一个**

按文档做，一次通过；凭直觉做，会在这些地方反复卡住。

## 其他文档

| 文件 | 内容 |
|---|---|
| [`docs/build-and-test/`](docs/build-and-test/) | 编译、测试、排障（**先读这个**） |
| `TECHNICAL_DESIGN.md` | 整体架构与技术选型 |
| `RESEARCH.md` | 早期方案调研 |
| `APP/CHANGELOG.md` | 面向用户的更新日志 |
| `APP/macOS/`、`APP/Windows/` | 已归档的正式安装包与校验清单 |

## 快速上手

```bash
export PATH="$HOME/Library/Application Support/com.dshdesk.app/runtime/bin:$PATH"
pnpm build                       # 前端
cd src-tauri && cargo test --lib # 后端测试
```

完整流程见 `docs/build-and-test/README.md`。
