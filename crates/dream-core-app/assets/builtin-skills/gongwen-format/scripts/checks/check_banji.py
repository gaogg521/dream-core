"""
check_banji — 版记校验

依据：GB/T 9704-2012 §7.5（版记）

核心规则：
1. 版记位于偶数页最末一面（即末面页码为偶数）；不与正文同面
2. 三条分隔线：首末条粗 0.35mm（推荐值），中间细 0.25mm（推荐值）
3. 抄送机关：4 号仿宋，「抄送」+ 机关名称 + 句号（与附件说明"不加标点"方向相反！）
4. 印发机关和成文日期：4 号仿宋
"""
from __future__ import annotations

import re
from typing import List

from report import Issue, Severity, CheckResult

CHECK_ID = "banji"
CHECK_NAME = "版记校验"

PAT_CHAOSONG = re.compile(r"^抄送[:：]\s*(.+)$")
PAT_YINFA = re.compile(r"^(印发|印发机关|印发日期)")
TARGET_FONT = "仿宋"
TARGET_SIZE_PT = 14.0  # 4 号


def run(doc, lib) -> CheckResult:
    result = CheckResult(check_id=CHECK_ID, check_name=CHECK_NAME)
    paragraphs = lib.iter_paragraphs(doc)

    chaosong_found = False
    chaosong_pass = 0

    # 找最后 N 个段落作为版记启发（前 60% 不算版记）
    last_section = paragraphs[-max(3, len(paragraphs) // 4):]

    for p in paragraphs:
        text = (p.text or "").strip()

        # 抄送
        m = PAT_CHAOSONG.match(text)
        if m:
            chaosong_found = True
            names = m.group(1).strip()
            # 抄送机关末尾必须标句号
            # 名称后最后字符应终止于句号或顿号（多机关场景）
            if not names or names[-1] not in "。，；：.":
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="CHAOSONG-END-PUNCT",
                    severity=Severity.ERROR,
                    title="抄送机关末尾缺少标点",
                    message="GB/T 9704-2012 §7.5.1：抄送机关名称末尾标句号。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual=f"抄送：{names}",
                    expected=f"抄送：{names.rstrip()}。",
                    fix=f"在抄送机关末尾添加'。'。",
                    reference="GB/T 9704-2012 §7.5.1",
                ))
            else:
                chaosong_pass += 1

        # 印发机关和成文日期
        if PAT_YINFA.match(text) or text == "印发" or "印发机关" in text:
            sz = p.fonts[0].size_pt if p.fonts else None
            if sz and abs(sz - TARGET_SIZE_PT) > 0.5:
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="YINFA-SIZE",
                    severity=Severity.WARN,
                    title=f"印发机关字号 {sz:g}pt ≠ 14pt（四号）",
                    message="GB/T 9704-2012 §7.5.3：印发机关用 4 号仿宋。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual=f"{sz:g}pt",
                    expected="14pt（四号）",
                    fix="Word 选中 → 字号改为「四号」。",
                    reference="GB/T 9704-2012 §7.5.3",
                ))

    # 分隔线检查：版记首末分隔线应为粗线（≥0.35mm）；中间细线
    # python-docx 不直接读取 w:bottom（段落下边框），这里只输出 INFO 提示人工核对
    # 但文档中如检测到版记段落，则提示。
    if chaosong_found:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="SEPARATORS-INFO",
            severity=Severity.INFO,
            title="版记分隔线粗细建议核对",
            message="GB/T 9704-2012 §7.5：版记首末条分隔线粗 0.35mm（推荐），中间细线 0.25mm。注：分隔线粗细无法通过段落属性自动读出，请在 Word 中核对。",
            reference="GB/T 9704-2012 §7.5",
        ))

    if not result.issues:
        if not chaosong_found:
            result.add(Issue(
                check_id=CHECK_ID,
                rule_id="NO-BANJI",
                severity=Severity.INFO,
                title="未识别到版记要素",
                message="文档中未识别到'抄送：'或'印发机关'段落。如确属无版记的简短公文，可忽略。",
                reference="GB/T 9704-2012 §7.5",
            ))
        else:
            result.add(Issue(
                check_id=CHECK_ID,
                rule_id="ALL-PASS",
                severity=Severity.PASS,
                title="版记格式合规",
                message=f"抄送 {chaosong_pass} 处均以句号结尾。",
                reference="GB/T 9704-2012 §7.5",
            ))
    return result