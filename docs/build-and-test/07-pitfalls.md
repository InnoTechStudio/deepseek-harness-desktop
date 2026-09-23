# 07 · 踩坑总表

按"症状 → 真实原因 → 处理"组织。报错和根因对不上时先查这里。

---

## 环境类

### `command not found: pnpm`

机器上没有系统 node。用内置 runtime：

```bash
export PATH="$HOME/Library/Application Support/com.dshdesk.app/runtime/bin:$PATH"
```

不要装 Homebrew node——会和内核用的 pnpm store 打架。详见 `01`。

### 界面改动没进包，也没有报错

`pnpm build` 只更新 `dsh-desktop-shell/lib/`，打包读的是
`src-tauri/resources/dsh-desktop-shell/`。必须手动同步：

```bash
cp dsh-desktop-shell/lib/* src-tauri/resources/dsh-desktop-shell/lib/
```

---

## Windows 虚拟机类

### PowerShell 报"字符串缺少终止符"，但引号明明是配对的

脚本文件含中文且**没有 UTF-8 BOM**。PowerShell 5.1 按 GBK 解码，
中文字节被拆错。加 BOM：

```bash
printf '\xef\xbb\xbf' > script.ps1
cat >> script.ps1 <<'PS'
…
PS
```

隔离验证结论：中文只在**注释**里不会触发，出现在**字符串**里才会。
最省心是脚本只用 ASCII。

### `prlctl exec` 没有任何输出也不报错

`cmd.exe` 和 `/c` 合并成一个参数了。必须拆开：

```bash
prlctl exec "Windows 11-ARM" cmd.exe /c "echo OK"   # ✅
prlctl exec "Windows 11-ARM" "cmd.exe /c echo OK"   # ❌ 静默失败
```

### `copy` 报找不到文件，但文件确实存在

脚本写在了项目目录之外。共享根是项目目录，上层目录虚拟机看不见。
必须写在 `DeepSeek-Harness-Desktop/` 下面。

### 改了脚本但行为没变

共享目录缓存。换个文件名。

### `ERR_PNPM_UNEXPECTED_STORE`

prlctl 以 SYSTEM 运行，pnpm 去 systemprofile 找 store。显式指定：

```powershell
$env:npm_config_store_dir = 'C:\Users\<user>\AppData\Local\pnpm\store\v11'
```

只改 `LOCALAPPDATA` 无效，pnpm 读的是 npm 风格配置项名。

### 应用启动了但几十秒后消失，数据目录没变化

单实例插件把控制权交给了已在跑的实例。先 `taskkill /F /IM "DeepSeek Harness.exe" /T`
彻底杀干净。`Stop-Process` 对带空格进程名匹配不稳。

### 快捷方式指向 `systemprofile` 路径

**测量假象**，不是 bug。`Shell.Application` 按调用者（SYSTEM）身份展开
`%LOCALAPPDATA%`。读 `.lnk` 原始字节才能看到真实目标，见 `05`。

### 删了数据目录，应用启动后却不重建

删的时候应用还在跑。顺序必须是先杀进程再删目录。

---

## 打包类

### `failed to bundle app with makensis`

Tauri 调 makensis 的子进程失败，但 **exe 已经编译成功了**。
直接调 makensis 绕过：

```bat
cd /d …\release\nsis\x64
"C:\Program Files (x86)\NSIS\makensis.exe" installer.nsi
```

产物是 `nsis-output.exe`，改名即可。

### 升级后安装目录残留旧 `dshdesk.exe`

主程序改名成 `DeepSeek Harness.exe` 后，NSIS 只覆盖同名文件，
旧文件留在原地。用户可能点开上一版，而且系统权限弹窗会显示内部代号。

已在 `src-tauri/installer/hooks.nsi` 里删除，通过
`tauri.conf.json` → `bundle.windows.nsis.installerHooks` 挂载，
安装前/安装后/卸载前三处都删。

### 构建目录里有旧命名的 .app

`DSH Desk.app`、`dshdesk.app` 这类历史产物会被 LaunchServices 登记，
干扰通知归属和登录项。直接删掉。

### 改了版本号，但 exe 里的 FileVersion 还是旧的

同步源码到虚拟机用的 `copy /Y` **会保留源文件时间戳**，
所以虚拟机里的 `tauri.conf.json` 看起来"没变化"，cargo 判定 `build.rs`
无需重跑，嵌入 exe 的版本资源仍是上一版。

