# 05 · Windows 虚拟机测试

先读 `03-build-windows.md` 里的 `prlctl exec` 写法和共享路径约定。

## 最大的坑：PowerShell 脚本含中文必须写 UTF-8 BOM

这个我逐项隔离验证过，结果很明确：

| 脚本内容 | 无 BOM | 带 BOM |
|---|---|---|
| 纯 ASCII | 正常 | 正常 |
| 中文**只在注释里** | 正常 | 正常 |
| 中文在**字符串里**（单引号或双引号都一样） | **解析失败** | 正常 |

失败时的报错是：

```
字符串缺少终止符: "。
+ FullyQualifiedErrorId : TerminatorExpectedAtEndOfString
```

原因：虚拟机上的 PowerShell 5.1 对无 BOM 文件默认按 GBK 解码，
中文字节被拆错，引号配对失效。**报错完全指向"引号写错了"，
会让你反复去改转义，方向全错。**

正确做法——用 printf 写 BOM：

```bash
printf '\xef\xbb\xbf' > script.ps1
cat >> script.ps1 <<'PS'
$msg = "中文内容没问题了"
Write-Output ("RESULT len=" + $msg.Length)
PS
```

实测带 BOM 后中文长度正确（`len=5`），字符没被破坏。

**更省心的办法：脚本里只用 ASCII**，中文留在 macOS 侧的输出里。
本文档里的示例脚本都遵循这个约定。

顺带验证：`ConvertFrom-Json` 本身是可靠的（读出 `version=0.4.3`），
之前怀疑它有编码问题是误判，根因就是脚本文件编码。所以不必写正则兜底。

## 脚本投递方式

`prlctl exec` 不方便传多行脚本，走共享目录中转：

```bash
# 1) 写到项目根目录（共享可见）
cat > vm-task.ps1 <<'PS'
Write-Output 'HELLO'
PS

# 2) 拷进虚拟机
prlctl exec "Windows 11-ARM" cmd.exe /c "copy /Y \\\\Mac\\project\\vm-task.ps1 C:\Users\<user>\vm-task.ps1"

# 3) 执行
prlctl exec "Windows 11-ARM" cmd.exe /c "powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\<user>\vm-task.ps1"

# 4) 用完删掉，别留在仓库里
rm -f vm-task.ps1
```

**共享目录有缓存**：同名文件改内容后重拷，虚拟机可能读到旧版本。
症状是"我改了脚本但行为没变"。换个文件名最省事。

**脚本必须写在项目根目录**。我曾在 `/Users/<user>/Documents/AI-Agent`
（上一级）写脚本，共享根是项目目录，那里的文件虚拟机看不见，
表现为 `copy` 报"找不到文件"。

## 三个结构性限制（很重要）

### 1. prlctl 以 SYSTEM 身份运行

```bash
$ prlctl exec "Windows 11-ARM" cmd.exe /c "whoami"
nt authority\system
```

后果：

- 你启动的应用**数据目录不是用户的**，会落在
  `C:\Windows\System32\config\systemprofile\AppData\...`
- pnpm 会去 systemprofile 找 store，报
  `ERR_PNPM_UNEXPECTED_STORE`。绕法是显式指定：
  ```
  $env:npm_config_store_dir = 'C:\Users\<user>\AppData\Local\pnpm\store\v11'
  ```
  注意**只改 `LOCALAPPDATA` 无效**，pnpm 读的是 npm 风格的配置项名。
