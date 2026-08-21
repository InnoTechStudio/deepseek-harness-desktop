# DeepSeek Harness 桌面客户端 —— 技术设计文档 v2

> 2026-08-22　｜　面向普通用户：**安装即用、免装任何环境、双击就开、自动升级**

## 0. 产品原则（先定死）

面向普通用户的四条铁律，任何设计冲突时以此为准：

1. **零前置依赖**：用户机器上不需要装 Node / npm / pnpm / Git / 任何驱动。运行时要么打进安装包，要么首次启动自动下载。
2. **安装即用**：双击安装 → 首启自动准备环境 → 直接打开可用的 Harness 界面。全程无命令行、无黑窗口。
3. **自解释 UI**：所有过程（下载、升级、出错）都有中文进度条 / 提示 / 一键修复按钮，不抛技术报错让用户猜。
4. **自动升级不打扰**：后台静默检测，发现新版征询用户，一键确认，失败绝不影响现有使用。

### v2 变更（2026-08-22，用户拍板）

| 项 | 决定 |
|---|---|
| Node runtime | **首启自动下载**（不进安装包 → 安装包 ≈5MB）；内核也首启下载 |
| 下载源策略 | **运行时动态测速选最快的镜像源**（见 §2.2） |
| 应用名/图标 | 暂用占位名 `DSH Desk` + 临时图标，后续用户提供再改 |
| 版本策略 | **全部升级，不区分 rc**（当前全是 rc 版） |
| 签名/公证 | 见 §7（要钱 + 省钱方案） |

## 1. 总体架构

```
┌────────────────────────────────────────────────────────┐
│  Tauri 2 (Rust) 原生壳 —— 安装包本体（≈5MB，不含 Node）    │
│                                                          │
│  ├─ 首启引导状态机：准备环境 → 下载官方内核 → 打开界面     │
│  ├─ 下载加速器：动态测速选最快镜像源（npm / Node）        │
│  ├─ 生命周期管理：启动/守护/重启官方 dsh 进程             │
│  ├─ 官方内核升级引擎（npm overlay，借鉴 EAC）             │
│  ├─ 壳自身上升（tauri-plugin-updater → GitHub Releases） │
│  ├─ 托盘 / 常驻 / 通知                                    │
│  └─ 预装 dsh-market 插件（GitHub DSH 插件商城）           │
└───────────────┬────────────────────────────┬────────────┘
                │  spawn + 127.0.0.1:0(随机端口)          │  HTTP 200 轮询
        ┌───────▼────────┐            ┌───────────▼─────────┐
        │ 内置 Node 运行时 │            │ 官方内核 @deepseek-ai/dsh │
        │  (node v22.22) │  ────────►  │  dsh web --host 127.0.0.1 │
        └────────────────┘   spawn     └───────────┬─────────┘
                ▲                                  │ 加载 Web UI
        ┌───────┴────────┐            ┌───────────▼─────────┐
        │  数据目录        │            │  原生窗口内嵌 WebView   │
        │  ~/Library/... │  ◄───────  │  （官方界面原样，非套壳） │
        │  + overlay 内核 │  升级时原子替换                    │
        └────────────────┘            └──────────────────────┘
```

关键决策：
- **安装包只含壳（≈5MB）**：Node runtime 与官方内核都首启自动下载（≈40MB Node + ≈40MB 内核）。安装包极小，代价是首启下载量。**下载加速由动态选源解决**（见 §2.2）。
- **官方内核放数据目录**（用户可写、可替换），NSIS 安装版和 portable 版都能升级。

## 2. 模块拆解与实现细节

### 2.1 环境依赖处理（需求①）

| 依赖 | 策略 | 大小 |
|---|---|---|
| Node runtime | **打进安装包**（portable 版，含 npm CLI） | macOS ~40MB / Windows ~25MB |
| 官方内核 `@deepseek-ai/dsh` | **首次启动自动下载**到数据目录 | ~40MB |
| Git / 其它工具 | 不需要。dsh 内部依赖（Cordis 全家桶）随 npm 包自动安装 | 0 |

