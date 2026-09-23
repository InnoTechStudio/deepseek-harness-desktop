// 把 dsh-desktop-shell 的构建产物同步到 src-tauri/resources。
//
// Tauri 打包时只认 resources 里的副本，源目录改了不会自动带过去。
// 手工复制漏掉过一次：inject 清单里还留着内核已删除的包，
// 装出来的界面直接白屏，而源码看上去完全正常。
//
// 只复制 package.json 里 files 字段声明的东西 + package.json 自身，
// 与 npm 发布的内容保持一致，避免把 node_modules、src 一起塞进安装包。
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const source = join(root, "dsh-desktop-shell");
const destination = join(root, "src-tauri", "resources", "dsh-desktop-shell");

const manifest = JSON.parse(readFileSync(join(source, "package.json"), "utf8"));
const entries = ["package.json", "LICENSE", ...(manifest.files ?? [])];

rmSync(destination, { recursive: true, force: true });
mkdirSync(destination, { recursive: true });

const missing = [];
for (const entry of entries) {
  const from = join(source, entry);
  if (!existsSync(from)) {
    missing.push(entry);
    continue;
  }
  const to = join(destination, entry);
  mkdirSync(dirname(to), { recursive: true });
  cpSync(from, to, { recursive: true });
}

// lib/ 是 tsdown 的产物；缺了它插件根本加载不了，必须当成硬错误。
if (!existsSync(join(destination, "lib", "client.js"))) {
  console.error("同步失败：缺少 lib/client.js，请先在 dsh-desktop-shell 里跑 npm run build");
  process.exit(1);
}
if (missing.length > 0) {
  console.warn(`跳过不存在的条目: ${missing.join(", ")}`);
}
console.log(`已同步 ${entries.length - missing.length} 项到 src-tauri/resources/dsh-desktop-shell`);
