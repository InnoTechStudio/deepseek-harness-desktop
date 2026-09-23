// 生成 update.json：客户端更新检查读的那个静态清单。
//
// 为什么不查 GitHub API：匿名调用限 60 次/小时，按 IP 计，国内用户往往
// 共用一个出口，几分钟就打满了；而且 api.github.com 国内直连不稳，
// 没法交给镜像。静态文件没有这两个问题——镜像能缓存它，也不需要凭据。
//
// 用法：
//   node scripts/make-update-manifest.mjs \
//     --version=1.0.7 \
//     --macos-glob="release-assets/*.dmg" \
//     --windows-glob="release-assets/*.exe" \
//     --changelog=APP/CHANGELOG.md \
//     --out=update.json
//
// 这条命令在 CI 里由 release workflow 调用。

import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = parseArgs(process.argv.slice(2));

const version = args.version;
if (!version) {
  console.error("缺少 --version");
  process.exit(1);
}

function parseArgs(argv) {
  const out = {};
  for (const arg of argv) {
    const m = arg.match(/^--([^=]+)=?(.*)$/);
    if (m) out[m[1]] = m[2];
  }
  return out;
}

/**
 * 从目录里按扩展名挑文件。不用 glob 依赖：CI 里只装了项目依赖，
 * 引一个额外的包会让 workflow 多一步，而这里要的匹配本身就极简单。
 */
function findIn(dir, extension) {
  if (!existsSync(dir)) return null;
  const matches = readdirSync(dir)
    .filter((name) => name.toLowerCase().endsWith(extension))
    .sort();
  if (matches.length === 0) return null;
  return join(dir, matches[matches.length - 1]);
}

/** 读出一个安装包的文件名、大小、sha256 与发布地址。 */
function manifestFromFile(path) {
  const stats = statSync(path);
  const name = path.split("/").pop();
  return {
    name,
    size: stats.size,
    sha256: createHash("sha256").update(readFileSync(path)).digest("hex"),
    // Release 资产的稳定地址；镜像前缀由客户端按测速结果拼上去。
    url: `https://github.com/InnoTechStudio/deepseek-harness-desktop/releases/download/v${version}/${name}`,
  };
}

/** 从 CHANGELOG 找当前版本段，作发布说明。 */
function releaseNotes(changelogPath, version) {
  if (!existsSync(changelogPath)) return "";
  const text = readFileSync(changelogPath, "utf8");
  const lines = text.split("\n");
  const start = lines.findIndex((line) => line.startsWith(`## ${version}`));
  if (start === -1) return "";
  const rest = lines.slice(start + 1);
  const end = rest.findIndex((line) => line.startsWith("## "));
  return (end === -1 ? rest : rest.slice(0, end)).join("\n").trim();
}

const macosDir = resolve(args["macos-dir"] || "release-assets");
const windowsDir = resolve(args["windows-dir"] || "release-assets");
const macosPath = findIn(macosDir, ".dmg");
const windowsPath = findIn(windowsDir, ".exe");

if (!macosPath) {
  console.error(`找不到 macOS 安装包（在 ${macosDir} 里找 *.dmg），无法生成清单`);
  process.exit(1);
}
if (!windowsPath) {
  console.error(`找不到 Windows 安装包（在 ${windowsDir} 里找 *.exe），无法生成清单`);
  process.exit(1);
}

const macos = manifestFromFile(macosPath);
const windows = manifestFromFile(windowsPath);

const changelogPath = resolve(args.changelog || "APP/CHANGELOG.md");

const manifest = {
  version,
  notes: releaseNotes(changelogPath, version),
  publishedAt: new Date().toISOString().slice(0, 10),
  files: {
    macos,
    windows,
  },
};

writeFileSync(resolve(args.out || "update.json"), `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`已生成 update.json：macOS=${macos.name} Windows=${windows.name}`);
console.log(`  macOS sha256=${macos.sha256.slice(0, 16)}…`);
console.log(`  Windows sha256=${windows.sha256.slice(0, 16)}…`);