- Node runtime 固定版本（如 v22.22.0），**不随官方升级而变化**，所以打进包一次、终身使用，同时省去用户等待。
- 内核每次启动对比本地与 npm 最新版，过期自动准备升级（见 2.3）。
- 优化点：若安装包也想压到 ~5MB，可把 Node runtime 也改为首启下载（加 ~40MB 首启流量）。**默认推荐内置 Node**，把"首次打开下载量大"和"安装包大"的取舍留给用户选。

### 2.2 首次启动引导（安装即用体验）

用户安装后第一次打开，走一个**三步状态机**，每步都有中文进度和"重试"按钮：

```
准备环境（解压内置 Node runtime，秒级）
   ↓
下载官方内核 @deepseek-ai/dsh@latest（~40MB，带进度条，断点续传，镜像源自动切换）
   ↓
启动 dsh web --host 127.0.0.1 --port 0 → 轮询 HTTP 200 → 加载界面 → 结束引导
```

- 引导页是壳自带的 React 页面（Tauri WebView），不依赖网络资源。
- 引导完成后最小化到托盘，用户看到的就是"装好就能用"。
- 首次引导可选预装推荐插件（勾选式），默认预装 dsh-market（插件商城）。

### 2.2 下载加速：运行时动态测速选最快镜像源（v2 新增）

普通用户在中国直连 `registry.npmjs.org` / `github.com` 会很慢（实测首包 2.7–3s，下载大文件更慢）。方案是**每次需要下载时，先对候选源做一次极快的测速，选最快的下载**：

**实测数据（本机，2026-08-22，HTTP 首包到达时间中位数）：**

| 源 | 类型 | 中位数 |
|---|---|---|
| `cdn.npmmirror.com/binaries/node` | Node 官方二进制镜像 | **85ms** |
| `registry.npmmirror.com` | npm 镜像（阿里） | **161ms** |
| `api.github.com` | GitHub API | 2196ms |
| `jsdelivr.net` | CDN | 2528ms |
| `nodejs.org` | Node 官方 | 2719ms |
| `registry.npmjs.org` | npm 官方 | 2842ms |
| `raw.githubusercontent.com` | GitHub 原始文件 | 2902ms |

结论：**中国网络下国内镜像快 10–20 倍**；但其它国家/地区情况未知，所以必须动态测速，不写死。

**v3 新增实测（2026-08-22，本次重点验证「版本检测 + 升级下载」走第三方源的可行性）：**

| 用途 | 源 | 首包中位数 |
|---|---|---|
| 版本检测/内核 tarball | **华为云 npm** `repo.huaweicloud.com/repository/npm` | **~100ms** |
| 版本检测/内核 tarball | **阿里 npmmirror** `registry.npmmirror.com` | **~160–266ms** |
| 版本检测/内核 tarball | **腾讯云 npm** `mirrors.cloud.tencent.com/npm` | ~230–390ms |
| Node runtime | **npmmirror-node** `cdn.npmmirror.com/binaries/node` | **~145ms** |
| Node runtime | 腾讯/华为 node 镜像 | ~250–300ms |
| GitHub API（官方版本号） | `api.github.com` | **~2.2–3.8s（最慢，且时通时断）** |
| npm 官方 / jsdelivr / nodejs.org | — | 1.6–3.6s（慢） |

**结论确认**：华为云、阿里、腾讯三大国内 npm 镜像**版本检测 + 内核 tarball 全流程都能用，且比 GitHub API 快 10–40 倍**——完全满足"升级检测/升级对象用第三方分发源"的需求，还能在 GitHub 被墙时照常更新。

**第三方源准入标准（写进代码的候选源要求）：**
- ✅ **完整**：镜像的是官方 npm registry 全量（`@deepseek-ai/dsh` 及全部依赖可解析），且每次访问都拉实时最新元数据（不缓存过期版本号）
- ✅ **快**：国内实测首包 <500ms（华为/阿里/腾讯达标；官方源作兜底，仅用于极端兜底）
- ✅ **稳定**：厂商级 SLA，长年可用（阿里/腾讯/华为均为公有云大厂）
- ✅ **官方同步及时**：npm 镜像通常分钟级同步官方新版本，不会错过更新
- ❌ **排除**：个人小镜像、图床类 CDN（jsdelivr 只能拿静态文件，拿不到 npm 元数据）、自建源

