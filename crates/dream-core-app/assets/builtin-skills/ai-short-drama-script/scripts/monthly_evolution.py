#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""AI短剧剧本 · 月度自动进化引擎 V2（带增强联网核验任务清单）
用法:
  python monthly_evolution.py --check            # 时效审计（每月1号 0-1 点执行）
  python monthly_evolution.py --tasks            # 输出本月待联网核验清单（关键词组 + 目标文件）
  python monthly_evolution.py --check --tasks    # 组合：审计 + 落后时打印任务清单
  python monthly_evolution.py --bump YYYY-MM     # 更新知识截止（写入 changelog 条目骨架 + 各 references 头部）
流程: --check（落后）→ --tasks 取关键词 → 由 AI 执行 WebSearch 联网核验 → 人工/AI 更新 references
      → --bump 写入新版本标记 → 重跑 --check 确认「时效已同步」
零依赖，Python 3.8+。
"""
import argparse, datetime, os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.join(HERE, "..")
CHANGELOG = os.path.join(ROOT, "references", "changelog.md")
REFS = ["script-formulas.md", "script-format-standard.md", "platform-policy-2026.md",
        "character-dialogue-craft.md", "ai-video-pipeline.md", "red-line-checklist.md",
        "episode-template.md"]

TASKS = [
    ("平台政策", "platform-policy-2026.md",
     ["2026 微短剧 分账新规 抖音 红果 腾讯视频 爱奇艺",
      "AI微短剧 分类分层 审核 AI生成标识 溯源材料 最新",
      "红果短剧 剧本投稿 限制 保底 扶持 最新公告"]),
    ("完播与收益数据", "script-formulas.md",
     ["短剧 完播率 有效播放 数据 2026 最新",
      "微短剧 万播单价 分账 收益 行业数据"]),
    ("爆款结构公式", "script-formulas.md",
     ["AI短剧 爆款 剧本结构 Hook 悬念 技巧 最新",
      "短剧 题材 排行 热门 逆袭 甜宠 悬疑 最新趋势"]),
    ("剧本格式与投稿标准", "script-format-standard.md",
     ["短剧剧本 格式规范 投稿要求 过稿标准 最新",
      "AI漫剧 分镜脚本 格式规范 v版 更新"]),
    ("AI视频工具与角色一致性", "ai-video-pipeline.md",
     ["即梦 Seedance 可灵 小云雀 版本 参数 更新 2026",
      "AI短剧 角色一致性 方案 定妆照 外貌锚点 最新"]),
    ("合规红线", "red-line-checklist.md",
     ["广电总局 微短剧 管理提示 新规 最新",
      "微短剧 内容审核 违规 下架 案例 最新"]),
]


def latest_cutoff():
    try:
        dates = re.findall(r"(\d{4}-\d{2})-\d{2}", open(CHANGELOG, encoding="utf-8").read())
        return max(dates) if dates else "0000-00"
    except FileNotFoundError:
        return None


def bump(month):
    entry = (
        f"\n## vN（{month} 月度进化）\n"
        f"- 联网核验日期：{datetime.date.today()}（关键词组见 monthly_evolution.py --tasks）\n"
        f"- 本轮更新：\n"
        f"  - [ ] platform-policy-2026.md（平台分账/审核/投稿规则）\n"
        f"  - [ ] script-formulas.md（爆款公式/完播口径）\n"
        f"  - [ ] script-format-standard.md（格式与投稿标准）\n"
        f"  - [ ] ai-video-pipeline.md（AI 分镜与角色一致性）\n"
        f"  - [ ] character-dialogue-craft.md（人物/台词技法）\n"
        f"  - [ ] red-line-checklist.md（合规红线）\n"
        f"- 输出能力提升点：\n"
    )
    with open(CHANGELOG, "a", encoding="utf-8") as f:
        f.write(entry)
    for name in REFS:
        p = os.path.join(ROOT, "references", name)
        if not os.path.exists(p):
            continue
        txt = open(p, encoding="utf-8").read()
        txt = re.sub(r"知识截止：\d{4}-\d{2}", f"知识截止：{month}", txt, count=1)
        txt = re.sub(r"> 知识截止：\d{4}-\d{2}", f"> 知识截止：{month}", txt, count=1)
        open(p, "w", encoding="utf-8").write(txt)
    print(f"[bump] changelog 已追加 {month} 条目骨架；各 references 知识截止已更新为 {month}。")
    print("[bump] 下一步：补全条目内容后重跑 --check 确认「时效已同步」。")


def print_tasks():
    print(f"# 月度联网核验任务清单（{datetime.date.today()}）")
    print("执行方式：对每条关键词组使用 WebSearch 检索 → 比对 references 现状 → 更新 → changelog 记录。\n")
    for i, (area, target, kws) in enumerate(TASKS, 1):
        print(f"{i}. 【{area}】→ references/{target}")
        for kw in kws:
            print(f"   - 检索：{kw}")
    print("\n核验优先级：政策类 > 完播收益数据 > 格式投稿标准 > 爆款公式 > AI 工具 > 合规红线。")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--tasks", action="store_true")
    ap.add_argument("--bump", nargs="?", const="", default=None)
    a = ap.parse_args()

    if a.bump is not None:
        now = datetime.date.today()
        bump(a.bump or f"{now.year:04d}-{now.month:02d}")
        return
    if a.tasks and not a.check:
        print_tasks()
        return

    now = datetime.date.today()
    cur = f"{now.year:04d}-{now.month:02d}"
    cut = latest_cutoff()
    if cut is None:
        print("[FAIL] 未找到 references/changelog.md，资源库缺失，请重建。")
        sys.exit(1)
    lag = (now.year - int(cut[:4])) * 12 + (now.month - int(cut[5:7]))
    print(f"# AI短剧剧本 资源库时效审计（{now}）")
    print(f"- 当前月份：{cur}")
    print(f"- changelog 最新知识截止：{cut}")
    if lag == 0:
        print("- 审计结论：✅ 时效已同步")
        if a.tasks:
            print()
            print_tasks()
        return
    print(f"- 审计结论：⚠️ 落后 {lag} 个月")
    print("- 进化流程：--tasks 取核验清单 → WebSearch 逐条核验 → 更新 references → --bump → 重跑 --check")
    if a.tasks:
        print()
        print_tasks()
    sys.exit(1)


if __name__ == "__main__":
    main()
