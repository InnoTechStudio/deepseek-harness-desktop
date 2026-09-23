# 02 · 编译 macOS

## 完整流程

```bash
cd /Users/<user>/Documents/AI-Agent/Projects/DeepSeek-Harness-Desktop
export PATH="$HOME/Library/Application Support/com.dshdesk.app/runtime/bin:$PATH"

# 1) 桌面插件（改过 dsh-desktop-shell/ 才需要）
cd dsh-desktop-shell && pnpm build && cd ..

# 2) 关键一步：把产物同步进 Tauri 资源目录
cp dsh-desktop-shell/lib/client.js \
   dsh-desktop-shell/lib/client.js.map \
   dsh-desktop-shell/lib/index.js \
   dsh-desktop-shell/lib/invariant.js \
   src-tauri/resources/dsh-desktop-shell/lib/

# 3) 宿主前端
pnpm build

# 4) 后端测试
cd src-tauri && cargo test --lib && cd ..

# 5) 只出 .app（dmg 交给 DropDMG）
pnpm tauri build --bundles app

# 6) 用 DropDMG 打 dmg
node scripts/make-dmg.mjs
```

或者一条命令走完 5+6+归档：

```bash
pnpm build:macos
```

## dmg 用 DropDMG，不用 Tauri 自带的

**这是用户的明确要求，配置名固定为「单软件」。**

原因：Tauri 内部的 `bundle_dmg.sh` 出的镜像没有品牌化窗口布局
（背景图、图标位置、拖入 Applications 的提示）。DropDMG 里已经配好了
「单软件」配置（DropDMG 里提前配好的布局），产物还更小
（实测 8.25 MB vs 8.65 MB）。

命令行工具在 `/usr/local/bin/dropdmg`，配置存在
`~/Library/Application Support/DropDMG/Configurations/单软件.plist`。

```bash
dropdmg --config-name=单软件 --destination=<输出目录> "<path>/DeepSeek Harness.app"
```

已验证的几点：

- **配置名中文可以直接传**，不需要转义
- **版本号不用手动指定**：配置里 `appendVersionNumber` 是开的，DropDMG 自己从
  `.app` 的 `CFBundleShortVersionString` 读，产物叫 `DeepSeek Harness 1.0.2.dmg`
- **成功时 stdout 就是产物完整路径**，可以直接拿来校验
- 镜像内含 `Applications` 软链接、`.DropDMGBackground`、自定义卷图标，
  应用签名在镜像里依然有效

**必须后台跑**：这个命令阻塞时间长（首次要拉起 DropDMG 主程序，
我第一次前台跑撞上了 120 秒超时），`scripts/make-dmg.mjs` 里设了 300 秒上限
并做了产物存在性与数量的交叉校验。

## 第 2 步不能省

`pnpm build` 只更新 `dsh-desktop-shell/lib/`，Tauri 打包读的是
`src-tauri/resources/dsh-desktop-shell/`。不同步的话，界面改动**完全不会进包**，
而且没有任何报错——你会以为改的代码没生效，跑去怀疑逻辑。

验证是否同步到位：

```bash
diff -q dsh-desktop-shell/lib/client.js src-tauri/resources/dsh-desktop-shell/lib/client.js \
  && echo "已同步"
```

## 构建耗时与后台执行

release 编译约 100–150 秒。超过命令超时时间，用后台执行：

```bash
nohup pnpm tauri build --bundles app > /tmp/mac-build.log 2>&1 &
sleep 115
tail -5 /tmp/mac-build.log
```

成功的标志：

```
Finished 1 bundle at:
    .../bundle/macos/DeepSeek Harness.app
```

## 签名：用免费的 ad-hoc

`tauri.conf.json` 里已配置：

```json
"macOS": { "signingIdentity": "-" }
```

这是 ad-hoc 签名，不需要付费开发者账号。Tauri 原生支持，
**不要另写 codesign 脚本**（我写过一个，后来发现是重复劳动，已删）。

验证：

```bash
codesign --verify --deep --strict "/Applications/DeepSeek Harness.app" && echo "签名校验通过"
codesign -dvvv "/Applications/DeepSeek Harness.app" 2>&1 | grep -E "Identifier|Signature"
# Identifier=com.dshdesk.app
# Signature=adhoc
```

实测结论：**通知能显示本应用名称和图标，ad-hoc 签名就够了**，
前提是应用位于 `/Applications` 且已在 LaunchServices 注册。付费签名不解决任何额外问题。

## 装到 /Applications 并启动

```bash
pkill -f "/Applications/DeepSeek Harness.app/Contents/MacOS/" 2>/dev/null; sleep 2
rm -rf "/Applications/DeepSeek Harness.app"
cp -R "src-tauri/target/release/bundle/macos/DeepSeek Harness.app" /Applications/
codesign --verify --deep --strict "/Applications/DeepSeek Harness.app"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
  -f "/Applications/DeepSeek Harness.app"
open -a "/Applications/DeepSeek Harness.app"
sleep 35
pgrep -fl "DeepSeek Harness"
```

`lsregister -f` 不能省：LaunchServices 缓存不刷新会导致通知归属、
登录项路径解析出问题。

## 可执行文件名

`tauri.conf.json` 里有 `"mainBinaryName": "DeepSeek Harness"`。
默认会用 crate 名 `dshdesk`，那个名字会出现在系统权限请求弹窗里
（例如"dshdesk 想要录制屏幕"），用户看到内部代号很困惑。

改名带来一个升级陷阱，见 `07-pitfalls.md` 的 NSIS 残留条目。