**动态选源算法（v3）：**

```
候选源（版本检测+内核：npmjs官方 + 华为云 + 阿里 + 腾讯；Node：官方 + npmmirror-node + 腾讯 + 华为）
    │  并行对每个源发一个 256B Range 请求（只取首包，50ms 超时）
    │  = 极快测速（并行 + 短超时 + 只读 256B，代价极小）
    ▼
按首包到达时间排序 → 选最快的
    │  下载大文件（Node tarball / 内核 tarball）走选中的源
    │  失败/停滞 → 自动换下一个源（每次下载都是"选源→下载→失败切源"）
    ▼
同时把这个选择写入本地缓存（下次可直接沿用，除非失败）
```

- 测速只发 256B 的 HTTP Range 请求，并行 + 50ms 超时，总耗时 <1s，对普通用户几乎无感。
- **不写死、不靠 IP 判断**（IP 段判断会误伤跨国用户）；每次都实测，任何地区都拿最优。
- 所有下载（首启 Node、首启内核、内核升级、壳升级、插件安装）共用这一套"选源→下载→失败切源"逻辑。
- npm 安装命令层面同样用 `--registry=<测速最快源>` 传入。
- 兼容：若某源对用户不可达（404/超时/证书），自动跳过，不影响下载。
- 兜底链：最快的失败 → 依次试次快 → … → 最后回官方源。
- **版本检测同样走第三方源**（不再依赖 GitHub API 当主源）：`GET {registry}/{pkg}/latest` 即可拿到官方最新版本号；GitHub 仅作交叉兜底。

### 2.3 官方内核自动升级引擎（需求③，最核心）

这是用户明确要的："**检测官方 DeepSeek Harness 项目 → 升级底层**"。官方 dsh 的发布渠道是 npm `@deepseek-ai/dsh`，升级引擎直接检测它。

完整流程（借鉴 EAC `updater.js` 的实现，已验证）：

```
定时触发（启动后 + 每 6h）+ 手动"检查更新"
    │
    ▼ 1. checkLatest：GET {测速最快 npm 镜像}/{pkg}/latest → 官方最新版本号
    │    镜像链：动态测速选源（华为/阿里/腾讯/npmjs）→ 失败/停滞自动切换（150s 停滞判死）
    │    GitHub API 仅作交叉兜底（第三/第四顺位），避免 GitHub 不可达时一直不更新
    │    失败静默不打扰
    │
    ▼ 2. semver 比较（含 0.1.x-rc.N 预发布规则；v2：rc 也照常升级）
    │
    ▼ 3. 有新版 → 弹窗："发现 DeepSeek Harness 官方更新 vX.Y.Z，是否立即更新？"
    │     选项：立即更新 / 跳过此版本 / 稍后
    │
    ▼ 4. applyUpdate：npm install --prefix staging @deepseek-ai/dsh@X.Y.Z --registry=<测速最快源>
    │     装到 <userData>/agent-staging（全新目录，失败零影响）
    │     实时进度（fetch / install / done）+ 动态选源 + 停滞换源
    │
    ▼ 5. 原子切换：overlay(agent) → agent-old-<ts>(备份，含配置快照)
    │             staging → agent（rename 两步，任一步失败自动回滚）
    │
    ▼ 6. 重启 dsh 进程，用 overlay 版本（dshBin 优先 overlay > 内置）
    │
    ▼ 7. 健康确认：新版能正常启动并响应 HTTP 200 → 清理旧备份
    │            启动失败 → 弹窗"一键回退到上一版本"（保留 agent-old）
```

