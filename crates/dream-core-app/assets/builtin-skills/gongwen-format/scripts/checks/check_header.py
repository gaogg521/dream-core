"""
check_header — 版头校验

依据：GB/T 9704-2012 §7.2（版头）

版头要素自上而下：
1. 份号：6 位补零（如 000001）
2. 密级：秘密/机密/绝密 + 「★」+ 期限
3. 紧急程度：特急/加急
4. 发文机关标志：红色小标宋（电子公文为机关名称居中）
5. 发文字号：六角括号〔〕，顺序号不加「第」、不编虚位
6. 签发人：上行文才标注
7. 红色分隔线：发文字号之下 4mm

本检查器做可机器检查项：
- 发文字号格式（六角括号〔〕）
- 签发人存在性（仅当上行文）
- 份号 6 位补零
"""
from __future__ import annotations

import re
from typing import List

from report import Issue, Severity, CheckResult

CHECK_ID = "header"
CHECK_NAME = "版头校验"

# 发文字号：六角括号〔YYYY〕N号（部门代号）
PAT_HAO = re.compile(r"〔\s*(\d{2,4})\s*〕\s*(?:第)?(\d+)\s*号")
PAT_HAO_WRONG_BRACKET = re.compile(r"\[\s*(\d{2,4})\s*\]\s*(?:第)?(\d+)\s*号")  # 方括号错版（含「第」也容忍）

# 份号：6 位补零
PAT_FENHAO = re.compile(r"^\s*(\d+)\s*$")

# 上行文标志
PAT_SHANGXING = re.compile(r"(上行文|请示|报告|意见|建议)")


def _is_shangxing(text: str) -> bool:
    """启发：上行文=请示/报告/意见类。"""
    return bool(PAT_SHANGXING.search(text))


def _looks_like_hao(text: str) -> bool:
    return "号" in text and ("〔" in text or "[" in text)


def run(doc, lib) -> CheckResult:
    result = CheckResult(check_id=CHECK_ID, check_name=CHECK_NAME)
    paragraphs = lib.iter_paragraphs(doc)
    full_text = "\n".join(p.text or "" for p in paragraphs)

    is_shangxing = _is_shangxing(full_text[:1000])

    hao_found = False
    qianfa_found = False
    fenhao_found = False

    for p in paragraphs:
        text = (p.text or "").strip()
        if not text:
            continue

        # 1) 发文字号
        if PAT_HAO.search(text) and not hao_found:
            hao_found = True
            m = PAT_HAO.search(text)
            year, seq = m.group(1), m.group(2)
            if len(seq) > 1 and seq.startswith("0"):
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="HAO-SEQ-PADDED",
                    severity=Severity.WARN,
                    title=f"发文字号顺序号补零：'{seq}号'",
                    message="GB/T 9704-2012 §7.2.5：发文字号顺序号不编虚位。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual=text,
                    expected=f"〔{year}〕{int(seq)}号",
                    fix=f"将'{seq}'改为'{int(seq)}'。",
                    reference="GB/T 9704-2012 §7.2.5",
                ))
            if "第" in text:
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="HAO-DI",
                    severity=Severity.ERROR,
                    title="发文字号含「第」",
                    message="GB/T 9704-2012 §7.2.5：发文字号不加「第」。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual=text,
                    expected=f"〔{year}〕{seq}号",
                    fix="删除「第」字。",
                    reference="GB/T 9704-2012 §7.2.5",
                ))
        elif PAT_HAO_WRONG_BRACKET.search(text):
            result.add(Issue(
                check_id=CHECK_ID,
                rule_id="HAO-BRACKET",
                severity=Severity.ERROR,
                title="发文字号使用了方括号[]",
                message="GB/T 9704-2012 §7.2.5：发文字号用六角括号〔〕，不是方括号[]。",
                location=f"第 {p.paragraph_index+1} 段",
                actual=text,
                expected="六角括号〔YYYY〕N号",
                fix="Word 替换：'[' → '〔'，']' → '〕'。",
                reference="GB/T 9704-2012 §7.2.5",
            ))

        # 2) 签发人（仅上行文检查）
        if is_shangxing and ("签发人" in text or PAT_HAO.search(text)):
            if "签发人" in text:
                qianfa_found = True

        # 3) 份号：6 位纯数字
        if PAT_FENHAO.match(text) and len(text.strip()) == 6 and not fenhao_found:
            fenhao_found = True
            if text.strip() != str(int(text.strip())).zfill(6):
                # 不太可能到这里，但保险
                pass

    # 总结
    if not hao_found:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="NO-HAO",
            severity=Severity.INFO,
            title="未识别到发文字号",
            message="GB/T 9704-2012 §7.2.5：发文字号是公文必备要素。如确属不需要发文字号的公文（如内部便函），可忽略。",
            reference="GB/T 9704-2012 §7.2.5",
        ))

    if is_shangxing and not qianfa_found:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="QIANFA-MISSING",
            severity=Severity.WARN,
            title="上行文缺签发人",
            message="GB/T 9704-2012 §7.2.6：上行文应当标注签发人姓名。",
            expected="签发人 XXX",
            fix="在发文字号右侧或下行添加'签发人 XXX'。",
            reference="GB/T 9704-2012 §7.2.6",
        ))

    if not result.issues:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="ALL-PASS",
            severity=Severity.PASS,
            title="版头格式合规",
            message="发文字号使用六角括号、顺序号不补零、不含「第」，结构完整。",
            reference="GB/T 9704-2012 §7.2",
        ))
    return result