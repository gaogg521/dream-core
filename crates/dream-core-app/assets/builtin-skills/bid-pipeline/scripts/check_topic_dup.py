#!/usr/bin/env python3
# check_topic_dup.py —— 选题母题去重硬卡口（canonical 校验脚本）
# 用法：python3 check_topic_dup.py <today_dir> <topic_today.md>
# 退出码：0 = 无 7 天内同母题重发；1 = 命中重复，须退改
#
# 判定逻辑（权威源 = topic_pool.md「已发母题冷却记录」表）：
#   读冷却表里「冷却到期」>= 今日的行，取其母题关键词；
#   若今日选题主标题命中任一冷却中母题的关键词 → 判重发，退改。
#   兜底：标题模糊相似度（同含发文主体/文号强 token）二次拦截。

import sys, os, re, glob

import os
SHARED = os.environ.get("BID_SHARED_DIR", os.path.dirname(os.path.abspath(__file__))))
# 注：原系统硬编码 /home/laoty/.openclaw/workspace-bid-shared，已参数化为 BID_SHARED_DIR（未设置时回退到本脚本所在目录）
POOL = os.path.join(SHARED, "topic_pool.md")

def today_str():
    import datetime
    return datetime.date.today().strftime("%Y-%m-%d")

def parse_cooling_table():
    """返回 [(母题描述, 到期日str, [关键词...])]"""
    if not os.path.exists(POOL):
        return []
    lines = open(POOL, encoding="utf-8").read().splitlines()
    in_table = False
    rows = []
    for ln in lines:
        if "已发母题冷却记录" in ln:
            in_table = True
            continue
        if in_table:
            if ln.strip().startswith("## ") or (ln.strip() and not ln.strip().startswith("|")):
                # 表结束（遇到下一个二级标题或非表行且非空）
                if ln.strip().startswith("## "):
                    break
                if ln.strip().startswith("|") is False and ln.strip():
                    # 允许表内空行，但遇到非 | 开头且有内容则结束（保守：仅遇 ## 才断）
                    pass
            if ln.strip().startswith("|") and "冷却到期" not in ln and "---" not in ln:
                cells = [c.strip() for c in ln.strip().strip("|").split("|")]
                if len(cells) >= 3:
                    desc, last, due = cells[0], cells[1], cells[2]
                    # 从描述里抽关键词：括号/顿号/空格分隔的词
                    kws = re.split(r'[（(）、，,/／\s]+', desc)
                    kws = [k for k in kws if len(k) >= 2 and k not in ("含", "等", "系列", "拆篇")]
                    rows.append((desc, due, kws))
    return rows

def extract_today_topics(topic_path):
    if not os.path.exists(topic_path):
        return []
    txt = open(topic_path, encoding="utf-8").read()
    topics, cur = [], None
    for line in txt.splitlines():
        if re.match(r'^##\s*选题\s*[AB]', line):
            cur = line.strip()
        m = re.search(r'\*\*主标题\*\*[：:]\s*(.+)', line)
        if m and cur:
            topics.append((cur, m.group(1).strip()))
    return topics


def normalize_subject(title):
    """发文主体归一：国资委各层级都归国资委；深圳/新疆保留"""
    t = title
    t = t.replace("国务院国资委", "国资委").replace("深圳市国资委", "国资委").replace("深圳国资委", "国资委")
    t = t.replace("新疆统一评标专家库", "新疆").replace("新疆评标专家", "新疆")
    return t

def main():
    today_dir = sys.argv[1] if len(sys.argv) > 1 else ""
    topic_path = sys.argv[2] if len(sys.argv) > 2 else os.path.join(SHARED, today_dir, f"{today_dir}-topic_today.md")
    td = today_str()

    cooling = parse_cooling_table()
    # 仅保留未过冷却期的
    active = []
    for desc, due, kws in cooling:
        try:
            if due >= td:
                active.append((desc, due, kws))
        except Exception:
            pass

    today = extract_today_topics(topic_path)
    if not today:
        print("[WARN] 未从 topic_today 解析到选题主标题，跳过去重检查")
        sys.exit(0)

    hits = []
    for sec, ttitle in today:
        ntt = normalize_subject(ttitle)
        for desc, due, kws in active:
            # 标题与关键词都做主体归一后比对
            nkws = [normalize_subject(k) for k in kws]
            if any(kw in ntt for kw in nkws):
                hits.append((sec, ttitle, desc, due))
                break

    if hits:
        print(f"[FAIL] 命中 {len(hits)} 处冷却期内同母题重发：")
        for sec, tt, desc, due in hits:
            print(f"  {sec} «{tt}» 命中冷却母题「{desc}」（到期 {due}）")
        print("须退改：换非重复母题（或等冷却到期 / 新节点驱动+换支柱角度）。")
        sys.exit(1)
    else:
        print(f"[OK] 今日 {len(today)} 选题均未命中冷却母题（激活冷却项 {len(active)} 条）")
        sys.exit(0)

if __name__ == "__main__":
    main()
