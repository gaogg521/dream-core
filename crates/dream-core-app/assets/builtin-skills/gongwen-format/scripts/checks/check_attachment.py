"""
check_attachment — 附件三态区分校验

依据：GB/T 9704-2012 §7.3.5 + 蒸馏 skill `gongwen-attachment-triad`

附件一词的三种不同技术态：
1. **附件说明**：正文下空一行、左空二字、「附件：」+ 名称（不加标点）—— 在主体内
2. **附件**：另面编排、版记之前、首页左上角标注「附件」+ 顺序号 —— 独立文件
3. **外部附件**：以图形对象嵌入（如扫描件），不是 Word 主体

机器可检查项：
- 「附件说明」段落数量 ≥ 0；如果有正文下空一行+左空二字，则 PASS；否则 WARN
- 不应将"附件说明"写成"附："或"附录："（错版）
"""
from __future__ import annotations

import re
from typing import List

from report import Issue, Severity, CheckResult

CHECK_ID = "attachment"
CHECK_NAME = "附件三态区分校验"

PAT_ATT_DESC = re.compile(r"^附件[:：]\s*\S+")
PAT_WRONG = re.compile(r"^(附[:：]|附录[:：])\s*\S+")


def run(doc, lib) -> CheckResult:
    result = CheckResult(check_id=CHECK_ID, check_name=CHECK_NAME)
    paragraphs = lib.iter_paragraphs(doc)

    desc_count = 0
    wrong_count = 0
    empty_after_count = 0  # 标题段下方是否有空段（粗略检查）

    # 找附件说明段
    for i, p in enumerate(paragraphs):
        text = (p.text or "").strip()
        if not text:
            continue
        if PAT_ATT_DESC.match(text):
            desc_count += 1
            # 左空二字 ≈ 32pt
            if p.indent_left_chars is not None and abs(p.indent_left_chars - 2.0) > 0.5:
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="ATT-DESC-INDENT",
                    severity=Severity.WARN,
                    title="附件说明未左空二字",
                    message="GB/T 9704-2012 §7.3.5：附件说明左空二字。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual=f"左缩进 {p.indent_left_chars:g} 字符",
                    expected="2 字符",
                    fix="Word 选中附件说明段 → 段落 → 缩进 → 左侧 2 字符。",
                    reference="GB/T 9704-2012 §7.3.5",
                ))
            # 名称后无标点（已由 check_body 检查）
        if PAT_WRONG.match(text):
            wrong_count += 1
            result.add(Issue(
                check_id=CHECK_ID,
                rule_id="ATT-DESC-WRONG-PREFIX",
                severity=Severity.WARN,
                title="附件说明前缀错版（'附：'或'附录：'）",
                message="GB/T 9704-2012 §7.3.5：附件说明正确前缀为'附件：'，不是'附：'或'附录：'。",
                location=f"第 {p.paragraph_index+1} 段",
                actual=text,
                expected="附件：XXX",
                fix="Word 替换：'附：'或'附录：' → '附件：'。",
                reference="GB/T 9704-2012 §7.3.5",
            ))

    # 附件说明前应空一行
    if desc_count > 0:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="ATT-DESC-NEWLINE",
            severity=Severity.INFO,
            title="附件说明上方应空一行",
            message="GB/T 9704-2012 §7.3.5：附件说明在正文下空一行处、左空二字。请在 Word 中确认附件说明段前有空段。",
            reference="GB/T 9704-2012 §7.3.5",
        ))

    if not result.issues:
        if desc_count == 0 and wrong_count == 0:
            result.add(Issue(
                check_id=CHECK_ID,
                rule_id="NO-ATTACHMENT",
                severity=Severity.INFO,
                title="未识别到附件说明",
                message="文档中无'附件：'段落。如确属无附件的公文，可忽略。",
                reference="GB/T 9704-2012 §7.3.5",
            ))
        else:
            result.add(Issue(
                check_id=CHECK_ID,
                rule_id="ALL-PASS",
                severity=Severity.PASS,
                title="附件格式合规",
                message=f"附件说明 {desc_count} 处，前缀与缩进均符合 §7.3.5。",
                reference="GB/T 9704-2012 §7.3.5",
            ))
    return result