- 读快捷方式时 `Shell.Application` 会按**调用者**身份展开 `%LOCALAPPDATA%`，
  于是解析出 systemprofile 路径，**看起来像 bug 其实是测量假象**。
  要判断 `.lnk` 真实指向，读原始字节：
  ```powershell
  $b=[System.IO.File]::ReadAllBytes($lnk)
  $s=[System.Text.Encoding]::Unicode.GetString($b) -replace "`0",''
  [regex]::Matches($s,'[A-Za-z]:\\[^\x00-\x1f"<>|]{4,200}') | ForEach-Object { $_.Value }
  ```

### 2. 单实例插件会让启动"看起来失败"

应用装了 `tauri-plugin-single-instance`。如果交互会话里已经有一个实例在跑，
你新启动的进程会把控制权交给它然后**自己退出**。
症状：`Start-Process` 返回了 pid，几十秒后进程消失，数据目录没变化。

对策：先彻底杀干净再测。

```powershell
taskkill /F /IM "DeepSeek Harness.exe" /T 2>&1 | Out-Null
taskkill /F /IM node.exe /T 2>&1 | Out-Null
Start-Sleep 5
```

`Stop-Process` 对带空格的进程名匹配不稳，用 `taskkill` 更可靠。

### 3. 图形界面交互测不了

`prlctl` 只能进 session 0。我试过用计划任务（`schtasks /RU <user> /IT`、
COM `Schedule.Service`、批处理包一层）把进程投进交互会话，**都没成功**。

所以这些**在虚拟机里验证不了**，必须交给用户在真机确认：
剪贴板快捷键、文件拖入、DSH 设置面板交互、插件市场界面点击。

可以验证的：单元测试、安装/卸载、注册表、文件落位、进程存活、
内核 CLI 行为、HTTP 接口返回值。

**不要把"单元测试通过"说成"实机验证通过"。**

## 全新环境测试

用户建议过"清空所有数据再重装测试，避免误测"，这是对的。

```powershell
taskkill /F /IM "DeepSeek Harness.exe" /T 2>&1 | Out-Null
taskkill /F /IM node.exe /T 2>&1 | Out-Null
Start-Sleep 3
$home_ = 'C:\Users\<user>'
foreach ($rel in @(
  'AppData\Roaming\com.dshdesk.app',
  'AppData\Local\com.dshdesk.app',
  'AppData\Local\DeepSeek Harness',
  'AppData\Local\pnpm'
)) {
  $d = Join-Path $home_ $rel
  if (Test-Path -LiteralPath $d) { Remove-Item -LiteralPath $d -Recurse -Force -ErrorAction SilentlyContinue }
  Write-Output ('CLEARED=' + (-not (Test-Path -LiteralPath $d)) + '  ' + $rel)
}
```

`AppData\Local\DeepSeek Harness` 是安装目录，删之前先跑 `uninstall.exe /S`，
否则"应用和功能"里会留死项。

**注意**：删数据目录时如果应用还在跑，它不会重建目录，
你会看到"启动了但数据目录不存在"的怪现象。顺序必须是先杀进程再删。

## 静默安装

```powershell
$setup = 'C:\Users\<user>\dsh-build\src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis\dsh-1.0.1-x64-setup.exe'
Start-Process -FilePath $setup -ArgumentList '/S' -Wait
Start-Sleep 10
Get-ChildItem -LiteralPath 'C:\Users\<user>\AppData\Local\DeepSeek Harness' -File |
  Select-Object -ExpandProperty Name
```

装完应该**只有** `DeepSeek Harness.exe` 和 `uninstall.exe`。
若还有 `dshdesk.exe`，说明安装钩子没生效（见 `07`）。

## 版本与命名核对

```powershell
(Get-Item 'C:\Users\<user>\AppData\Local\DeepSeek Harness\DeepSeek Harness.exe').VersionInfo |
  Select-Object FileVersion,ProductName,FileDescription | Format-List
reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\DeepSeek Harness" 2>nul
```

三处都应是 `DeepSeek Harness` / `1.0.1`，不能出现 `dshdesk`。

## 开机自启（Windows 侧）

写 HKCU Run 项，通过 `reg.exe` 操作，免管理员权限：

```powershell
reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v DeepSeekHarness
```

## 插件安装（绕开图形界面）

```powershell
$home_ = 'C:\Users\<user>'
$root  = Join-Path $home_ 'AppData\Roaming\com.dshdesk.app'
$node  = Join-Path $root 'runtime\node.exe'
$entry = Join-Path $root 'agent\node_modules\@deepseek-ai\dsh\lib\bin.js'
$env:npm_config_store_dir = Join-Path $home_ 'AppData\Local\pnpm\store\v11'
$env:LOCALAPPDATA = Join-Path $home_ 'AppData\Local'
$env:APPDATA      = Join-Path $home_ 'AppData\Roaming'
$env:USERPROFILE  = $home_
$env:DSH_HOME     = Join-Path $root 'dsh-home'
$env:PATH         = (Join-Path $root 'runtime') + ';' + $env:PATH
& $node $entry plugin --profile web add dsh-file-upload --registry=https://repo.huaweicloud.com/repository/npm/
Write-Output ('EXIT=' + $LASTEXITCODE)
```

`EXIT=0` 且看到 `sharp install: Done` 才算成功。
市场界面内部走的也是这条 CLI。

## 收尾清理

```bash
rm -f vm-*.ps1 vm-*.cmd     # 宿主侧
prlctl exec "Windows 11-ARM" cmd.exe /c "del /Q C:\Users\<user>\vm-*.ps1 C:\Users\<user>\*.log 2>nul & echo CLEANED"
```

自己造成的污染（SYSTEM 身份下的安装、systemprofile 里的数据目录）也要一并清掉。