而注册表里的版本是**对的**——NSIS 脚本直接读配置文件，不看 build.rs。
于是出现"注册表 1.0.2、exe 属性 1.0.1"这种自相矛盾的状态。

改版本号后同步到虚拟机，必须刷新时间戳强制重新生成：

```powershell
(Get-Item 'src-tauri\tauri.conf.json').LastWriteTime = Get-Date
(Get-Item 'src-tauri\Cargo.toml').LastWriteTime = Get-Date
```

核对方法：

```powershell
(Get-Item '…\release\DeepSeek Harness.exe').VersionInfo.FileVersion
```

### 静默安装"成功"了，但装进去的还是旧文件

NSIS 覆盖被占用的 exe 时会**静默跳过，不报错**，`INSTALLED=True`
和文件列表都正常，只有版本号和时间戳能看出问题。

安装脚本必须先彻底结束进程：

```powershell
taskkill /F /IM "DeepSeek Harness.exe" /T 2>&1 | Out-Null
taskkill /F /IM node.exe /T 2>&1 | Out-Null
Start-Sleep 5
Write-Output ('RUNNING_AFTER_KILL=' + @(Get-CimInstance Win32_Process -Filter "Name='DeepSeek Harness.exe'").Count)
```

验证装进去的确实是刚编译的那个：

```powershell
(Get-FileHash <已安装exe>).Hash -eq (Get-FileHash <编译产物exe>).Hash
```

只看"安装成功"和文件存在**不够**，必须比对哈希或版本号。

---

## macOS 系统能力类

### 开机自启开关打开后无效

两个独立原因，都遇到过：

1. **LaunchAgent Label 与 bundle identifier 同名**。应用运行时系统会自动注册
   `application.com.dshdesk.app.<hash>`，同名作业被 launchd 以
   `Input/output error` 拒绝。用 `com.dshdesk.app.launcher`。
2. **从 DMG 直接运行**。登录项指向 `/Volumes/...`，卸载镜像即失效。
   代码已拦住并提示先拖进"应用程序"。

只看 plist 文件存在**不够**，必须确认 launchd 真的登记了：

```bash
launchctl print "gui/$(id -u)/com.dshdesk.app.launcher"
```

### 弹出"System Events 正在后台运行"

用了 System Events 的登录项 API（要通过 AppleScript 驱动那个进程）。
用户会看到与本应用无关的软件名和提示语，很困惑。
改用 LaunchAgent，**不要再引入 System Events**。

### 通知完全不出现，也没有报错

授权状态是 `Denied`。macOS 永久记住拒绝：`requestAuthorization` 立即失败、
不再弹窗，`tccutil reset` 无效（通知权限不在 TCC 库里）。

只能手动开：系统设置 → 通知 → DeepSeek Harness。
代码里有 `open_notification_settings` 命令直达那一页。

**与付费签名无关**——ad-hoc 和免费 Apple Development 签名实测报错一致。

### 语音输入按钮点了没反应（macOS）

语音输入来自社区 `dsh-file-upload` 插件，走浏览器的 Web Speech API
(`webkitSpeechRecognition`)，**不是内核自带功能**。内核 0.1.5 起官方已内置
`file-upload`，桌面端会停用这个社区插件（两边撞同一个 loader id，见下文
「内核 0.1.5 与社区 dsh-file-upload 撞 loader id」），按钮会随之消失。
若仍看到按钮，那是旧 profile 还没被摘掉。

WKWebView 里这个对象**存在但不可用**，所以 `typeof` 特征检测过不了关：

| 配置 | 实测结果 |
|---|---|
| 什么都不加 | `onerror` 立刻报 `not-allowed` |
| 加 `NSMicrophoneUsageDescription` + `NSSpeechRecognitionUsageDescription` + audio-input / speech-recognition entitlements | 不报错，但也不触发任何事件，静默挂起 |

用一个最小 WKWebView 探针（同样的 bundle 与签名）验证过，两种结果都能稳定复现。
TCC 库里 `kTCCServiceSpeechRecognition` 已是 2（已允许），
但 `kTCCServiceMicrophone` **从未出现记录**——WebKit 压根没去申请麦克风。

