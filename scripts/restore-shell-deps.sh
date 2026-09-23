#!/usr/bin/env bash
# 恢复 dsh-desktop-shell 的构建依赖。
#
# 这些依赖有三个来源，单跑 pnpm install 是不通的：
#   1. 部分包只发布在 next 标签下（dsh-client-ui-slots 的 latest 指向旧版），
#      而 dsh-client-ui-theme 又 peer 依赖尚未发布的 dsh-settings 版本区间，
#      pnpm 解析不出满足条件的版本就整体失败；
#   2. 部分包只存在于已安装的 DSH 内核目录里，内核重装会打断这些符号链接；
#   3. rolldown / lightningcss 需要当前平台的原生 binding。
# 因此这里软链内核目录 + 按精确版本直取 tarball，绕开 peer 解析。
set -euo pipefail

cd "$(dirname "$0")/.."
SHELL_DIR="dsh-desktop-shell"
KERNEL="${DSH_KERNEL_DIR:-$HOME/Library/Application Support/com.dshdesk.app/agent/node_modules}"
NM="$SHELL_DIR/node_modules"

# 客户端自带的 Node 运行时，未装系统 Node 时也能跑通。
RUNTIME_BIN="${DSH_RUNTIME_BIN:-$HOME/Library/Application Support/com.dshdesk.app/runtime/bin}"
if ! command -v node >/dev/null 2>&1 && [ -x "$RUNTIME_BIN/node" ]; then
  PATH="$RUNTIME_BIN:$PATH"
  export PATH
fi
if ! command -v node >/dev/null 2>&1; then
  echo "找不到 node：请安装 Node 或先让客户端安装内置运行时" >&2
  exit 1
fi

mkdir -p "$NM/@deepseek-ai" "$NM/@types" "$NM/.bin"

# 内核提供的包：软链过去，避免重复下载几百 MB。
for pkg in cordis dsh-client-locale dsh-client-runtime dsh-client-ui-settings \
           dsh-client-ui-theme dsh-client-ui-renderer dsh-client-ui-primitives; do
  if [ -d "$KERNEL/@deepseek-ai/$pkg" ]; then
    ln -sfn "$KERNEL/@deepseek-ai/$pkg" "$NM/@deepseek-ai/$pkg"
  fi
done
for pkg in react react-dom; do
  [ -d "$KERNEL/$pkg" ] && ln -sfn "$KERNEL/$pkg" "$NM/$pkg"
done
[ -d "$KERNEL/@types/node" ] && ln -sfn "$KERNEL/@types/node" "$NM/@types/node"

# registry 提供的包：名称与版本分开传，固定版本避免 latest 漂移。
node scripts/fetch-npm-packages.mjs \
  --pkg @deepseek-ai/dsh-client-ui-slots --at 0.0.1-rc.1 \
  --pkg @types/react --at 18.3.12 \
  --pkg @types/react-dom --at 18.3.1 \
  --pkg tsdown --at 0.22.14 \
  --pkg lightningcss --at 1.33.0

ln -sfn ../tsdown/dist/run.mjs "$NM/.bin/tsdown"
echo "shell deps ready — 运行 pnpm --dir $SHELL_DIR build 验证"
