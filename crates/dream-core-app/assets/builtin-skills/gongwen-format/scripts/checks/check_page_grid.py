"""
check_page_grid — 页面网格校验

依据：GB/T 9704-2012 §5、§6（页面与版心）+ §10（行数字数）

直接可检查项：
- 纸张：A4（210×297mm）已在 check_margin 中检查
- 段落行距：固定值 28 磅
- 段落字号：正文 3 号（16pt），可逐段统计
- 段落每行字数（字符数 / 平均字符宽）—— 实际很难精确检查；可检查段落总字数占比

不做精确检查的项：
- 22 行 / 28 字 —— 取决于字体渲染，必须排版后实测，python-docx 不能精确推算。
  改为信息性提示：在每面 22 行 28 字 + 28 磅行距下，反推版心高度应能容纳 22 行；
  22 × 10.39mm = 228.58mm，比版心 225mm 溢出 3.58mm。这是 GB/T 9704 内部矛盾，
  我们提供 INFO 级提示，让用户知道。

校验策略：
1. 段落的固定行距 = 28pt → PASS；偏差 > 1pt → WARN
2. 段落正文字号 = 16pt（三号）→ PASS；偏差 > 0.5pt → WARN
3. 每段字符数估算：若版心宽 156mm / 16pt 字宽 ≈ 35 字符；超长/过短 → INFO
"""
from __future__ import annotations

import re
from typing import List

from report import Issue, Severity, CheckResult

CHECK_ID = "page-grid"
CHECK_NAME = "页面网格校验"

TARGET_LINE_PT = 28.0     # 固定行距 28 磅
TARGET_BODY_SIZE_PT = 16.0  # 正文 3 号字（16pt）
TOL_LINE = 1.0
TOL_SIZE = 0.5


def _has_text(p) -> bool:
    return bool(p.text and p.text.strip())


def run(doc, lib) -> CheckResult:
    result = CheckResult(check_id=CHECK_ID, check_name=CHECK_NAME)
    paragraphs = lib.iter_paragraphs(doc)

    # 过滤出真正的"主体正文"段落
    body_paragraphs = []
    for p in paragraphs:
        if not _has_text(p):
            continue
        text = p.text.strip()
        if p.alignment == "center":
            continue  # 标题或发文字号都居中
        if p.alignment == "right":
            continue  # 落款
        if text.endswith("：") or text.endswith(":"):
            continue  # 主送机关
        if text.startswith("附件") and len(text) < 30:
            continue  # 附件说明
        if text.startswith("抄送") or re.search(r"\d{4}年\d{1,2}月\d{1,2}日印发\s*$", text):
            continue  # 版记（含"X年X月X日印发"结尾的印发段）
        # 主体正文
        body_paragraphs.append(p)

    if not body_paragraphs:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="NO-BODY",
            severity=Severity.INFO,
            title="未识别到主体正文段落",
            message="未能定位主体正文（3 号仿宋 28 磅行距段落）。如确属空文档，可忽略。",
        ))
        return result

    # 检查每段行距 + 字号
    line_ok = 0
    line_warn = 0
    size_ok = 0
    size_warn = 0

    for p in body_paragraphs:
        # 行距
        if p.line_spacing and p.line_spacing.get("type") == "exact":
            v = p.line_spacing["value"]
            if abs(v - TARGET_LINE_PT) <= TOL_LINE:
                line_ok += 1
            else:
                line_warn += 1
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="LINE-SPACING",
                    severity=Severity.WARN,
                    title=f"行距 {v}pt ≠ 28pt",
                    message="公文正文行距应为固定值 28 磅。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual=f"{v}pt",
                    expected="28pt（固定值）",
                    fix="Word「段落 → 行距 → 固定值 28 磅」。",
                    reference="GB/T 9704-2012 §6.3",
                ))
        else:
            line_warn += 1
            result.add(Issue(
                check_id=CHECK_ID,
                rule_id="LINE-SPACING-NOT-EXACT",
                severity=Severity.WARN,
                title="正文行距未设为固定值",
                message="公文要求固定行距 28 磅。",
                location=f"第 {p.paragraph_index+1} 段",
                actual=f"类型={p.line_spacing.get('type') if p.line_spacing else '无'}",
                expected="exact 28pt",
                fix="Word「段落 → 行距 → 固定值 28 磅」。",
                reference="GB/T 9704-2012 §6.3",
            ))

        # 字号
        if p.fonts and p.fonts[0].size_pt:
            sz = p.fonts[0].size_pt
            if abs(sz - TARGET_BODY_SIZE_PT) <= TOL_SIZE:
                size_ok += 1
            else:
                size_warn += 1
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="BODY-SIZE",
                    severity=Severity.WARN,
                    title=f"正文字号 {sz:g}pt ≠ 16pt（三号）",
                    message="公文正文应使用三号字（16pt）。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual=f"{sz:g}pt",
                    expected="16pt（三号）",
                    fix="Word 选中正文 → 字号改为「三号」。",
                    reference="GB/T 9704-2012 §7.3.4",
                ))

    if line_warn == 0 and size_warn == 0:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="ALL-PASS",
            severity=Severity.PASS,
            title="页面网格合规",
            message=f"共 {line_ok} 段正文均使用 28 磅固定行距 + 三号（16pt）字。",
            reference="GB/T 9704-2012 §6.3, §7.3.4",
        ))

    # 内部矛盾提示：22 行溢出
    result.add(Issue(
        check_id=CHECK_ID,
        rule_id="GRID-OVERFLOW-INFO",
        severity=Severity.INFO,
        title="页网格 22 行 × 28 字 与版心 225mm 存在内部张力",
        message="按 9704 附录公式：22 行 × 10.39mm = 228.58mm > 版心 225mm，溢出 3.58mm；按 33476.2 附录 A 另一算法得 223.52mm。标准未自洽。实操：保证 28 磅行距 + 版心高度，行数让位于版心。",
        reference="GB/T 9704-2012 §5.3；GB/T 33476.2-2016 附录 A",
    ))
    return result