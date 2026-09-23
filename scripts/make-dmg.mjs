/**
 * 用 DropDMG 打 macOS 的 dmg。
 *
 * 为什么不用 `tauri build --bundles dmg`：它内部的 bundle_dmg.sh 出的镜像
 * 没有品牌化的窗口布局（背景图、图标位置、拖入提示）。DropDMG 里已经配好了
 * 「单软件」配置 + 「单软件」布局，产物也更小（实测 8.25 MB vs 8.65 MB）。
 *
 * 版本号不用传：配置里 appendVersionNumber 是开的，DropDMG 自己从 .app 的
 * Info.plist 读 CFBundleShortVersionString 并附到文件名上。
 */
import { spawn } from "node:child_process";
import { access, mkdir, readFile, readdir, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const CONFIG_NAME = process.env.DSH_DROPDMG_CONFIG ?? "单软件";
const DROPDMG = "/usr/local/bin/dropdmg";
/** 首次运行要拉起 DropDMG 主程序，会慢很多；给足余量。 */
const TIMEOUT_MS = 300_000;

const config = JSON.parse(
  await readFile(join(projectRoot, "src-tauri", "tauri.conf.json"), "utf8"),
);
const appPath = join(
  projectRoot,
  "src-tauri/target/release/bundle/macos",
  `${config.productName}.app`,
);
const outputDir = join(projectRoot, "src-tauri/target/release/bundle/dmg");

await access(DROPDMG).catch(() => {
  throw new Error(`DropDMG 命令行工具不存在：${DROPDMG}（在 DropDMG 里装一下）`);
});
await access(appPath).catch(() => {
  throw new Error(`未找到 .app：${appPath}\n先跑 pnpm tauri build --bundles app`);
});

// 清掉旧 dmg，避免归档脚本挑到上一版。
await rm(outputDir, { recursive: true, force: true });
await mkdir(outputDir, { recursive: true });

/** 跑 dropdmg，返回它打到 stdout 的产物路径。 */
function runDropDmg() {
  return new Promise((resolvePath, reject) => {
    const child = spawn(DROPDMG, [
      `--config-name=${CONFIG_NAME}`,
      `--destination=${outputDir}`,
      appPath,
    ]);
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error(`dropdmg 超时（${TIMEOUT_MS / 1000}s）未返回`));
    }, TIMEOUT_MS);

    child.stdout.on("data", (chunk) => { stdout += chunk; });
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    child.on("error", (error) => {
      clearTimeout(timer);
      reject(new Error(`无法启动 dropdmg: ${error.message}`));
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      if (code !== 0) {
        reject(new Error(`dropdmg 退出码 ${code}${stderr.trim() ? `：${stderr.trim()}` : ""}`));
        return;
      }
      // 成功时 stdout 就是产物完整路径；配置名写错等情况会输出空。
      const produced = stdout.trim().split("\n").filter(Boolean).pop();
      if (produced === undefined) {
        reject(new Error(`dropdmg 没有报告产物路径，检查配置名「${CONFIG_NAME}」是否存在`));
        return;
      }
      resolvePath(produced);
    });
  });
}

const produced = await runDropDmg();
await access(produced).catch(() => {
  throw new Error(`dropdmg 报告了路径但文件不存在：${produced}`);
});

// 交叉验证：目录里确实只有这一个 dmg，避免误判成功。
const dmgs = (await readdir(outputDir)).filter((name) => name.toLowerCase().endsWith(".dmg"));
if (dmgs.length !== 1) {
  throw new Error(`预期 1 个 dmg，实际 ${dmgs.length} 个：${dmgs.join(", ")}`);
}

console.log(`DropDMG 配置「${CONFIG_NAME}」已生成:`);
console.log(produced);