- **升级对象是官方 npm 包**（经第三方镜像源拉取，保证速度快 + GitHub 不可达也能更新）——满足"基于官方 DeepSeek Harness 自动检测升级"。
- 交叉兜底：GitHub `releases/tags` 仅作版本号来源的第三/第四顺位（避免单一依赖）；主源为 npm 镜像（华为/阿里/腾讯），秒级返回。
- **v2：当前上游全是 rc 版，全部照常升级**（不区分预发布，设置里保留"跳过某个版本"供用户临时躲坑）。

### 2.4 壳自身上升（Tauri updater）

- 用官方 `tauri-plugin-updater`，指向**我们自己的 GitHub Releases**（发布新壳版本时附 .dmg/.exe + .sig 签名）。
- 后台静默检查 → 发现新版弹窗征询 → 下载安装包 → macOS 打开 DMG / Windows 引导安装器。
- 壳与内核是**两套独立升级**：壳负责界面/托盘/引导；内核负责 agent 能力。互不阻塞。

### 2.5 插件商城（聚合 GitHub DSH 插件）

**结论：直接预装 dsh-market，不自己造。** 它已完美实现你要的功能，且是官方 dsh 生态的标准做法：

- **数据源**：`awesome-dsh-plugin.com/plugins.json` —— 每天由 CI 从 GitHub 精选列表聚合刷新（1550+ 插件，中文名"Awesome DSH Plugin"列表，收录了全部可装插件）。
- **功能**：浏览/搜索/分类/按 star 排序、App Store 式截图、**一键安装**（走 npm tarball，校验 `repository` 指回同一 GitHub 仓库防冒名）、逐插件更新、卸载、主题一键切换、备份/恢复。
- **安全**：只允许安装精选列表内来源、构建脚本默认禁止执行、安装接口仅同源 POST。
- **集成方式**：首启引导默认勾选预装 `dsh plugin --profile web add dshmarket`；桌面壳设置里提供"插件商城"入口直达。
- 如果你的网络访问不了数据源域名，可设 `DSHM_REGISTRY_URL` 指向你自己的镜像（官方支持）——这给"自建聚合"留了口子，但**默认用官方精选列表**。

判断：自己写一个聚合 GitHub 插件的商城，等于重做 dsh-market + awesome-dsh-plugin 的聚合与安全体系，工作量大且安全风险高。**预装 dsh-market 是更优解**，符合"已有类似插件可供参考"。

### 2.6 托盘 / 常驻 / 通知

- 关闭窗口 → 最小化到托盘，agent 任务继续跑；托盘菜单：打开 / 检查更新 / 退出。
- 会话完成通知（可选）：可交给 `dsh-notification` 插件，不占壳代码。
- 单实例锁：重复启动聚焦已有窗口。

### 2.7 错误处理与恢复

| 场景 | 处理 |
|---|---|
| 内核下载失败 | 镜像源自动切换 + "重试"按钮；已保留旧内核则继续用 |
| 新版内核启动失败 | 自动检测健康，弹"一键回退上一版本" |
| 升级中途断电/被杀 | staging 目录下次启动自动清理；原子 rename 保证工作副本不被破坏 |
| 端口冲突 | `--port 0` 让系统分配随机端口 |
| macOS 未公证 | 首次启动系统"仍要打开"放行一次（或后续做公证，见风险） |

## 3. 技术选型与工具链

| 组件 | 选型 | 理由 |
|---|---|---|
| 壳 | Tauri 2 (Rust + WebView) | 体积小（5MB vs Electron 200MB）、内存低、原生窗口 |
| 前端 | React + TypeScript（引导/设置 UI） | Tauri 标配 |
| 内核 | `@deepseek-ai/dsh`（官方 npm，原样运行） | 不魔改官方，可随官方升级 |
| 升级引擎 | 自研（照 EAC updater.js 模式） | 官方 npm overlay + 原子切换 + 回退 |
| 壳更新 | tauri-plugin-updater | 官方方案，指向自建 GitHub Releases |
| 插件商城 | 预装 dsh-market | 成熟、安全、聚合 GitHub 精选列表 |
| 打包 | tauri build + GitHub Actions 矩阵 | macOS DMG + Windows NSIS/portable |

