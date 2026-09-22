#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""对外发布物「外链/联系方式」违规核查（canonical 校验脚本）。
对齐各平台「禁止站外引流」规则：正文不得含外部链接、裸域名、电话、微信号、QQ、邮箱。
仅查对外发布物（YYYYMMDD/ 下的五平台 md + 纯文本版），内部工作台文件（expert_db_news/policy_tracker/topic_today/review）不查。
退出码：0=通过，1=命中违规。
用法：python3 check_no_leak.py [运行日目录]   # 不传则查全部 20260* 目录
"""
import os, re, sys, glob

# 违规模式：裸域名（含 .com/.cn/.gov/.org/.net 且无中文包围）、http(s) 链接、电话、微信/QQ/邮箱
PATTERNS = [
    (r"https?://[^\s）)】]+", "外部链接(http/https)"),
    (r"\b[a-z0-9\-]+\.(com|cn|gov|org|net|edu)(\.[a-z]{2})?\b", "裸域名"),
    (r"(?<!\d)(0\d{2,3}[\-—]?\d{7,8})(?!\d)", "固定电话"),
    (r"\b1[3-9]\d{9}\b", "手机号"),
    (r"[a-z0-9_.+-]+@[a-z0-9-]+\.[a-z]{2,}", "邮箱"),
    (r"(微信|微信号|加微|vx|VX|V信)[\s：:]*[a-zA-Z0-9_\-]{4,}", "微信号"),
    (r"\b[Qq][Qq]?[\s：:]*[0-9]{5,}\b", "QQ号"),
]

INTERNAL_KEYWORDS = ("expert_db_news", "policy_tracker", "topic_today", "review", "审核", "可直接复制粘贴版")

def scan_file(path):
    hits = []
    for i, line in enumerate(open(path, encoding="utf-8"), 1):
        for pat, label in PATTERNS:
            for m in re.finditer(pat, line):
                # 裸域名豁免：出现在中文句子里且被中文包围（如「xx.gov.cn 网站」）仍算违规，但内部文件不查
                hits.append((i, label, m.group(0).strip(), line.strip()[:60]))
    return hits

def main():
    if len(sys.argv) > 1:
        dirs = [sys.argv[1]] if os.path.isdir(sys.argv[1]) else []
    else:
        dirs = sorted(glob.glob(os.path.join(os.path.dirname(__file__), "20260*")))

    total_hits = 0
    for d in dirs:
        if not os.path.isdir(d):
            continue
        for fn in sorted(os.listdir(d)):
            if not fn.endswith((".md", ".txt")):
                continue
            if any(k in fn for k in INTERNAL_KEYWORDS):
                continue
            fp = os.path.join(d, fn)
            hits = scan_file(fp)
            if hits:
                total_hits += len(hits)
                print(f"[违规] {fp}")
                for i, label, val, ctx in hits:
                    print(f"  L{i} {label}: 「{val}」  …{ctx}")
    if total_hits:
        print(f"\n❌ 共 {total_hits} 处外链/联系方式违规")
        sys.exit(1)
    print("✅ 无外链/联系方式违规")
    sys.exit(0)

if __name__ == "__main__":
    main()
