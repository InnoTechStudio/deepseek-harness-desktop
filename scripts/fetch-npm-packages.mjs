// 从 npm registry 直接取包并解压到 dsh-desktop-shell/node_modules。
//
// 为什么不用 pnpm：shell 的几个构建依赖在 registry 上的 latest 标签指向旧版
// （dsh-client-ui-slots），而 dsh-client-ui-theme 的 peer 依赖 dsh-settings
// 尚未发布对应版本，pnpm 会因为解析不出满足区间的版本而整体失败。这里按精确
// 版本号取 tarball，绕开 peer 解析。
import { createWriteStream } from "node:fs";
import { mkdir, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createGunzip } from "node:zlib";
import { pipeline } from "node:stream/promises";
import { spawn } from "node:child_process";

const REGISTRY = process.env.DSH_NPM_REGISTRY ?? "https://registry.npmmirror.com";
const shellRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "dsh-desktop-shell");
const modulesDir = join(shellRoot, "node_modules");

/** 解析 `--pkg <名称> --at <版本>` 序列。名称与版本分开传，避免 shell 处理 @ 时出岔。 */
function parseArgs(argv) {
  const wanted = [];
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--pkg") {
      const name = argv[i + 1];
      if (!name) throw new Error("--pkg 后面缺少包名");
      let version = "latest";
      if (argv[i + 2] === "--at" && argv[i + 3]) {
        version = argv[i + 3];
        i += 2;
      }
      wanted.push({ name, version });
      i += 1;
    } else {
      throw new Error(`无法识别的参数: ${argv[i]}`);
    }
  }
  return wanted;
}

async function exists(path) {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

async function metadata(name, version) {
  const encoded = name.replace("/", "%2f");
  const url = /^\d/.test(version)
    ? `${REGISTRY}/${encoded}/${version}`
    : `${REGISTRY}/${encoded}`;
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${name}@${version}: HTTP ${response.status}`);
  const body = await response.json();
  return /^\d/.test(version) ? body : body.versions[body["dist-tags"].latest];
}

/** 用系统 tar 解包：tarball 的根目录名不固定（@types 用裸类型名），统一 strip 一层。 */
async function extract(tarball, dest) {
  await mkdir(dest, { recursive: true });
  const response = await fetch(tarball);
  if (!response.ok) throw new Error(`下载失败 ${tarball}: HTTP ${response.status}`);
  const tmp = `${dest}.tgz`;
  await writeFile(tmp, Buffer.from(await response.arrayBuffer()));
  await new Promise((done, fail) => {
    const child = spawn("tar", ["-xzf", tmp, "-C", dest, "--strip-components=1"]);
    child.on("error", fail);
    child.on("exit", (code) => (code === 0 ? done() : fail(new Error(`tar 退出码 ${code}`))));
  });
  await rm(tmp, { force: true });
}

const installed = new Set();

async function install(name, version, depth = 0) {
  if (installed.has(name) || depth > 4) return;
  installed.add(name);
  const dest = join(modulesDir, name);
  if (await exists(dest)) return;

  let meta;
  try {
    meta = await metadata(name, version);
  } catch (error) {
    console.warn(`  ! 跳过 ${name}: ${error.message}`);
    return;
  }
  await extract(meta.dist.tarball, dest);
  console.log(`  + ${name}@${meta.version}`);

  for (const [dep, spec] of Object.entries(meta.dependencies ?? {})) {
    await install(dep, /^\d/.test(spec) ? spec : "latest", depth + 1);
  }
  // 原生 binding 在 optionalDependencies 里，按当前平台挑一个。
  const suffix = `${process.platform}-${process.arch}`;
  for (const dep of Object.keys(meta.optionalDependencies ?? {})) {
    if (dep.includes(suffix)) await install(dep, meta.version, depth + 1);
  }
}

let wanted;
try {
  wanted = parseArgs(process.argv.slice(2));
} catch (error) {
  console.error(error.message);
  wanted = [];
}
if (wanted.length === 0) {
  console.error("用法: node scripts/fetch-npm-packages.mjs --pkg <名称> [--at <版本>] ...");
  process.exit(2);
}
for (const { name, version } of wanted) {
  await install(name, version);
}
console.log(`完成，共处理 ${installed.size} 个包`);
