#!/usr/bin/env bash
# Echo desktop player — 一键本地开发启动脚本 (项目根目录运行)
#
#   ./dev.sh build   # 只构建、不启动 (构建前端 dist + 编译 Rust)
#   ./dev.sh         # 完整启动: 安装→构建→生成 IPC→tauri dev
#   ./dev.sh skip-install  # 跳过 pnpm install (依赖已装时更快)
#
# 说明: 本项目的 `tauri dev` 加载的是 apps/desktop/dist 的已构建前端
# (tauri.conf.json 未配置 beforeDevCommand/devUrl), 所以每次都要先 build。

set -euo pipefail
cd "$(dirname "$0")"

BUILD_ONLY=0
SKIP_INSTALL=0
case "${1:-}" in
  build)        BUILD_ONLY=1 ;;
  skip-install) SKIP_INSTALL=1 ;;
  ""|*)         ;;
esac

step() { printf "\n\033[1;36m==> %s\033[0m\n" "$*"; }

# ── 依赖: pnpm + Rust toolchain ───────────────────────────────────────────
for bin in pnpm cargo; do
  if ! command -v "$bin" >/dev/null 2>&1; then
    echo "缺少依赖: $bin — 请先安装 (macOS: brew install pnpm rustup)" >&2
    exit 1
  fi
done

# ── 前端依赖 ───────────────────────────────────────────────────────────────
if [ "$SKIP_INSTALL" -eq 0 ]; then
  step "pnpm install"
  pnpm install
fi

# ── 生成 IPC TypeScript 类型 (需要先编译 echo-desktop 的 generator bin) ──
step "生成 IPC TS 类型 (echo-generate-ipc)"
pnpm --dir apps/desktop generate:ipc

# ── 构建前端 ────────────────────────────────────────────────────────────────
step "构建前端 (tsc + vite → apps/desktop/dist)"
pnpm --dir apps/desktop build

if [ "$BUILD_ONLY" -eq 1 ]; then
  step "构建完成。启动请运行: ./dev.sh"
  exit 0
fi

# ── 启动 Tauri 开发窗口 ─────────────────────────────────────────────────────
step "pnpm tauri dev"
pnpm --dir apps/desktop tauri dev