WebKit 官方 bug [239816](https://bugs.webkit.org/show_bug.cgi?id=239816) 记的是 iOS
上加 usage description 即可修好；macOS 的行为并不相同，加了照样不工作。

**结论**：Apple 没有在 WKWebView 里启用 Web Speech，只有 Safari 能用。
Info.plist 和 entitlements 是必要条件但不充分，已保留（去掉会退回到 `not-allowed`，
而且插件的 `onerror` 降级路径要用麦克风）。真要做，只能上原生
`SFSpeechRecognizer` 再桥接回 JS，属于插件作者的范围。

Windows 侧连按钮都不会出现：WebView2 基于开源 Chromium，
而语音识别是 Chrome 接的谷歌云服务，不在开源部分里。

### 通知有名字但没图标

走了老接口 `NSUserNotification`（notify_rust → mac-notification-sys），
它靠 `installNSBundleHook()` 偷换 bundle 身份，署名图标不受控。
改用 `UNUserNotificationCenter`，见 `src-tauri/src/notify_mac.rs`。

### 裸二进制调 UNUserNotificationCenter 崩溃

```
NSInternalInconsistencyException: bundleProxyForCurrentProcess is nil
```

没有 `.app` 身份时必然崩。`notify_mac.rs` 的 `has_bundle_identity()`
就是为挡这个而存在，开发模式下会退回插件通道。

---

## 内核与插件类

### 进度条长时间停住然后突然跳变

`Command::output()` 会等进程完整结束才返回，回调在结束后才被一次性回放。
必须 `spawn()` + 逐行读：

```rust
let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
// stderr 必须另起线程排空，否则管道缓冲区填满会把进程卡死
for line in BufReader::new(child.stdout.take().unwrap()).lines().map_while(Result::ok) { … }
```

真实进度来自 `--reporter=ndjson`：`pnpm:progress` 的 `resolved`/`imported`
计数，加上 `pnpm:stage` 的 `resolution_done`（此刻总数已知）。

### 插件安装总是失败：`ERR_PNPM_IGNORED_BUILDS`

pnpm 11 默认拒绝执行依赖的构建脚本（`sharp`、`tesseract.js` 这类）。
它会在 `pnpm-workspace.yaml` 写下待决占位行：

```yaml
allowBuilds:
  sharp: set this to true or false
```

**关键时序**：占位行是用户第一次装带原生依赖的插件时才写入的。
只在装内核时放开一次赶不上——那时占位行还不存在。
所以 `allow_plugin_build_scripts` 必须**每次启动内核前**都跑一遍
（见 `dsh.rs`），它是幂等的。

桌面端没有终端让用户自己跑 `pnpm approve-builds`，这条必须由程序兜住。

### 插件在市场"已安装"里消失，但功能还在

市场的"已安装"只读 profile 的 `dependencies`，`bundles` 不参与：

```js
for (const [name, spec] of Object.entries(manifest.dependencies ?? {})) {
    if (!INBOX_BUNDLES.has(name)) installed[name] = spec;
}
```

存在"半安装"状态：目录在 `node_modules`、名字在 `bundles`、但 `dependencies`
里没有。插件照常加载，市场却看不见它，也无法更新卸载。
制造者是市场自己——安装失败时它回滚 `dependencies`，已落盘的目录留在原处。

`restore_unregistered_profile_plugins`（`kernel.rs`）会在启动前补回登记。
必须排在 `prune_missing_profile_bundles` 之后，否则会为已消失的目录凭空造依赖。

### 插件在市场里没有简介/作者，搜索里不显示"已安装"

比上一条更深一层。市场客户端匹配目录条目的规则：

```js
if (depRepoIds(spec, repos).size === 0 && looseMatchCount(plugins, name) > 1
    && !repoHintMatches(plugin, repoHints[name] ?? [])) continue;
```

即：目录里同名条目 >1 时，必须有 repo 证据才肯匹配。
而 `readInstalledRepoEvidence` **只对 `link:`/`file:` 规格读本地 repository 字段**，
npm 规格的来源证据本该由市场安装时记进 lockfile。

`dsh-file-upload` 在目录里正好有两个作者的同名条目
（`HongMing-Huang` 有 npm 包、`a903067276-rgb` 仅 GitHub），
所以缺证据就匹配失败，卡片退化成只有名字和版本。

**教训**：手写 `dependencies` 条目能让插件出现在列表里、能查更新，
但拿不到目录信息。要让插件被市场完整管理，得走市场自己的安装路径。

### 本地桌面插件不该出现在市场里

`dsh-desktop-shell` 随客户端更新，不该被市场管理。做法是**只登记到
`bundles`、不写 `dependencies`**（`kernel.rs` 的 `LOCAL_SHELL_PACKAGE` 常量）。

`bundles` 必须保留：DSH 只为 bundles 里的名字加载 `cordis.patch.yml`，
删掉整个高级布局都不生效。

**已知缺口**：新版市场客户端把 `bundles` 也并进了目录判定
（`installedForCatalog`），所以本地插件可能在较新版本里露出来。尚未修。

### 插件市场安装全部失败：`Symlink path is the same as the target path`

曾经在 `dependencies` 里写了包在 `node_modules` 里的自身路径，
pnpm 直接退出。现在只写 `bundles`，不写自引用路径。

---

## 交叉编译

试过 `cross` + Docker 在 macOS 上编 Windows x64，链接 WebView2 时问题很多。
**结论：不值得，用虚拟机原生编译。** `Cross.toml` 是那次尝试的残留。

---

## 内核鉴权（0.1.2-rc.1 起）

新版内核给 Web 界面加了浏览器鉴权。这一个变化连锁引出六层问题，
**每层单独看都像"已经修好了"，实际界面仍然不可用**。按依赖顺序记下来：

### 1. 只取端口会丢掉令牌

内核打印的是 `http://127.0.0.1:<port>/?dsh_token=…`。
拿端口自己拼地址 → 401 `dsh web authentication required`。

`parse_endpoint_from_line` 现在整条 URL 带回。刻意**没有**提供"只取端口"的
公开变体，就是防止再犯；要端口用返回值的 `port` 字段。

### 2. 鉴权 cookie 是 `SameSite=Strict`，iframe 存不下

宿主是 `tauri://localhost`，内核是 `http://127.0.0.1:*`，两者属跨站，
WebView 会丢弃这个 cookie。**表现为时好时坏**，取决于是否残留旧 cookie。

解法是 `proxy.rs` 里的本地反向代理：凭证由 Rust 侧持有，
iframe 只与同源的代理端口打交道。

### 3. 自定义协议下 WebSocket 用不了

一度想用 `dsh://` 自定义协议绕开跨站。行不通：内核会把地址改写成
`ws:`，WebKit 在自定义协议下拒绝这个切换，报 `Failed to load plugins`。
**必须是真正的 HTTP 端口。**

### 4. `Upgrade`/`Connection` 属逐跳首部，转发时会被剥掉

剥掉后 WebSocket 握手退化成普通 GET，内核回 404，页面一片空白。
现在 `wants_upgrade()` 检出握手，走单独的 `upgrade()` 路径。

### 5. 握手响应之后紧跟的字节会被吞

逐字节读到 `\r\n\r\n` 就停，同一个 TCP 段里的数据帧就丢了。
现在成块读，`head_end` 之后的 `leftover` 必须在对拷前先补发。

### 6. 内核要求 Origin 与 Host **完全相等**

代理改写了 Host 却原样转发浏览器的 Origin → 403，界面显示「连接异常」。
`forward()` 和 `upgrade()` 两处都要把 Origin 改写成上游 origin。

`--trusted-host` 对此**无效**，实测过：

```
不带 Origin        → 101 Switching Protocols
带浏览器的 Origin  → 403 Forbidden
Origin 与 Host 相同 → 101 Switching Protocols
```

### 附带：reqwest 没启用 gzip feature

它不会自动解压。删掉 `content-encoding` 而不解压 → 乱码；
保留它但内容是明文 → 空白。**解法是请求时发 `Accept-Encoding: identity`**，
让内核直接返回明文。

### 附带：0.1.4 的 `details` 是会话级插槽；0.1.5 换成了 root 级 `rightbar`

内核 0.1.4 的右侧栏叫 `details`，`scope: 'session'`。无活动会话时渲染它，内核抛
`strict session slot 'details' rendered without a scope binding`，
而且是渲染期异常，**整棵 React 树挂掉，界面全白**。当时必须先判断有没有会话再挂载。

内核 0.1.5-rc.1 起官方右侧栏改成 `rightbar`，**root-scoped 的 single**。
`dsh-client-ui-sidebar-right` 注入它；会话级子插槽 `rightbar.session` 由 RightbarRoot 自己处理。
桌面壳如果还声明 / 渲染 `details`，右侧栏永远是空的；如果还用「没会话就不 renderSlot」当防护，
官方 ExpandButton 永远挂不上去。正确做法：始终 `renderSlot('rightbar', {})`，
没有真实会话时只把列宽收成 0。

### 教训：协议层全绿 ≠ 界面正常

前四次我都只验到状态码（首页 200、API 200、WebSocket 101），
每次都以为修好了，实际浏览器端仍然不工作。

**curl 和 reqwest 没有 SameSite 概念、不校验 Origin、对编码错配很宽容。**
必须验证到内容层（`首页开头: <!doctype html>`）并看真实截图。
定位第 6 层问题时，是往页面注入错误捕获、把报错 postMessage 回宿主才找到的。

---

## 健康探测必须接受 401

`is_http_ok` 早先只认 2xx/3xx。内核带鉴权后首页返回 401，于是被判成
"服务没起来"，连锁出一条完整的故障链：

```
升级完成 → 探测判失败 → 报「DSH 启动失败」
        → 用户重开软件 → 退回首次安装流程
        → 装完内核 → 同一个判断再次失败 → 循环
```

401 恰恰证明服务在监听并能处理请求。判定逻辑抽成了
`status_means_serving()` 并有回归测试，别再改回去。
注意 403 仍然算失败——那是同源校验问题，放行会掩盖代理配置错误。

---

## 插件市场旧版本会让内核完全起不来

`dshmarket` ≤1.38.0 从 `@deepseek-ai/dsh-settings` 具名导入
`installSettingsSection`，内核 0.1.2 删掉了这个导出。

**缺失的具名导入不是可降级的缺失服务**：`ctx.inject` 会安静降级，
但 ESM 具名导入解析不到就是模块求值期的 SyntaxError，cordis 判定整条
loader entry 失败，内核退出 1。

这会形成死锁：内核起不来 → 界面打不开 → 用户没有任何入口去更新市场。

`spawn_dsh` 每次启动前调 `market_breaks_kernel_boot()` 检查，
命中就自动升级；升级救不回来（离线、或装到的仍是旧版）就调
`disable_market()` 摘掉它，**宁可少一个插件也要让软件能打开**。

判定只看 `settings.js` 里是否真有这条 `import` 语句：1.39.0 修好之后
在注释里原样保留了这段文字说明历史，**裸的子串匹配会把已修复的版本
误判成坏版本**，反而把好端端的市场停用掉。有测试守着这一点。

市场版本不跟着内核走，所以 macOS 上是 1.39.0（正常）、Windows 上可能
还是 1.38.0（崩溃）——**同一份代码在两端表现完全不同，别急着归因于平台差异。**

---

## 内核 0.1.5 与社区 `dsh-file-upload` 撞 loader id

内核 0.1.5-rc.1 起，`@deepseek-ai/dsh-web-app` 的 patch 插入：

```yaml
- id: file-upload
  name: '@deepseek-ai/dsh-client-file-upload'
```

社区插件 `dsh-file-upload` 的 `cordis.patch.yml` 也插入 `id: file-upload`。
cordis 要求 loader id 全局唯一，两边一并存就是：

```
Error: dsh: plugin tree failed to load: duplicate loader entry id: file-upload
TypeError: duplicate loader entry id: file-upload
```

内核退出 1。界面把这次失败当成「内核坏了」去重装——**重装只换 agent 目录，
清不掉 profile 里的社区插件**，于是死循环：重装 → 启动失败 → 再重装。

Windows 上有时看起来「更新成功了」，其实插件树已经半死，会话列表/新建会话
都没了；磁盘上的 `dsh-home/sessions/` 其实还在，不是数据丢了。

官方包干的是会话文件上传（Blob / ReadableStream → 暂存凭证）；社区包还带
MarkItDown、OCR、语音输入，但桌面 WebView 里语音用不了，核心上传能力两边
重叠。`spawn_dsh` 每次启动前调 `file_upload_plugin_collides()`，命中就
`disable_community_file_upload()` 把 `dsh-file-upload` 从 dependencies 和
bundles 里摘掉（目录留着）。`restore_unregistered_profile_plugins` 在内核
已占槽时也不会把它写回去。

---

## 无法新建会话 / 历史列表空白：ILayout + 0.1.5 插槽契约

桌面壳禁用了官方 `ui-layout`，自己 `provide('layout', DesktopLayoutState)`。
缺任何一块，症状都是「界面还在、会话全空、点新建没反应」——磁盘上的
`dsh-home/sessions/` 其实还在，不是数据丢了。

### 1. 缺官方 ILayout 方法

早期 `DesktopLayoutState` 只有侧栏/详情几何，**没有**：

```
beginNavigation(): AbortSignal
selectPanel(panelId): void
openRightbar / closeRightbar / dispose
```

ui-workspace 的「新建会话」是：

```
openWorkspace → ctx.layout.beginNavigation()
             → sessions.create / sessions.open
             → ctx.layout.selectPanel(null)
```

缺方法就是 `TypeError: … is not a function`，被 `.catch` 吃成
`console.warn("new session failed:")`。

`beginNavigation` 用 AbortController 轮换；`selectPanel` **不能**在未知 id 上
抛错（官方会抛，桌面不行：侧栏 PanelRow 可能在 occupant 注入前进 panel）；
`openRightbar` / `closeRightbar` 映射到详情栏宽度。

这一层修完之后，**历史列表仍然可能是空的**——`watchNavigation` 自动打开
最近工作区并不调用 `beginNavigation`，所以缺 ILayout 解释不了「老会话也不显示」。

### 2. 0.1.5 把 `conversation` / `details` 改成了 `main` / `rightbar`

官方 `ui-layout` 的 root children 是 `sidebar` / **`main`（keyed）** /
**`rightbar`（single, root）** / `shell.overlay`。
`ui-conversation` 注入 `{ name: "main", key: "conversation" }`；
`dsh-client-ui-sidebar-right` 注入 `rightbar`。
再也没有人注入 `conversation` 或 `details`。

桌面壳如果还声明旧名字：中间栏空、没有 Composer → 拖图片变成 WKWebView
全屏导航；新建会话即使成功也没有对话面可以显示。

中间栏必须：

```
renderSlot("main", {}, { entryKey: panelInfo.activePanelId ?? "conversation" })
```

`entryKey` 是 `renderSlot` 的**第三个参数**。

### 3. 缺 `panelInfo` root hook → 会话树渲染期崩溃

官方 `ui-layout` 还调用（运行时方法，slots 的 `.d.ts` 里没有）：

```
ctx.slots.provideRoot({ hooks: { panelInfo: { getSnapshot, subscribe } } })
```

WorkspaceBrowser / SessionTree / FlatList / SearchResults 无条件
`usePanelInfo((info) => info.activePanelId !== null)`。
hook 不存在就是渲染期异常：侧栏「新建会话」铬还在，**历史列表整段空白**。

`getSnapshot` 必须返回**稳定引用**（官方返回 store 里冻结的 `panelInfo`
嵌套对象）。每次 `return { activePanelId }` 一个新对象会让
`useSyncExternalStore` 无限重渲染。`selectPanel` 改 id 时才换这个对象。

---

## 拖入图片变成全屏：WKWebView 把 drop 当导航

浏览器默认：`drop` 不 `preventDefault`，就把文档导航到那个文件。
WKWebView 同样如此，表现是窗口/iframe 变成一张全屏图。

官方拦截在 `ComposerAttachments`（会话输入栏挂载之后）。没有活动会话、
或社区 `dsh-file-upload` 被摘掉（它以前在 document 上拦过）时，没人
`preventDefault`，拖一张图就全屏了。

桌面壳在 `AdvancedFrame` 捕获阶段对带 `Files` 的 `dragover`/`drop`
`preventDefault`，**不** `stopPropagation`，这样输入栏自己读
`dataTransfer.files` 的逻辑还能跑。宿主 `App.tsx` 再拦一层，免得拖到
iframe 外面把整个窗口换掉。

---

## shell 插件改了要同步到 resources

Tauri 打包只认 `src-tauri/resources/dsh-desktop-shell/`，
改了 `dsh-desktop-shell/` 源目录不会自动带过去。

漏同步过一次：源码里的 `inject` 已经清理干净，打进包里的却还是旧清单，
装出来直接白屏，而看源码完全正常。

现在 `pnpm sync:shell` 负责这件事，`build:macos` / `build:windows`
都会先跑它。**手工 `pnpm tauri build` 时记得自己先跑一遍。**
