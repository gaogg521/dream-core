#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""跟进报告：对比待办状态，生成跟进表。
用法：
  python3 followup-report.py < todos.csv
todos.csv 每行：事项|负责人|截止日期|状态(完成/进行中/逾期/阻塞)|备注
"""
import sys
import csv

def report(rows):
    status = {"完成": 0, "进行中": 0, "逾期": 0, "阻塞": 0}
    out = []
    out.append("【待办跟进报告】")
    out.append("-" * 30)
    for r in rows:
        item = r[0] if len(r) > 0 else ""
        owner = r[1] if len(r) > 1 else ""
        due = r[2] if len(r) > 2 else ""
        st = r[3] if len(r) > 3 else ""
        note = r[4] if len(r) > 4 else ""
        status[st] = status.get(st, 0) + 1
        mark = {"完成": "[x]", "进行中": "[~]", "逾期": "[!]", "阻塞": "[#]"}.get(st, "[ ]")
        line = "%s %s | 负责人:%s | 截止:%s" % (mark, item, owner or "-", due or "-")
        if note:
            line += " | %s" % note
        out.append(line)
    out.append("-" * 30)
    total = len(rows)
    closed = status["完成"]
    if total:
        out.append("闭环率：%d%%（%d/%d）" % (round(closed * 100 / total), closed, total))
        out.append("逾期 %d 条，阻塞 %d 条" % (status["逾期"], status["阻塞"]))
    else:
        out.append("（无待办记录）")
    return "\n".join(out)

def main():
    rows = []
    for raw in sys.stdin:
        raw = raw.rstrip("\n")
        if not raw.strip():
            continue
        parts = [p.strip() for p in raw.split("|")]
        rows.append(parts)
    print(report(rows))

if __name__ == "__main__":
    main()
