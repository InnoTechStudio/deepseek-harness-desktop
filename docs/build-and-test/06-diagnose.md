# 06 · 取证与定位根因

这一章讲方法：怎么拿到能下判断的证据，而不是靠猜。

## 总原则

**先能观察，再谈修。** 本项目踩过的坑里，有相当一部分是
"看着像 A 其实是 B"，靠读代码猜会改错方向。

反例（真实发生过）：
- 进度条卡住 → 以为是没做字节级进度 → 实际是 `output()` 阻塞导致回调压根没被调用
- 插件从市场消失 → 以为是显示逻辑过滤了它 → 实际是根本没装上（构建脚本被拒）
- 快捷方式指向错误路径 → 以为是安装器 bug → 实际是 SYSTEM 身份下的测量假象

## 变量隔离

同时怀疑两个原因时，一次只动一个。

实例：开机自启失败，怀疑「签名」和「LaunchAgent Label 冲突」。
做法是保持签名不变、只改 Label，写一个探针 plist 手动 `launchctl bootstrap`。
结果立刻登记成功 → 确定是 Label 冲突，与签名无关。

同理，验证通知是否需要付费签名：用同一份代码分别以 ad-hoc 和免费
Apple Development 签名各测一次，报错完全一致 → 结论"与签名无关"。

## 让代码把真相报出来

排障时如果现有日志不够，**先补日志/补返回值，再继续查**。

真实例子：`send_test_notification` 原来无论成败都返回 `Ok(true)`，
用户看不到通知也看不到原因，只能猜。改成返回真实结果后，
问题一次就定位到「授权状态是 Denied」。

```rust
// 改之前：失败被静默吞掉
crate::platform::notify(&app, kind, title, body);
Ok(true)

// 改之后：把系统给出的原因带回前端
crate::platform::notify_detailed(&app, kind, title, body)
```

## 直接调被测系统的函数

插件市场是 JS 写的，与其点界面猜，不如直接调它自己的判定函数——
省时间且结论确定。见 `04-test-macos.md` 的市场章节。

同样思路：市场的数据也可以通过 HTTP 接口取，不需要图形界面。

```bash
curl -s http://127.0.0.1:<port>/dsh-market/installed | python3 -m json.tool
```

## 用 ignored 测试跑真实链路

需要网络和真实内核的验证，写成 `#[ignore]` 测试，手工触发：

```rust
#[test]
#[ignore = "需要网络与内置 node，手工运行以核对真实进度序列"]
fn live_install_reports_monotonic_progress() { … }
```

```bash
cargo test --lib live_install_reports_monotonic_progress -- --ignored --nocapture
```

这条实测输出：`回调 955 次，首个 0.006，末个 0.980`——
直接证明流式读取生效了。改之前回调次数会是 0（安装期间一次都不调）。

## 抓外部进程的真实输出

想知道 pnpm 到底输出什么，别猜格式，跑一次数出来：

```bash
cd /tmp && mkdir -p probe && cd probe
echo '{"name":"probe","private":true}' > package.json
NODE="$HOME/Library/Application Support/com.dshdesk.app/runtime/bin/node"
PNPM="$HOME/Library/Application Support/com.dshdesk.app/runtime/lib/node_modules/pnpm/bin/pnpm.mjs"
"$NODE" "$PNPM" add "@deepseek-ai/***@***-rc.2" \
  --config.registry=https://repo.huaweicloud.com/repository/npm/ \
  --ignore-scripts --node-linker=hoisted --reporter=ndjson 2>&1 |
python3 -c "
import sys, json
names = {}
for line in sys.stdin:
    try: d = json.loads(line)
    except: continue
    n = d.get('name','?'); names[n] = names.get(n,0)+1
for k,v in sorted(names.items(), key=lambda x:-x[1]): print(f'{v:5d}  {k}')
"
```

这次实测拿到：`pnpm:progress` 1399 条（其中 `resolved` 504、`imported` 449）、
`pnpm:stage` 4 条。据此才能设计出真实的进度算法，而不是靠关键词猜阶段。

## 大输出落盘再筛

命令输出可能几百 KB，直接打印会淹没上下文。先重定向再过滤：

```bash
cmd > /tmp/out.log 2>&1
grep -E "pattern" /tmp/out.log | head -20
tail -30 /tmp/out.log
```

## 复现问题状态

修复前先把坏状态造出来，改完再验证同一状态能被自动纠正。

例：验证插件市场登记恢复功能。
先人为摘掉 `dependencies` 里的条目、保留 `node_modules` 目录和 `bundles` 条目
（这正是市场安装失败后的残留形态），然后启动应用，看日志是否出现
`恢复插件市场登记: dsh-file-upload`，并把 manifest 与备份逐字节比对：

```bash
cp profile/package.json /tmp/backup.json
# …人为破坏…
# …启动应用…
diff <(python3 -c "import json;print(json.dumps(json.load(open('/tmp/backup.json')),sort_keys=True,indent=2))") \
     <(python3 -c "import json;print(json.dumps(json.load(open('<profile>/package.json')),sort_keys=True,indent=2))") \
  && echo "与原始状态一致，恢复无副作用"
```

## 系统级观察点速查

| 要查什么 | 命令 |
|---|---|
| macOS 电源断言 | `pmset -g assertions` |
| macOS 登录项 | `launchctl print "gui/$(id -u)/<label>"` |
| macOS 签名 | `codesign -dvvv <app>` |
| macOS TCC 权限 | `sqlite3 -readonly ~/Library/Application\ Support/com.apple.TCC/TCC.db "select service,client,auth_value from access where client like '%dsh%';"` |
| macOS 通知注册表 | `~/Library/Group Containers/group.com.apple.usernoted/Library/Preferences/group.com.apple.usernoted.plist`（用 `plistlib` 读） |
| Windows 卸载项 | `reg query "HKCU\...\Uninstall\DeepSeek Harness"` |
| Windows exe 元数据 | `(Get-Item <exe>).VersionInfo` |
| Windows 进程会话 | `Get-CimInstance Win32_Process -Filter "Name='…'"` 看 `SessionId` |

## 自我怀疑清单

改完之后，问自己：

1. 这个现象我**观察到**了，还是**推断**的？
2. 报错指向的位置，和真实根因是同一处吗？
3. 我的测量方法本身会不会制造假象？（SYSTEM 身份、缓存、单实例）
4. 有没有可能是**我上一轮的改动**造成的？

第 4 条尤其重要。本项目里插件市场信息显示不全，
根因就是我自己写的登记恢复逻辑绕过了市场的安装流程，导致缺少来源证据。
