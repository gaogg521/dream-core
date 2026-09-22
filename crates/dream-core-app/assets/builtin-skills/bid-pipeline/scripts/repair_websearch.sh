#!/usr/bin/env bash
# canonical: web_search provider 自愈脚本（2026-08-20 03:00 修正）
# 关键认知（2026-08-20 实测）：
# - 本环境是 QClaw 桌面 App，gateway 由 App 拉起；App 硬编码默认 provider=yuanbao
#   并在每次重拉 gateway 时写回磁盘。yuanbao 在 App 环境下**正常工作**（走腾讯元宝
#   搜索通道，返回 qq.com/企鹅号聚合源，实测可用、返回 10 条真实结果）。
# - 之前误判 yuanbao 为非法 provider（基于老版本/错误假设），实测确认可用。
# - duckduckgo / parallel-free / ddgs 在本环境均不可用：DDG 出网超时(HTTP=000)、
#   parallel-free 需 key、ddgs 库底层也是 DDG。baidu 出网正常但无 baidu provider。
# - 因此：本脚本**不再强制改 provider**（避免与 App 写回 yuanbao 打架导致磁盘抖动），
#   只做旁观式健康检查：若磁盘 provider 非 yuanbao，记日志提示（不自动改回）。
# 由 bid_selfcheck（*/15 * * * *）调用做轻量巡检。radar 主通道仍是 curl 直连官方域名。
set -e
CONF="${QCLAW_CONF:-/home/laoty/.qclaw/openclaw.json}"
LOG="${BID_SHARED_DIR:-.}/web_search_repair.log"
NOW=$(date '+%Y-%m-%d %H:%M:%S')
EXPECTED=yuanbao

# 1) 旁观式健康检查：仅记录，不强制改回
cur=$(python3 -c "import json;print(json.load(open('$CONF'))['tools']['web']['search']['provider'])" 2>/dev/null || echo "err")
if [ "$cur" = "$EXPECTED" ]; then
  echo "[$NOW] provider=$cur (OK, App 环境正常通道)" >> "$LOG"
else
  echo "[$NOW] provider=$cur (非 $EXPECTED；App 会自行管理，不自动改回以避免磁盘抖动)" >> "$LOG"
fi

# 2) 向 gateway 发 SIGUSR1（勿杀进程，避免 App 重拉写回）；仅作健康检查信号
GW=$(ps aux | grep openclaw-gateway | grep -v grep | awk '{print $2}' | head -1)
if [ -n "$GW" ]; then
  kill -USR1 "$GW" 2>/dev/null || true
  echo "[$NOW] SIGUSR1 sent to gateway pid=$GW (health check signal)" >> "$LOG"
fi
echo "done: provider=$cur (observed)"
