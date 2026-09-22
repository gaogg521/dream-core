#!/usr/bin/env bash
# check_ai_taste.sh · 反 AI 味自检（对齐 AGENTS.md「质量闸门」第5条）
# 用法： bash check_ai_taste.sh <运行日目录>  例： bash check_ai_taste.sh 20260813
# 退出码：0=通过 1=命中套路句式（不合格，需回炉重写散文）
set -u
DIR="${1:-.}"
PAT='写在最后|写在前面|结语|总结一下|总的来说|一句话总结|划重点|现在能做的[0-9一二三四五]?件事|今天就能做的几件事|3 个动作|行动清单|记住这几点|需要注意的是|值得一提的是|综上所述|不可否认'
FOUND=0
echo "== 反 AI 味扫描: $DIR =="
for f in "$DIR"/*.md "$DIR"/*.html; do
  [ -f "$f" ] || continue
  # 跳过纯配置/清单类文件（meta/publish/checklist），只扫内容产物
  case "$(basename "$f")" in
    meta.json|publish_log.json|publish_checklist.md|review*.md|topic_today.md) continue ;;
  esac
  hits=$(grep -oE "$PAT" "$f" 2>/dev/null | sort | uniq -c)
  if [ -n "$hits" ]; then
    FOUND=1
    echo "  [命中] $(basename "$f"):"
    echo "$hits" | sed 's/^/      /'
  fi
done
if [ "$FOUND" -eq 0 ]; then
  echo "✅ 反 AI 味自检通过：未发现套路句式"
  exit 0
else
  echo "❌ 不合格：命中反 AI 味套路句式，须回炉重写散文（reviewer 在 Phase 4 已应先卡此关）"
  exit 1
fi
