# 08 · 发版

## 版本号要改三处

```bash
src-tauri/tauri.conf.json   "version": "1.0.1"
package.json                "version": "1.0.1"
src-tauri/Cargo.toml        version = "1.0.1"
```

用脚本改，避免手滑改到别处（`Cargo.toml` 里依赖也有 version 字段）：

```bash
python3 - <<'EOF'
import re, pathlib
p = pathlib.Path('package.json'); t = p.read_text()
p.write_text(re.sub(r'("version"\s*:\s*)"1\.0\.0"', r'\1"1.0.1"', t, count=1))
p = pathlib.Path('src-tauri/Cargo.toml'); t = p.read_text()
p.write_text(re.sub(r'(?m)^version = "1\.0\.0"$', 'version = "1.0.1"', t, count=1))
EOF
```

`tauri.conf.json` 用 Edit 工具改（JSON 格式敏感），改完验证：

```bash
python3 -c "import json;json.load(open('src-tauri/tauri.conf.json'));print('合法')"
```

## 更新说明

**用户的长期要求：每次改版本号，都要自动对比上一版内容并写入 CHANGELOG，
面向顾客的介绍要简短。**

写在 `APP/CHANGELOG.md`，倒序排列。风格：用户能理解的现象描述，
不要提内部函数名和实现细节。

```markdown
## 1.0.1（2026-08-31）

修复桌面通知、安装进度与插件市场。

**修复**
- 安装内核时进度条不再长时间停在 44% 后突然跳变，改为按真实进度实时推进
- 桌面通知改用系统新接口投递，通知会显示本应用的名称和图标
- 统一应用在系统各处显示的名称，不再出现内部代号
```

对照写法：说"进度条不再卡住"，不说"把 output() 改成 spawn 流式读取"。

## 出包顺序

```bash
export PATH="$HOME/Library/Application Support/com.dshdesk.app/runtime/bin:$PATH"

# 1) 插件 + 同步资源 + 前端
cd dsh-desktop-shell && pnpm build && cd ..
cp dsh-desktop-shell/lib/* src-tauri/resources/dsh-desktop-shell/lib/
pnpm build

# 2) 两端测试都要过
cd src-tauri && cargo test --lib && cd ..

# 3) macOS：只出 .app，dmg 交给 DropDMG
nohup pnpm tauri build --bundles app > /tmp/mac.log 2>&1 &
# 编译完后（dropdmg 阻塞久，也要后台跑）
(node scripts/make-dmg.mjs > /tmp/dmg.log 2>&1 &) && sleep 60 && cat /tmp/dmg.log

# 4) Windows（同步 + 虚拟机构建，见 03）
/tmp/sync-to-vm.sh
# 改过版本号的话，必须刷新时间戳，否则 exe 里的版本资源不会重新生成：
prlctl exec "Windows 11-ARM" cmd.exe /c "cd /d C:\Users\<user>\dsh-build && powershell -NoProfile -Command \"(Get-Item 'src-tauri\tauri.conf.json').LastWriteTime = Get-Date; (Get-Item 'src-tauri\Cargo.toml').LastWriteTime = Get-Date\""
# … vm-build.cmd …
```

macOS 和 Windows 可以并行跑，互不干扰。

编译完核对 exe 版本号，别只信注册表：

```bash
prlctl exec "Windows 11-ARM" cmd.exe /c "powershell -NoProfile -Command \"(Get-Item '…\release\DeepSeek Harness.exe').VersionInfo.FileVersion\""
```

## 归档

macOS 有现成脚本（读 `bundle/dmg/` 下 DropDMG 产出的那个 dmg）：

```bash
node scripts/build-artifacts.mjs macos
# 输出：APP/macOS/DeepSeek-Harness-1.0.2-aarch64.dmg + manifest.json
```

打包完值得挂载确认一次内容，尤其是换了打包工具之后：

```bash
hdiutil attach -nobrowse -readonly APP/macOS/DeepSeek-Harness-1.0.2-aarch64.dmg
ls -la "/Volumes/DeepSeek Harness 1.0.2/"          # 应有 .app 与 Applications 软链接
plutil -p "/Volumes/…/DeepSeek Harness.app/Contents/Info.plist" | grep -i shortversion
codesign --verify --deep --strict "/Volumes/…/DeepSeek Harness.app"
hdiutil detach "/Volumes/DeepSeek Harness 1.0.2"
```

Windows 拷回来后手写 manifest：

```bash
python3 - <<'EOF'
import json, hashlib, pathlib
p = pathlib.Path('APP/Windows/DeepSeek-Harness-1.0.1-x64-setup.exe')
data = p.read_bytes()
m = {
  "product": "DeepSeek Harness", "version": "1.0.1", "platform": "windows",
  "architecture": "x64", "file": p.name, "bytes": len(data),
  "sha256": hashlib.sha256(data).hexdigest(), "signed": False, "notarized": False,
  "source": "src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/DeepSeek Harness_1.0.1_x64-setup.exe",
}
pathlib.Path('APP/Windows/manifest.json').write_text(json.dumps(m, indent=2, ensure_ascii=False) + "\n")
print('sha256:', m['sha256'][:16], m['bytes'], 'bytes')
EOF
```

**务必核对两侧哈希一致**，确认传输没出错：

```bash
shasum -a 256 APP/Windows/*.exe | awk '{print toupper($1)}'
prlctl exec "Windows 11-ARM" cmd.exe /c "powershell -NoProfile -Command \"(Get-FileHash -Algorithm SHA256 -LiteralPath '<vm路径>').Hash\""
```

## 发版前自查

- [ ] 三处版本号一致
- [ ] CHANGELOG 有本版条目，措辞面向用户
- [ ] `cargo test --lib` 在 macOS 与 Windows x64 目标都通过
- [ ] shell 产物已同步进 `src-tauri/resources/`
- [ ] macOS 包已装进 `/Applications` 并启动确认，`CFBundleShortVersionString` 是新版本号
- [ ] **macOS 的 dmg 是 DropDMG「单软件」配置打的**（不是 Tauri 自带的 bundle_dmg.sh）
- [ ] dmg 挂载确认：内含 .app 与 Applications 软链接、版本号正确、签名有效
- [ ] **Windows exe 的 `FileVersion` 是新版本号**（不是只看注册表）
- [ ] Windows 包已在虚拟机静默安装确认，**安装前先 taskkill**，装完哈希与编译产物一致
- [ ] 装完目录里没有 `dshdesk.exe`
- [ ] 两端 manifest 的 hash 与实际文件一致（用 shasum 复核一遍）
- [ ] 旧版本的包已从 `APP/` 移除
- [ ] 临时脚本已从仓库和虚拟机清理
- [ ] **明确告知用户哪些项没能实机验证**（尤其虚拟机测不了的图形交互）

## 当前状态

- 签名：ad-hoc（免费），未公证。首次打开需右键→打开，或在"隐私与安全性"里放行。
- Windows：仅 x64，未做 ARM64 包。
- 更新分发：GitHub Release（客户端通过镜像选源读取 update.json）。