**本机（Mac arm64）需要装**：`rustup`（Rust 工具链）+ `node` + `pnpm` + `@tauri-apps/cli`。装完即可在 macOS 上编译出 DMG。Windows 包后续由 GitHub Actions 的 windows runner 构建。

## 4. 目录结构（规划）

```
deepseek-harness-desktop/
├── src-tauri/                  # Rust 壳
│   ├── src/
│   │   ├── main.rs             # 入口、单实例
│   │   ├── tray.rs             # 托盘
│   │   ├── lifecycle.rs        # dsh 进程 spawn/守护/重启
│   │   ├── bootstrap.rs        # 首启状态机（准备/下载/就绪）
│   │   └── updates.rs          # 内核升级引擎 + 壳更新
│   ├── capabilities/
│   └── tauri.conf.json
├── src/                        # React：引导页、设置页
├── resources/
│   ├── node-runtime/           # 内置 Node（打包时注入，不进 git）
│   └── preset-plugins.json     # 首启推荐插件（默认含 dshmarket）
├── updater/                    # 内核升级引擎逻辑（TS/JS，可单测）
│   ├── check-latest.ts         # npm view + 镜像链
│   ├── apply-update.ts         # staging 安装 + 原子切换 + 回退
│   └── semver.ts               # rc 预发布比较
├── scripts/
│   ├── speed-test.ts            # 动态测速选源（并行 Range 首包）
│   └── release.yml             # GitHub Actions 构建发布
└── README.md
```

## 5. 里程碑计划

| 阶段 | 内容 | 产出 | 预计 |
|---|---|---|---|
| M0 | 装工具链（rustup/node/pnpm/tauri-cli） | 本机可编译 | 1 次 |
| M1 | **最小壳跑通**：Tauri 壳 + 首启下载 Node/内核（动态选源）+ 启动官方 dsh + 加载 Web UI + 托盘 | macOS 可用安装包（未公证） | 核心 |
| M2 | **官方内核自动升级引擎**：检测→staging→原子切换→失败回退 | 升级链路全绿 | 核心 |
| M3 | 首启引导 UI + 预装 dsh-market 插件商城 + 壳自身上升 | 完整产品体感 | 核心 |
| M4 | 打包发布：macOS DMG（可选签名/公证）、Windows NSIS/portable、GitHub Actions CI + 更新源 | 可对外分发 | 收尾 |

## 6. 风险与对策

| 风险 | 影响 | 对策 |
|---|---|---|
| 官方 dsh 仍在预览期、承诺破坏性变更 | 新版可能崩 | overlay 原子切换 + 一键回退；设置里可"跳过此版本" |
| 首启需下载 Node(≈40MB)+内核(≈40MB) | 首启等待时间长 | 动态测速选最快源 + 断点续传 + 并发；Node 下载后可本地缓存，升级不重下 |
| 下载依赖 npm/GitHub 可达性 | 首启失败 | 动态测速（8+ 源）+ 失败自动切源 + 重试按钮 |
| macOS 未签名要 Gatekeeper 放行 | 安装体验打折 | 见 §7 签名省钱方案 |
| Windows 需在 Windows 上验证 | 无法本机全测 | M4 用 GitHub Actions windows runner |
| 插件来源安全性 | 第三方代码风险 | 沿用 dsh-market 安全策略（精选列表白名单、禁构建脚本） |

## 7. 签名 / 公证成本与省钱方案（v2）

### 需要什么、要多少钱

| 项 | 用途 | 成本 | 是否必须 |
|---|---|---|---|
| **Apple Developer Program**（个人 $99/年） | ① 对 mac 安装包签名（Developer ID）② notarization 公证（去掉"未知开发者"）③ 上 Mac App Store | **$99/年** | mac 免"仍要打开"提示需要 |
| **Windows 代码签名证书** | 给 .exe 签名，去掉 SmartScreen"未知发布者"红屏 | 机构证书（OV/EV）$200–400/年；**个人 OV 证书 ~$100–150/年**；有的厂商按年签 | Windows 免红屏需要 |
| Microsoft Store 个人开发者 | 只影响上架商店（Store），与"装自己分发的 exe"无关 | $19/一次性 | 不必须 |

