#!/usr/bin/env bash
# Hermes Agent Migrator — macOS 构建入口(cargo tauri build 的智能封装)
#
# 背景(2026-09-30 修复):Tauri 内置 bundle_dmg.sh 会用 osascript 驱动
# Finder 摆 DMG 图标布局。在 headless 环境(Agent/SSH/无 GUI 会话,或
# "有 Aqua 会话但 AppleEvent 无响应"的场景)里 Finder 不响应 →
# osascript 超时 -1712 → 脚本 exit 64 → cargo tauri build 报
# "error running bundle_dmg.sh"。GitHub CI 自带 CI=true 会自动加
# --skip-jenkins 跳过美化,故只有本机这类环境挂。
#
# 用法:
#   ./build_macos.sh               # 自动探测:Finder AppleEvent 可用 → 保留布局;不可用 → 自动 CI=true
#   FORCE_GUI=1 ./build_macos.sh   # 强制走 Finder 摆位(不可用时必失败,仅限桌面终端)
#   其余参数透传给 cargo tauri build(如 --bundles dmg)
#
# 说明:CI=true 时 tauri-bundler 给 bundle_dmg.sh 加 --skip-jenkins,
# 产出的 DMG 内容完全一致,只是没有自定义图标摆位/背景(可接受)。
set -euo pipefail
cd "$(dirname "$0")"   # gui/

if [ -n "${FORCE_GUI:-}" ]; then
  echo "[build] FORCE_GUI=1 → 强制使用 Finder 布局(需要 Finder AppleEvent 可用,否则 AppleScript 超时会失败)"
  exec cargo tauri build "$@"
fi

if [ -z "${CI:-}" ]; then
  # 限时(12s)跑一条轻量 Finder AppleEvent 做探针;失败/超时 → 该环境
  # AppleScript 不可靠,统一交给 Tauri 的 --skip-jenkins 路径。
  if perl -e 'alarm 12; exec @ARGV' osascript \
      -e 'tell application "Finder" to count of disks' >/dev/null 2>&1; then
    echo "[build] Finder AppleEvent 探针 OK → 保留 DMG 图标摆位"
  else
    echo "[build] Finder AppleEvent 探针不可用(headless/后台/AppleEvent 无响应)"
    echo "        自动 CI=true,让 Tauri --skip-jenkins(DMG 内容不变,仅跳过图标摆位)"
    export CI=true
  fi
fi

exec cargo tauri build "$@"
