import { createHash } from "node:crypto";
import { cp, mkdir, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const config = JSON.parse(
  await readFile(join(projectRoot, "src-tauri", "tauri.conf.json"), "utf8"),
);
const version = config.version;
const platform = process.argv[2]?.toLowerCase();
const supported = new Set(["macos", "windows"]);

if (!supported.has(platform)) {
  console.error("用法: node scripts/build-artifacts.mjs macos|windows");
  process.exit(2);
}

const isMac = platform === "macos";
const buildTarget = process.env.TAURI_BUILD_TARGET;
const bundleDir = join(
  projectRoot,
  "src-tauri",
  "target",
  ...(buildTarget ? [buildTarget] : []),
  "release",
  "bundle",
  isMac ? "dmg" : "nsis",
);
const outputDir = join(projectRoot, "APP", isMac ? "macOS" : "Windows");
// Derive the architecture from the Rust target triple so ARM builds are not mislabelled as x64.
const architecture = process.env.DSH_ARTIFACT_ARCH
  ?? (buildTarget?.startsWith("aarch64")
    ? "arm64"
    : buildTarget?.startsWith("x86_64")
      ? "x64"
      : isMac ? "aarch64" : "x64");
const expectedExtension = isMac ? ".dmg" : ".exe";

await rm(outputDir, { recursive: true, force: true });
await mkdir(outputDir, { recursive: true });
const entries = (await readdir(bundleDir, { withFileTypes: true })).filter((entry) =>
  entry.isFile() && entry.name.toLowerCase().endsWith(expectedExtension),
);
if (entries.length === 0) {
  throw new Error(`未找到 ${platform} 构建产物: ${bundleDir}`);
}

const source = entries.sort((a, b) => a.name.localeCompare(b.name))[0];
const targetName = isMac
  ? `DeepSeek-Harness-${version}-${architecture}.dmg`
  : `DeepSeek-Harness-${version}-${architecture}-setup.exe`;
const target = join(outputDir, targetName);
await cp(join(bundleDir, source.name), target);

const digest = createHash("sha256");
const content = await readFile(target);
digest.update(content);
const fileInfo = await stat(target);
const manifest = {
  product: config.productName,
  version,
  platform,
  architecture,
  file: targetName,
  bytes: fileInfo.size,
  sha256: digest.digest("hex"),
  signed: false,
  notarized: false,
  source: `src-tauri/target/${buildTarget ? `${buildTarget}/` : ""}release/bundle/${isMac ? "dmg" : "nsis"}/${source.name}`,
};
await writeFile(join(outputDir, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`已归档 ${target}`);
console.log(`SHA-256 ${manifest.sha256}`);