> 说明：macOS 上签 Developer ID 证书本身要 Apple 开发者账号；Windows 代码签名证书来自第三方 CA（如 Certum、DigiCert），不需要微软账号。

### 免费的 / 可绕过的方案（重点研究）

| 方案 | 平台 | 要钱吗 | 实际效果与坑 |
|---|---|---|---|
| **SignPath 开源赞助签名** | Windows EXE（也支持 Apple/容器/Linux） | **$0（开源项目）** | 面向开源项目提供免费代码签名（"sponsored signing / available at no cost"），走 CI 集成（GitHub Actions）；需项目公开、符合其赞助条件，走申请流程。**前提：你的项目得是开源的** |
| **Microsoft Trusted Signing（Azure）** | Windows EXE | **免费层存在** | 微软托管签名服务：Basic SKU 含 5,000 签名/月免费额度，**签名结果能直接过 SmartScreen（微软自家签）**；用 Azure 免费账户 + $200 试用额度起步。相当于把"买 OV 证书"变成"用微软的证书签"——**你的 exe 显示"Microsoft"发布者，天然受信任** |
| Certum 免费 OV 试用 | Windows EXE | 试用期免费，长期要钱 | 有免费试用/限时，长期仍需购买 OV 证书 |
| 自签证书 + signtool | Windows EXE | $0 | 只能自己机器过；分发给别人仍红屏，**不可作为交付方案** |
| Apple 无账号方案 | macOS | — | **不存在**。mac 的 Developer ID 签名 + notarization 强依赖 $99/年的付费账号；教育机构可免费（我们不适用）。**这是 mac 绕不开的唯一花钱点** |

### macOS 为什么绕不开（要讲清楚）

- Apple 规定：要分发"双击即装、无警告"的 mac 应用，必须用 **Developer ID 证书签名 + notarization**，而这两个能力**只对付费的 Apple Developer Program（$99/年）开放**。
- 免费账号只能本地装自己设备（`xcodebuild`），**不能用于对外分发**。
- 所以 mac 侧**没有免费路径**。最优省钱：$99/年包全部 mac 能力（签名+公证，不上 App Store 免抽成）。**如果想 0 成本先跑，mac 只能接受"首次仍要打开"一次**。

### 推荐落地方案（省到极限）

1. **macOS（无法免费，只可省）**：
   - 0 成本起步：无签名版，用户首次"右键→打开→仍要打开"一次（可写一份中文说明贴在下载页）。
   - 有钱/要正式分发：$99/年做 Developer ID + 公证，**双击直接装**。这是 mac 唯一花钱点，绕不开。
2. **Windows（可做到 0 成本真签名）**：
   - **首选：Microsoft Trusted Signing 免费额度**——用微软的证书签名，SmartScreen 直接显示受信任（微软自家签名天然免红屏），Basic 5,000 签名/月免费。这就是"免费的、且真能去掉红屏"的方案。
   - 备选：开源后申请 SignPath 免费赞助签名（同样免红屏）。
   - 兜底：不签名接受红屏过渡。
3. **先后顺序**：M1–M3 全部用无签名版开发；M4 分发前决定 mac 是否 $99、Windows 是否接 Trusted Signing。

### 体验取舍

- macOS 不公证 = 首次"右键→打开→仍要打开"一次（不致命，普通用户略困惑）。
- Windows 有微软 Trusted Signing 免费额度 → **双击即装，无红屏**。
- **结论**：Windows 免费可做好体验；mac 若不想花 $99 只能接受"打开一次"或加说明。

## 8. 待确认问题（开工前）

1. ~~Node runtime 内置 vs 首启下载~~ → **已定：首启下载**（安装包 ≈5MB）。
2. ~~预发布版本~~ → **已定：全部升级**（当前全是 rc）。
3. **应用名/图标** → 先用占位 `DSH Desk` + 临时图标，后续你给再改。
4. **签名预算** → 见 §7。开工先用无签名版本跑通，签名流程后置。
