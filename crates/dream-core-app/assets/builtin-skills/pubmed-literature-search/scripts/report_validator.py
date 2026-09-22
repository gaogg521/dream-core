#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
report_validator.py — pubmed搜文献助手 报告质量验证器（独立版，零第三方依赖）
用法: python report_validator.py <report.html> [--ifmap journal_if_map.json]
退出码 0 = 通过(可交付); 非0 = 存在红线/门禁失败(❌ 打印原因)

本验证器为「关键词文献报告」专用，与学术人才背调团及其验证器互不依赖：
  - 无 __ME__/候选人强制检查（客户指定追踪作者时才校验一致性）
  - IF 防编造：报告内所有 ifv 必须与本包 journal_if_map.json 精确命中一致
"""
import json
import os
import re
import sys

# ---- 净化词(冗余客套/不确定表述, 0 命中才可通过) ----
PURGE_WORDS = [
    "综上所述", "值得注意的是", "此外", "另外", "总体而言", "不难看出", "由此可见",
    "需要指出的是", "值得一提的是", "不难发现", "客观上", "主观上",
    "一定程度上", "简而言之", "归根到底", "一言以蔽之", "毫不意外", "顺带一提",
    "话说回来", "据不完全统计", "可能约为", "预计大约",
]
# ---- 禁用 CSS 变量(旧模板残留) ----
FORBIDDEN_CSS_VARS = ["--ink", "--sub", "--brand"]
# ---- IF/来源 红线词(禁用估算类表述) ----
RED_LINE_WORDS = ["OpenAlex", "IF≈", "篇均被引", "据公开资料推测", "网传", "疑似", "IF约", "影响因子约"]

CJK = re.compile(r"[\u4e00-\u9fff]")


def load_ifmap(path):
    """加载 IF 表, 兼容两种格式(与 pubmed_search.load_ifmap 保持一致):
    - 旧: {name: {field: value, ...}}
    - 新(体积优化): {"_fields": [...], name: [v1, ...]} → 还原为字典
    """
    if not os.path.exists(path):
        return None
    with open(path, "r", encoding="utf-8-sig") as f:  # utf-8-sig 兼容带/不带 BOM
        raw = json.load(f)
    if not raw:
        return raw
    sample = next(iter(raw.values()))
    if isinstance(sample, dict):
        return raw
    fields = raw.get("_fields") or []
    return {k: dict(zip(fields, v)) for k, v in raw.items()
            if k != "_fields" and isinstance(v, list)}


_NORM_IDX = {}


def _norm(s):
    """规范化: 大写、&→AND、去非字母数字。消除逗号/撇号/连字符/AND vs & 差异。"""
    return re.sub(r"[^A-Z0-9]", "", s.upper().replace("&", " AND "))


def _if_norm_index(ifmap):
    """IF 表规范化索引(惰性构建, 仅用于查表回退, 不影响防编造判定)。"""
    if not _NORM_IDX:
        for k, v in ifmap.items():
            _NORM_IDX.setdefault(_norm(k), v)
    return _NORM_IDX


def check(path, ifmap):
    errors = []
    if not os.path.exists(path):
        return ["❌ 文件不存在: " + path]
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        html = f.read()

    # 1) 基本结构闭合
    if not html.strip().lower().startswith("<!doctype html"):
        errors.append("✗ 缺少 <!DOCTYPE html> 声明")
    for tag in ["html", "head", "body", "style"]:
        o = len(re.findall(r"<%s[ >]" % tag, html, re.I))
        c = len(re.findall(r"</%s>" % tag, html, re.I))
        if o != c:
            errors.append("✗ <%s> 开合不平衡 (开%d/合%d)" % (tag, o, c))

    # 2) body 纯净(无属性)
    if re.search(r"<body[^>]", html, re.I):
        errors.append("✗ <body> 带属性, 须为纯 <body>")

    # 3) 禁用 CSS 变量
    for v in FORBIDDEN_CSS_VARS:
        if v in html:
            errors.append("✗ 使用了禁用 CSS 变量 %s" % v)

    # 4) 净化词 0 命中
    for w in PURGE_WORDS:
        if re.search(w, html):
            errors.append("✗ 命中净化词: %s" % w)

    # 5) 红线词 0 命中
    for w in RED_LINE_WORDS:
        if w in html:
            errors.append("✗ 命中红线词: %s" % w)

    # 6) DATA 数据逐篇校验
    m_data = re.search(r"const DATA\s*=\s*(\[.*?\]);", html, re.S)
    if not m_data:
        errors.append("✗ 未找到 const DATA=[...] 数据块")
        return errors
    try:
        arr = json.loads(m_data.group(1))
    except Exception as e:
        errors.append("✗ DATA 不是合法 JSON: %s" % e)
        return errors

    n = len(arr)
    # 6a) h2 篇数三处一致
    m_h2 = re.search(r"文章清单（共\s*(\d+)\s*篇）", html)
    if m_h2 and n and int(m_h2.group(1)) != n:
        errors.append("✗ h2 篇数(%s) ≠ DATA 篇数(%d)" % (m_h2.group(1), n))

    bad_zh = bad_corr_aff = bad_if = 0
    for it in arr:
        title = str(it.get("title", ""))
        zh = str(it.get("zh", ""))
        # 6b) 标题中译: 含中文且与原题不同
        if not zh or not CJK.search(zh) or zh.strip() == title.strip():
            bad_zh += 1
        # 6c) 通讯作者单位中译: 有单位就必须有中译
        aff = str(it.get("aff", "")).strip()
        aff_zh = str(it.get("aff_zh", "")).strip()
        if aff and not CJK.search(aff_zh):
            bad_corr_aff += 1
        # 6d) IF 防编造: ifv 必须与映射表精确命中(键=journal_canon 规范名)
        ifv = it.get("ifv")
        if ifv is not None and ifmap is not None:
            key = str(it.get("journal_canon") or it.get("journal_full") or it.get("journal", "")).strip().upper()
            rec = ifmap.get(key)
            if rec is None:  # 规范化回退: 消除 JCR 键与 PubMed 全称的写法差异(逗号/撇号/连字符/AND vs &)
                nk = _norm(key)
                idx = _if_norm_index(ifmap)
                rec = idx.get(nk)
            if rec is None:  # 兼容未带 journal_canon 的旧报告: 退回 full 再试
                raw = str(it.get("journal_full") or it.get("journal", "")).strip().upper()
                raw = re.sub(r"\s*\([^)]*\)\s*$", "", raw).strip()
                if raw.startswith("THE "):
                    raw = raw[4:]
                raw2 = re.sub(r"\s+", " ", raw.replace(".", " ")).strip()
                rec = ifmap.get(raw2) or _if_norm_index(ifmap).get(_norm(raw2))
            if rec is None or rec.get("if") is None or float(rec["if"]) != float(ifv):
                bad_if += 1

    if bad_zh:
        errors.append("✗ %d 篇标题中译缺失/未译, 翻译须≥95%%" % bad_zh)
    if bad_zh and n and bad_zh > n * 0.05:
        errors.append("✗ 翻译率 %d/%d 低于 95%%" % (n - bad_zh, n))
    if bad_corr_aff:
        errors.append("✗ %d 篇通讯作者单位缺中文翻译" % bad_corr_aff)
    if bad_if:
        errors.append("✗ %d 篇 ifv 与 journal_if_map.json 不符(IF 编造红线)" % bad_if)

    # 6d-2) IF 口径标注: 有任何 ifv 就必须写明 JCR2025 年度值口径
    if any(it.get("ifv") is not None for it in arr):
        if "JCR2025" not in html:
            errors.append("✗ 缺少 IF 来源标注(JCR2025)")
        if "非文章发表当年" not in html:
            errors.append("✗ 缺少 IF 口径标注(须写明「非文章发表当年 IF」)")

    # 6e) PMID 进入链接: DATA 逐篇须有 pmid, 且 PubMed 链接模板存在
    no_pmid = sum(1 for it in arr if not str(it.get("pmid", "")).strip())
    has_pubmed_link = "pubmed.ncbi.nlm.nih.gov/" in html
    if n and (no_pmid or not has_pubmed_link):
        errors.append("✗ %d 篇缺 PMID 或 PubMed 原文链接模板缺失, 每篇须有进入链接" % no_pmid)

    # 6f) 召回审计(防静默漏检): 报告必须写明命中/取回/入选链条, 否则不许出报告
    m_audit = re.search(r"召回审计：(.{0,400}?)</div>", html, re.S)
    if n:
        if not m_audit or not m_audit.group(1).strip() or "__AUDIT__" in m_audit.group(1):
            errors.append("✗ 缺少召回审计行(须写明 窗口内命中/取回/入选), 禁止静默漏检")
        else:
            aud = m_audit.group(1)
            for kw in ("命中", "取回", "入选"):
                if kw not in aud:
                    errors.append("✗ 召回审计行缺少「%s」数据" % kw)

    # 6g) __ME__ 可选: 客户指定追踪作者时, 高亮数须等于论文数
    me = len(re.findall(r"__ME__", html))
    if me and me != n:
        errors.append("✗ __ME__ 高亮数(%d) ≠ 论文数(%d)" % (me, n))

    # 7) 大段未译英文摘要
    big_en = re.findall(r"<p[^>]*>([A-Z][^<]{200,})</p>", html)
    if big_en:
        errors.append("✗ 疑似未翻译长英文段落(%d处)" % len(big_en))

    return errors


def main():
    if len(sys.argv) < 2:
        print("用法: report_validator.py <file.html> [--ifmap journal_if_map.json]")
        sys.exit(2)
    path = sys.argv[1]
    ifmap_path = ""
    if "--ifmap" in sys.argv:
        ifmap_path = sys.argv[sys.argv.index("--ifmap") + 1]
    else:
        cand = os.path.join(os.path.dirname(os.path.abspath(__file__)), "journal_if_map.json")
        ifmap_path = cand
    ifmap = load_ifmap(ifmap_path)
    if ifmap is None:
        print("⚠ 未找到 IF 映射表(%s), 跳过 IF 防编造校验" % ifmap_path)
    errs = check(path, ifmap)
    if errs:
        print("❌ 验证未通过:")
        for e in errs:
            print("  " + e)
        sys.exit(1)
    print("✅ 验证通过: 结构闭合 / 净化词0 / 红线0 / 翻译达标 / 单位中译 / IF防编造 / PMID链接齐全")
    sys.exit(0)


if __name__ == "__main__":
    main()
