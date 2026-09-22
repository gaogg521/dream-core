#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""微信公众号 / 今日头条 双深度 + 方向差异化 校验（canonical 校验脚本）。
对齐主编 2026-08-19 要求（2026-08-20 升级为深度优先）：两平台须产出深度文，1200 字仅为合规下限（低于即不合格），目标 1500–1800 字；1200–1499 压线短文给 WARN 提示（reviewer 据 AGENTS.md 质量闸门第 8 条判退改），且同主题两版不得相互搬运/改标题重发。
量化指标：
  1) 字数：微信 md + 头条 md 各自去除空白/标题块后，<1200 字符不合格（深度文最低红线）；1200–1499 为压线短文给 WARN（建议重写到 1500+）；目标 1500–1800 字。
  2) 差异化：两版正文文本相似度（difflib）须 < 0.6（即共同内容占比不过半），避免改标题重发
用法：python3 check_wx_tt_depth.py <运行日目录>   # 查该日全部选题的微信/头条对
退出码：0=通过，1=不达标。
"""
import os, re, sys, glob, difflib

MIN_CHARS = 1200      # 合规下限（低于即不合格）
TARGET_CHARS = 1500   # 深度文目标区间下限（1200–1499 为压线短文，WARN）
MAX_SIMILARITY = 0.60  # 相似度上限，超过即判"改标题重发"

def read_body(path):
    """读 md，去 # 标题块/发布提示块、去空行，返回纯正文。"""
    txt = open(path, encoding="utf-8").read()
    # 去掉 # 开头的块标识行（保留正文）
    lines = []
    for ln in txt.splitlines():
        s = ln.strip()
        if not s:
            continue
        if s.startswith("#"):
            continue
        lines.append(s)
    return "".join(lines)

def count_chars(body):
    # 中文+标点+英文均计字符（strip 空白）
    return len(re.sub(r"\s", "", body))

def similarity(a, b):
    return difflib.SequenceMatcher(None, a, b).ratio()

def main():
    if len(sys.argv) > 1:
        d = sys.argv[1]
        dirs = [d] if os.path.isdir(d) else []
    else:
        dirs = sorted(glob.glob(os.path.join(os.path.dirname(__file__), "20260*")))

    total_issues = 0
    for d in dirs:
        if not os.path.isdir(d):
            continue
        # 收集该日所有选题的微信/头条 md
        wechat = {}  # theme -> path
        toutiao = {}
        for fn in os.listdir(d):
            if not fn.endswith(".md"):
                continue
            m = re.match(r"(\d{8})-微信公众号-(.*?)\.md$", fn)
            if m:
                wechat[m.group(2)] = os.path.join(d, fn)
            m = re.match(r"(\d{8})-今日头条-(.*?)\.md$", fn)
            if m:
                toutiao[m.group(2)] = os.path.join(d, fn)
        # 按主题对齐（微信/头条主题应一致）
        themes = set(wechat) | set(toutiao)
        for theme in sorted(themes):
            wc = wechat.get(theme)
            tt = toutiao.get(theme)
            if not wc or not tt:
                # 缺一个平台不算错（可能该选题未发该平台），跳过
                continue
            wb = read_body(wc)
            tb = read_body(tt)
            wn, tn = count_chars(wb), count_chars(tb)
            sim = similarity(wb, tb)
            issues = []
            warn = []
            if wn < MIN_CHARS:
                issues.append(f"微信字数 {wn}<{MIN_CHARS}（低于合规下限）")
            elif wn < TARGET_CHARS:
                warn.append(f"微信字数 {wn} 为压线短文（建议写到 {TARGET_CHARS}+）")
            if tn < MIN_CHARS:
                issues.append(f"头条字数 {tn}<{MIN_CHARS}（低于合规下限）")
            elif tn < TARGET_CHARS:
                warn.append(f"头条字数 {tn} 为压线短文（建议写到 {TARGET_CHARS}+）")
            if sim >= MAX_SIMILARITY:
                issues.append(f"两版相似度 {sim:.2f}≥{MAX_SIMILARITY}（疑似改标题重发）")
            if issues:
                total_issues += len(issues)
                print(f"[不达标] {theme}")
                for x in issues:
                    print(f"  - {x}")
            else:
                print(f"[OK] {theme}  微信{wn}字 / 头条{tn}字 / 相似度{sim:.2f}")
                for x in warn:
                    print(f"  ⚠ {x}")

    if total_issues:
        print(f"\n❌ 共 {total_issues} 项微信/头条双深度或差异化不达标")
        sys.exit(1)
    print("\n✅ 微信/头条双深度 + 方向差异化全部达标")
    sys.exit(0)

if __name__ == "__main__":
    main()
