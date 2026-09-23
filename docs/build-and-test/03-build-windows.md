# 03 · 编译 Windows x64

Windows 包在 **Parallels 虚拟机里原生编译**，不在 macOS 上交叉编译。

## 虚拟机清单

```bash
$ prlctl list -a
{caeb5519-…}  running    Windows 11-ARM        ← 只用这台
{def02387-…}  suspended  Windows 11-测试专用   ← 禁止触碰，用户手动测试用
{34634b9c-…}  stopped    macOS-测试专用
```

启动：

```bash
prlctl start "Windows 11-ARM"
# 等它拿到 IP（约 30–60 秒）
prlctl list -f | grep ARM
```

## prlctl exec 的正确写法

**`cmd.exe` 和 `/c` 必须作为独立参数传**：

```bash
# ✅ 正确
prlctl exec "Windows 11-ARM" cmd.exe /c "echo OK"

# ❌ 静默失败，无任何输出也无报错
prlctl exec "Windows 11-ARM" "cmd.exe /c echo OK"
```

第二种写法我踩过，浪费了不少时间才发现是参数拆分问题，不是命令本身有错。

## 共享目录

虚拟机通过 UNC 路径访问 macOS 项目目录：

```
\\Mac\project\  →  /Users/<user>/Documents/AI-Agent/Projects/DeepSeek-Harness-Desktop
```

验证过的事实：
- UNC 路径可读写：`Test-Path '\\Mac\project\package.json'` → `True`
- **点文件也可见**：`.gitignore` 能读到（曾误以为不可见，实测可以）
- 映射盘 `Z:` 在 SYSTEM 会话里**不可用**，必须用 `\\Mac\project` 这种 UNC 形式

虚拟机里的构建目录是独立副本：`C:\Users\<user>\dsh-build`。

## 同步源码到虚拟机

用 robocopy 从虚拟机侧拉取（比逐文件 `prlctl exec` 快得多）：

```bash
cat > /tmp/sync-to-vm.sh <<'SH'
#!/bin/zsh
VM="Windows 11-ARM"
DST='C:\Users\<user>\dsh-build'
for d in src-tauri/src src dist src-tauri/resources src-tauri/installer dsh-desktop-shell/lib; do
  win=$(echo "$d" | tr '/' '\\')
  prlctl exec "$VM" cmd.exe /c "robocopy \\\\Mac\\project\\${win} ${DST}\\${win} /MIR /NFL /NDL /NJH /NJS /NP" >/dev/null 2>&1
done
for f in src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/tauri.conf.json package.json; do
  win=$(echo "$f" | tr '/' '\\')
  prlctl exec "$VM" cmd.exe /c "copy /Y \\\\Mac\\project\\${win} ${DST}\\${win}" >/dev/null 2>&1
done
echo SYNC_DONE
SH
chmod +x /tmp/sync-to-vm.sh && /tmp/sync-to-vm.sh
```

**不要同步 `target/` 和 `node_modules/`**：几 GB，而且平台不通用。

同步后确认一下代码真到位了（用 PowerShell 计数，`findstr` 对长行不可靠）：

```bash
prlctl exec "Windows 11-ARM" cmd.exe /c "powershell -NoProfile -Command \"Select-String -Path 'C:\Users\<user>\dsh-build\src-tauri\resources\dsh-desktop-shell\lib\client.js' -Pattern 'dshDesktopUpdateButton' -SimpleMatch | Measure-Object | Select-Object -ExpandProperty Count\""
```

## 构建脚本

写到项目根目录（这样能经共享投递进去）：

```bat
@echo off
cd /d C:\Users\<user>\dsh-build
call "C:\BuildTools\Common7\Tools\VsDevCmd.bat" -arch=amd64 -host_arch=arm64 >nul
set PATH=C:\Program Files (x86)\NSIS;C:\Users\<user>\.cargo\bin;C:\Program Files\LLVM\bin;C:\Program Files\nodejs;%PATH%
set CI=true
set CC_x86_64_pc_windows_msvc=C:\PROGRA~1\LLVM\bin\clang.exe
echo === TEST ===
cargo test --manifest-path src-tauri\Cargo.toml --lib --target x86_64-pc-windows-msvc 2>&1 | findstr /C:"test result" /C:"error["
echo === BUILD ===
npm.cmd exec -- tauri build --target x86_64-pc-windows-msvc --bundles nsis 2>&1 | findstr /C:"Built application" /C:"error"
echo === NSIS ===
cd /d C:\Users\<user>\dsh-build\src-tauri\target\x86_64-pc-windows-msvc\release\nsis\x64
"C:\Program Files (x86)\NSIS\makensis.exe" installer.nsi 2>&1 | findstr /C:"Output:"
copy /Y nsis-output.exe ..\..\bundle\nsis\dsh-1.0.1-x64-setup.exe
echo === DONE ===
```

注意 `-arch=amd64 -host_arch=arm64`：宿主是 ARM64，目标是 x64，两个都要指明。

执行（编译约 3–5 分钟，务必后台跑）：

```bash
prlctl exec "Windows 11-ARM" cmd.exe /c "copy /Y \\\\Mac\\project\\vm-build.cmd C:\Users\<user>\vm-build.cmd"
nohup prlctl exec "Windows 11-ARM" cmd.exe /c "C:\Users\<user>\vm-build.cmd > C:\Users\<user>\b.log 2>&1" >/dev/null 2>&1 &
sleep 110
prlctl exec "Windows 11-ARM" cmd.exe /c "cd /d C:\Users\<user> && more b.log"
```

## NSIS 打包必然失败，要手动绕

`tauri build --bundles nsis` 会走到最后一步然后报：

```
Running makensis to produce …DeepSeek Harness_1.0.1_x64-setup.exe
failed to bundle project: `Failed to bundle app with makensis`
```

**exe 其实已经编译成功了**（看 `Built application at:` 那行），
只是 Tauri 调 makensis 的子进程失败。绕法是直接调 makensis：

```bat
cd /d ...\release\nsis\x64
"C:\Program Files (x86)\NSIS\makensis.exe" installer.nsi
```

成功输出 `Output: "…\nsis-output.exe"`，那就是安装包，改名即可用。
上面的构建脚本已经包含这一步。

## 取回产物

```bash
prlctl exec "Windows 11-ARM" cmd.exe /c "copy /Y \"C:\Users\<user>\dsh-build\src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis\dsh-1.0.1-x64-setup.exe\" \"\\\\Mac\\project\\APP\\Windows\\DeepSeek-Harness-1.0.1-x64-setup.exe\""

# 两侧哈希必须一致
shasum -a 256 APP/Windows/DeepSeek-Harness-1.0.1-x64-setup.exe | awk '{print toupper($1)}'
```

## 安装钩子

`src-tauri/installer/hooks.nsi` 通过 `tauri.conf.json` 的
`bundle.windows.nsis.installerHooks` 挂进去，作用是删掉改名前的旧
`dshdesk.exe`。详见 `07-pitfalls.md`。
