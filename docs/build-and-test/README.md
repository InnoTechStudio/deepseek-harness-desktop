# 编译与测试手册

本项目在 **macOS 主机 + Parallels Windows 虚拟机** 的组合下开发。这里记录的每条命令都在真机跑通过，
并标注了失败时的真实报错，方便对照。

> **给 AI 助手**：动手前请完整读一遍本目录。这里的坑不是"注意事项"级别的提醒，
> 是那种不知道就必然卡住、而且报错会指向错误方向的问题。

## 文档索引

| 文件 | 什么时候看 |
|---|---|
| [`01-environment.md`](01-environment.md) | 第一次接触本项目；`command not found` |
| [`02-build-macos.md`](02-build-macos.md) | 要出 macOS 包 |
| [`03-build-windows.md`](03-build-windows.md) | 要出 Windows 包 |
| [`04-test-macos.md`](04-test-macos.md) | 验证 macOS 上的功能是否真的生效 |
| [`05-test-windows.md`](05-test-windows.md) | 在虚拟机里验证 Windows 行为 |
| [`06-diagnose.md`](06-diagnose.md) | 收集证据、定位根因的具体手法 |
| [`07-pitfalls.md`](07-pitfalls.md) | **踩坑总表**，报错对不上时先查这里 |
| [`08-release.md`](08-release.md) | 改版本号、出正式包、归档 |

## 五分钟速查

```bash
# 0. 每个新终端都要先做这一步，否则没有 node
export PATH="$HOME/Library/Application Support/com.dshdesk.app/runtime/bin:$PATH"

# 1. 后端测试（最快的正确性信号）
cd src-tauri && cargo test --lib

# 2. 前端 + 桌面插件
pnpm build
cd dsh-desktop-shell && pnpm build && cd ..
cp dsh-desktop-shell/lib/* src-tauri/resources/dsh-desktop-shell/lib/

# 3. macOS 包
pnpm tauri build --bundles app dmg

# 4. Windows 包（在虚拟机里，见 03）
```

## 三条铁律

1. **虚拟机只用 `Windows 11-ARM`。** 另一台 `Windows 11-测试专用` 是用户手动测试用的，
   任何情况下不要启动、不要写入、不要卸载它上面的东西。

2. **改完代码要自己跑通再交付。** 包括：重新编译、装到 `/Applications`、启动、查日志确认行为。
   不要把命令丢给用户让他们自己验证。

3. **区分"已验证"和"未验证"。** 虚拟机有几个结构性限制（见 `05`），
   有些行为在里面测不了。测不了就说测不了，不要把单元测试通过说成实机通过。
