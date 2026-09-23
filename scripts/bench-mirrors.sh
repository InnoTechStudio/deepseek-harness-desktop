#!/usr/bin/env bash
# Benchmark candidate download mirrors for the Node runtime and the npm registry.
# Usage: bash scripts/bench-mirrors.sh [label]
# Writes a TSV report to /tmp/mirror-bench-<label>.tsv and prints a ranked summary.
set -uo pipefail

LABEL="${1:-unknown}"
NODE_VER="v22.22.0"
NODE_FILE="node-${NODE_VER}-darwin-arm64.tar.gz"
PKG_ENC="%40deepseek-ai%2fdsh"
SLICE=$((3 * 1024 * 1024)) # 3 MiB probe slice
TIMEOUT=12
OUT="/tmp/mirror-bench-${LABEL}.tsv"

# name<TAB>node_binary_url
NODE_MIRRORS=$(
  cat <<EOF
huawei-repo	https://repo.huaweicloud.com/nodejs/${NODE_VER}/${NODE_FILE}
huawei-mirrors	https://mirrors.huaweicloud.com/nodejs/${NODE_VER}/${NODE_FILE}
tencent	https://mirrors.cloud.tencent.com/nodejs-release/${NODE_VER}/${NODE_FILE}
npmmirror	https://cdn.npmmirror.com/binaries/node/${NODE_VER}/${NODE_FILE}
aliyun	https://mirrors.aliyun.com/nodejs-release/${NODE_VER}/${NODE_FILE}
ustc	https://mirrors.ustc.edu.cn/node/${NODE_VER}/${NODE_FILE}
tuna	https://mirrors.tuna.tsinghua.edu.cn/nodejs-release/${NODE_VER}/${NODE_FILE}
nju	https://mirror.nju.edu.cn/nodejs-release/${NODE_VER}/${NODE_FILE}
sjtu	https://mirror.sjtu.edu.cn/nodejs-release/${NODE_VER}/${NODE_FILE}
zju	https://mirrors.zju.edu.cn/nodejs-release/${NODE_VER}/${NODE_FILE}
bfsu	https://mirrors.bfsu.edu.cn/nodejs-release/${NODE_VER}/${NODE_FILE}
hust	https://mirrors.hust.edu.cn/nodejs-release/${NODE_VER}/${NODE_FILE}
nanjing-edu	https://mirrors.nju.edu.cn/nodejs-release/${NODE_VER}/${NODE_FILE}
official	https://nodejs.org/dist/${NODE_VER}/${NODE_FILE}
EOF
)

# name<TAB>registry_metadata_url
NPM_MIRRORS=$(
  cat <<EOF
huawei	https://repo.huaweicloud.com/repository/npm/${PKG_ENC}
npmmirror	https://registry.npmmirror.com/${PKG_ENC}
tencent	https://mirrors.cloud.tencent.com/npm/${PKG_ENC}
ustc	https://mirrors.ustc.edu.cn/npm/${PKG_ENC}
npmjs	https://registry.npmjs.org/${PKG_ENC}
EOF
)

printf 'label\tkind\tname\thost\thttp\tspeed_bps\tbytes\ttime_s\tttfb_s\n' >"$OUT"

probe() {
  local kind="$1" name="$2" url="$3" extra="$4"
  local host
  host=$(printf '%s' "$url" | cut -d/ -f3)
  local res
  # shellcheck disable=SC2086
  res=$(curl -sL -o /dev/null \
    -w '%{http_code}\t%{speed_download}\t%{size_download}\t%{time_total}\t%{time_starttransfer}' \
    --max-time "$TIMEOUT" $extra "$url" 2>/dev/null) || res="000\t0\t0\t${TIMEOUT}\t${TIMEOUT}"
  printf '%s\t%s\t%s\t%s\t%s\n' "$LABEL" "$kind" "$name" "$host" "$res" >>"$OUT"
  printf '  %-14s %-32s %s\n' "$name" "$host" "$(printf '%s' "$res" | awk -F'\t' '{printf "http=%s %8.0f KB/s %6.2fs ttfb=%.2fs", $1, $2/1024, $4, $5}')"
}

echo "=== [$LABEL] Node binary throughput (3 MiB range slice, ${TIMEOUT}s cap)"
while IFS=$'\t' read -r name url; do
  [ -z "$name" ] && continue
  probe node "$name" "$url" "-r 0-$((SLICE - 1))"
done <<<"$NODE_MIRRORS"

echo
echo "=== [$LABEL] npm registry metadata latency"
while IFS=$'\t' read -r name url; do
  [ -z "$name" ] && continue
  probe npm "$name" "$url" ""
done <<<"$NPM_MIRRORS"

echo
echo "=== [$LABEL] ranking (usable node mirrors, fastest first)"
awk -F'\t' 'NR>1 && $2=="node" && ($5==200||$5==206) && $6>0 {printf "%10.0f KB/s  %-14s %s\n", $6/1024, $3, $4}' "$OUT" | sort -rn

echo
echo "report: $OUT"
