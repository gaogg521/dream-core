#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""会议纪要→待办：把会议记录文本拆成结构化纪要 + 待办清单。
用法：
  python3 minutes-to-todo.py < notes.txt
识别规则（启发式，仅供参考）：
  - 含"待办/负责/跟进/确认/需要/安排/推动/落地/完成"等 → 待办
  - 含"决定/确认/达成/拍板/同意" → 决策
  - 其余 → 讨论/信息
"""
import sys
import re

ACTION_WORDS = ["待办", "负责", "跟进", "确认", "需要", "安排", "推动", "落地", "完成", "别忘了"]
DECISION_WORDS = ["决定", "确认了", "达成", "拍板", "同意", "定了", "通过"]

def classify(line):
    if any(w in line for w in DECISION_WORDS) and not any(w in line for w in ACTION_WORDS):
        return "决策"
    if any(w in line for w in ACTION_WORDS):
        return "待办"
    return "讨论"

def extract(notes):
    todos, decisions, talks = [], [], []
    for ln in notes:
        s = ln.strip()
        if not s:
            continue
        c = classify(s)
        if c == "待办":
            todos.append(s)
        elif c == "决策":
            decisions.append(s)
        else:
            talks.append(s)
    out = []
    out.append("【会议纪要结构化】")
    out.append("◆ 决策")
    for d in decisions:
        out.append("- " + d)
    out.append("◆ 待办清单")
    for t in todos:
        out.append("- [ ] " + t)
    out.append("◆ 讨论/信息")
    for t in talks:
        out.append("- " + t)
    out.append("=" * 30)
    out.append("待办 %d 条，决策 %d 条，讨论 %d 条" % (len(todos), len(decisions), len(talks)))
    out.append("提示：请为每条待办补充【负责人 + 截止时间 + 优先级】。")
    return "\n".join(out)

def main():
    notes = sys.stdin.read().splitlines()
    print(extract(notes))

if __name__ == "__main__":
    main()
