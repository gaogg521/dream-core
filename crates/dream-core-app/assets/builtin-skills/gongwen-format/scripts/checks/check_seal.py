"""
check_seal — 落款印章三分支校验

依据：GB/T 9704-2012 §7.4（落款）

三条互斥分支：
1. 加盖印章：成文日期右空四字，印章下压署名与日期
2. 不加盖印章：成文日期右空二字，日期首字比署名首字右移二字
3. 加盖签发人签名章：签名章右空四字，其下空一行排成文日期

python-docx 看不到印章本身（印章是图片或独立对象），只能检查成文日期相对署名的右空字数。

启发：取最后两段右对齐段落（落款+日期），检查日期段的首字缩进/左缩进。
- 加盖印章：日期右空 4 字 ≈ 64 字符宽度（基于 16pt）
- 不加盖印章：日期右空 2 字 ≈ 32 字符宽度
- 签名章：日期单独一行 + 右空 4 字
"""
from __future__ import annotations

import re
from typing import List, Optional

from report import Issue, Severity, CheckResult

CHECK_ID = "seal"
CHECK_NAME = "落款印章校验"

PAT_DATE = re.compile(r"(\d{4})年(\d{1,2})月(\d{1,2})日")

# 字符宽度估算：16pt 三号字 ≈ 5.6mm = 16pt。右空 2 字 ≈ 32pt，4 字 ≈ 64pt。
RIGHT_INDENT_2CHARS_PT = 32.0  # 不加盖印章
RIGHT_INDENT_4CHARS_PT = 64.0  # 加盖印章 / 签名章


def _is_right_aligned(p) -> bool:
    return p.alignment == "right"


def _para_effective_indent_pt(p) -> float:
    """段落有效"右空"判定：left indent + first_line indent 之和（pt）。

    Word 实现"右空"两种方式：left indent 整体右移、或 first_line indent 模拟。
    """
    left = (p.indent_left_chars or 0) * 16.0  # 1 字符 ≈ 16pt（三号字）
    first = (p.indent_first_chars or 0) * 16.0
    return left + first


def run(doc, lib) -> CheckResult:
    result = CheckResult(check_id=CHECK_ID, check_name=CHECK_NAME)
    paragraphs = lib.iter_paragraphs(doc)

    # 找最后两个右对齐段落
    right_paras = [p for p in paragraphs if _is_right_aligned(p) and (p.text or "").strip()]
    if len(right_paras) < 2:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="NO-LUOKUAN",
            severity=Severity.INFO,
            title="未识别到落款（署名+日期）",
            message="落款应包含署名+成文日期两段，均右对齐。如确属无落款（如内部通知），可忽略。",
            reference="GB/T 9704-2012 §7.4",
        ))
        return result

    # 取末尾的两个右对齐段
    last_two = right_paras[-2:]
    sig_para = last_two[0]
    date_para = last_two[1]

    # 日期必须在最后
    if not PAT_DATE.search(date_para.text or ""):
        # 可能署名和日期颠倒，或日期独立
        if PAT_DATE.search(sig_para.text or ""):
            # 颠倒
            date_para, sig_para = sig_para, date_para
        else:
            result.add(Issue(
                check_id=CHECK_ID,
                rule_id="NO-DATE",
                severity=Severity.INFO,
                title="最后两段右对齐但未找到成文日期",
                message="GB/T 9704-2012 §8.3：成文日期应在落款署名之下。",
                reference="GB/T 9704-2012 §7.4",
            ))
            return result

    # 计算日期段的"右空"（left indent + first_line indent 合并）
    date_indent_pt = _para_effective_indent_pt(date_para)
    sig_indent_pt = _para_effective_indent_pt(sig_para)

    diff = date_indent_pt - sig_indent_pt
    # 容差 8pt
    if abs(date_indent_pt - RIGHT_INDENT_4CHARS_PT) <= 8:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="SEAL-OK-4CHARS",
            severity=Severity.PASS,
            title="日期右空 4 字（加盖印章 / 签名章分支）",
            message="成文日期右空四字，符合加盖印章或加盖签发人签名章分支的版式要求。",
            reference="GB/T 9704-2012 §7.4.1 / §7.4.3",
        ))
    elif abs(date_indent_pt - RIGHT_INDENT_2CHARS_PT) <= 8:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="SEAL-OK-2CHARS",
            severity=Severity.PASS,
            title="日期右空 2 字（不加盖印章分支）",
            message="成文日期右空二字，符合不加盖印章分支的版式要求。",
            reference="GB/T 9704-2012 §7.4.2",
        ))
    elif date_indent_pt < 8:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="SEAL-INDENT-INSUFFICIENT",
            severity=Severity.WARN,
            title="日期右空不足（应至少右空 2 字）",
            message="GB/T 9704-2012 §7.4：成文日期应右空至少 2 字；加盖印章分支应右空 4 字。",
            location=f"第 {date_para.paragraph_index+1} 段",
            actual=f"左缩进 {date_indent_pt:g}pt",
            expected="32pt（2 字）或 64pt（4 字）",
            fix="Word 选中日期段 → 段落 → 缩进 → 增加'右侧'或'左侧'缩进 2 字或 4 字。",
            reference="GB/T 9704-2012 §7.4",
        ))
    else:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="SEAL-INDENT-NONSTANDARD",
            severity=Severity.WARN,
            title=f"日期右空 {date_indent_pt/16:g:g} 字（非标准 2/4 字）",
            message="GB/T 9704-2012 §7.4：日期右空应等于 2 字或 4 字。",
            location=f"第 {date_para.paragraph_index+1} 段",
            actual=f"左缩进 {date_indent_pt:g}pt",
            expected="32pt（2 字）或 64pt（4 字）",
            fix="Word 调整日期段缩进为 32pt（不加盖印章）或 64pt（加盖印章）。",
            reference="GB/T 9704-2012 §7.4",
        ))

    return result