# 04 · macOS 功能验证

核心原则：**每个开关都要能观察到真实的系统行为**，不能只看它把布尔值存下来了。
下面每条都是实测跑通的观察方法。

## 防休眠

用户曾问"我看不出有没有休眠，都会黑屏"。不用等真的休眠，查系统的电源断言即可：

```bash
pmset -g assertions | grep -iE "caffeinate|PreventUserIdleSystemSleep"
```

开关打开后应看到（含明确的代理关系）：

```
pid 4430(caffeinate): PreventUserIdleSystemSleep
    Details: caffeinate asserting on behalf of Process ID 4420
```

确认 4420 就是本应用：

```bash
ps -o pid,command -p 4420 | tail -1
pmset -g | grep "^ *sleep"   # 应出现 "sleep prevented by ... caffeinate"
```

**最容易漏的是释放那一半**：退出应用后 caffeinate 必须随之消失，
否则用户关掉开关或退出程序后机器永远不睡。

```bash
pkill -f "/Applications/DeepSeek Harness.app/Contents/MacOS/"; sleep 4
pgrep -f "caffeinate -i -w" >/dev/null && echo "泄漏！" || echo "已正确释放"
```

## 桌面通知

实现走 `UNUserNotificationCenter`（`src-tauri/src/notify_mac.rs`）。
老接口 `NSUserNotification` 靠 hook 伪造 bundle 身份，会出现"有名字没图标"。

### 查授权状态

授权状态记在系统里，程序改不了。写个探针 app 读它：

```bash
mkdir -p /tmp/np && cd /tmp/np
cat > check.m <<'OBJC'
#import <Foundation/Foundation.h>
#import <UserNotifications/UserNotifications.h>
#import <AppKit/AppKit.h>
int main(void){@autoreleasepool{
  [NSApplication sharedApplication];
  [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
  UNUserNotificationCenter* c=[UNUserNotificationCenter currentNotificationCenter];
  [c getNotificationSettingsWithCompletionHandler:^(UNNotificationSettings* s){
    NSArray* n=@[@"NotDetermined",@"Denied",@"Authorized",@"Provisional",@"Ephemeral"];
    NSLog(@"AUTH=%@", n[s.authorizationStatus]); exit(0);
  }];
  [NSApp run];}return 0;}
OBJC
clang -fobjc-arc -framework Foundation -framework UserNotifications -framework AppKit -o probe check.m
# 必须放进有 Info.plist 的 .app 里，bundle id 写成 com.dshdesk.app
```

**裸二进制不行**：没有 bundle 身份时 `currentNotificationCenter` 直接抛
`bundleProxyForCurrentProcess is nil` 崩掉进程。`notify_mac.rs` 里的
`has_bundle_identity()` 就是为挡这个而存在。

### 状态是 Denied 时

一旦用户忽略过授权弹窗，macOS **永久记住拒绝**：
`requestAuthorization` 立刻返回失败、不再弹窗，`tccutil reset` 也无效
（通知权限不在 TCC 数据库里）。唯一出路是手动开：

```bash
open "x-apple.systempreferences:com.apple.Notifications-Settings.extension"
```

代码里已经有 `open_notification_settings` 命令，失败时界面会给出这个按钮。

### 确认投递结果

```bash
grep -iE "notification|UN 通道" ~/Library/Application\ Support/com.dshdesk.app/logs/app.log | tail -5
```

## 开机自启

用 LaunchAgent，**不要用 System Events 的登录项 API**：后者要通过 AppleScript
驱动 System Events 进程，系统会弹"System Events 正在后台运行"这种与本应用
无关的提示，用户会困惑（真实反馈踩过）。

```bash
# 文件在不在
ls ~/Library/LaunchAgents/com.dshdesk.app.launcher.plist

# launchd 是否真的登记了（只看文件存在不够）
launchctl print "gui/$(id -u)/com.dshdesk.app.launcher" >/dev/null 2>&1 \
  && echo "已登记" || echo "未登记"
```

两个必须知道的约束：

1. **Label 不能用 bundle identifier**。应用运行时 macOS 会在 GUI 域自动注册
   `application.com.dshdesk.app.<hash>`，同名 LaunchAgent 会被 launchd 以
   `Input/output error` 拒绝加载——文件躺在磁盘上但作业从未登记，
   开关显示"已开启"而开机后什么都不发生。所以用 `com.dshdesk.app.launcher`。

2. **从 DMG 直接运行时不能开**。登录项会指向 `/Volumes/...`，卸载镜像后失效。
   代码里已拦住并提示"请先把应用拖入应用程序文件夹"。

## 插件市场行为

市场是 JS 实现的，可以直接调它自己的函数，比点界面可靠得多：

```bash
cd /tmp && mkdir -p mk && cd mk
cat > probe.mjs <<'JS'
const dir = process.env.PROFILE_DIR
const mkt = `${dir}/node_modules/dshmarket/lib`
const { checkUpdates } = await import(`${mkt}/updates.js`)
const { readInstalled, readInstalledVersion } = await import(`${mkt}/profile.js`)
const installed = readInstalled('web', dir)
for (const [n, s] of Object.entries(installed)) {
  console.log(`  ${n} = ${s}  已装=${readInstalledVersion('web', n, dir)}`)
}
const r = await checkUpdates('web', true, dir)   // force=true 跳过 TTL 缓存
for (const [n, v] of Object.entries(r)) {
  console.log(`  ${n}: kind=${v.kind} current=${v.current} latest=${v.latest} update=${v.updateAvailable}`)
}
JS
NODE="$HOME/Library/Application Support/com.dshdesk.app/runtime/bin/node"
PROFILE_DIR="$HOME/Library/Application Support/com.dshdesk.app/dsh-home/profiles/web" "$NODE" probe.mjs
```

判读要点：
- `kind=npm` → 能通过市场更新
- `kind=linked` → spec 是 `link:`/`file:`，市场**永久放弃更新**它

也可以直接问 HTTP 接口（不需要图形界面）：

```bash
# 应用运行时端口在 get_state 里；或自己起一个
curl -s http://127.0.0.1:<port>/dsh-market/installed | python3 -m json.tool | head -30
```

关注 `installed`（来自 `dependencies`）、`present`（能读到 package.json 的）、
`bundles`（来自 `dsh.profile.bundles`）三个字段。

## 日志

```bash
tail -30 ~/Library/Application\ Support/com.dshdesk.app/logs/app.log
```

自查关键字：`install_kernel`、`spawn_dsh`、`notification`、
`恢复插件市场登记`、`已放开插件原生依赖的构建脚本`、`清理失效 profile bundle`。
