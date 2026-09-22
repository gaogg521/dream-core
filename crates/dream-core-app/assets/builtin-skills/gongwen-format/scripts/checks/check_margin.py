"""
check_margin — 版心/页边距校验

依据：GB/T 9704-2012 §6（页面设置）+ GB/T 33476.2-2016 附录 A（软件页边距）

GB/T 9704 版心坐标（公文真正长相）：
- 上白边（天头）37mm
- 下白边 35mm
- 左白边（订口）28mm
- 右白边（翻口）26mm
- 纸张 A4：210 × 297mm
- 版心：156 × 225mm

GB/T 33476.2 软件页边距坐标（Word/WPS 直接设置）：
- 上 34.58mm（= 37 − 2.42）
- 下 32.58mm（= 35 − 2.42）
- 左 28mm
- 右 26mm

允许误差 ±1mm（9704 §6.1）。
"""
from __future__ import annotations

from typing import List
from report import Issue, Severity, CheckResult

CHECK_ID = "margin"
CHECK_NAME = "版心/页边距校验"

# GB/T 9704-2012 公文真正长相
GONGWEN_TOP = 37.0
GONGWEN_BOTTOM = 35.0
GONGWEN_LEFT = 28.0
GONGWEN_RIGHT = 26.0
GONGWEN_TEXT_W = 156.0
GONGWEN_TEXT_H = 225.0

# GB/T 33476.2-2016 Word/WPS 软件页边距
SOFTWARE_TOP = 34.58
SOFTWARE_BOTTOM = 32.58
SOFTWARE_LEFT = 28.0
SOFTWARE_RIGHT = 26.0

# 允许误差 ±1mm
TOL = 1.0


def check_section(section_data: dict, doc_format_guess: str = "auto") -> List[Issue]:
    """
    doc_format_guess: "gongwen" 表示按 9704 校；"software" 表示按 33476.2 校；
    "auto" 自动：节内容已排版为公文时优先用 gongwen，否则用 software。

    一般实操：用户用 Word 排版，所以用 software 坐标系；如果用户明确做电子公文/打印预览，用 gongwen。
    """
    m = section_data["margins"]
    issues: List[Issue] = []
    if doc_format_guess == "auto":
        # 启发：版心上下接近 224 时按 software 校；接近 225 时按 gongwen 校
        # 这里直接给两个坐标的差值诊断
        targets = [
            ("上", m.top_mm, [GONGWEN_TOP, SOFTWARE_TOP], "上白边（天头）"),
            ("下", m.bottom_mm, [GONGWEN_BOTTOM, SOFTWARE_BOTTOM], "下白边"),
            ("左", m.left_mm, [GONGWEN_LEFT, SOFTWARE_LEFT], "左白边（订口）"),
            ("右", m.right_mm, [GONGWEN_RIGHT, SOFTWARE_RIGHT], "右白边（翻口）"),
        ]
    elif doc_format_guess == "gongwen":
        targets = [
            ("上", m.top_mm, [GONGWEN_TOP], "上白边（天头）"),
            ("下", m.bottom_mm, [GONGWEN_BOTTOM], "下白边"),
            ("左", m.left_mm, [GONGWEN_LEFT], "左白边（订口）"),
            ("右", m.right_mm, [GONGWEN_RIGHT], "右白边（翻口）"),
        ]
    else:  # software
        targets = [
            ("上", m.top_mm, [SOFTWARE_TOP], "上白边（软件页边距）"),
            ("下", m.bottom_mm, [SOFTWARE_BOTTOM], "下白边"),
            ("左", m.left_mm, [SOFTWARE_LEFT], "左白边"),
            ("右", m.right_mm, [SOFTWARE_RIGHT], "右白边"),
        ]

    for side, actual, expects, label in targets:
        # 在任一允许值 ±1mm 范围内，PASS
        matched = any(abs(actual - e) <= TOL for e in expects)
        if matched:
            continue
        # 都不匹配——按最近的目标值给整改建议
        nearest = min(expects, key=lambda e: abs(actual - e))
        if abs(actual - nearest) <= 3:
            sev = Severity.WARN
        else:
            sev = Severity.ERROR
        issues.append(Issue(
            check_id=CHECK_ID,
            rule_id=f"MARGIN-{side}",
            severity=sev,
            title=f"{label}不符合公文要求",
            message=f"当前 {label}={actual:g}mm，与公文要求差距较大。",
            location=f"第 {section_data['section_index']+1} 节",
            actual=f"{actual:g}mm",
            expected=f"{nearest:g}mm（±{TOL:g}mm 容差）",
            fix=f"Word「布局 → 页面设置 → 页边距」中设置 {label}={nearest:g}mm；如需在 9704 版心坐标下印刷件呈现，再加回 7/16 字高 2.42mm。",
            reference="GB/T 9704-2012 §6.1；GB/T 33476.2-2016 附录 A",
        ))

    # 纸张尺寸
    if abs(m.page_width_mm - 210) > 1 or abs(m.page_height_mm - 297) > 1:
        issues.append(Issue(
            check_id=CHECK_ID,
            rule_id="PAGE-SIZE",
            severity=Severity.ERROR,
            title="纸张不是 A4（210×297mm）",
            message=f"当前 {m.page_width_mm:g}×{m.page_height_mm:g}mm，公文必须使用 A4。",
            location=f"第 {section_data['section_index']+1} 节",
            actual=f"{m.page_width_mm:g}×{m.page_height_mm:g}mm",
            expected="210×297mm",
            fix="Word「布局 → 纸张大小 → A4」。",
            reference="GB/T 9704-2012 §5.1",
        ))

    # 版心尺寸校验（两套坐标系：9704 vs 33476.2 软件坐标）
    # 9704 严格：156×225；33476.2 软件：156×(297-67.16)≈156×229.84
    text_width_targets = [GONGWEN_TEXT_W]  # 156 唯一
    text_height_targets = [GONGWEN_TEXT_H, SOFTWARE_TOP + 297 - SOFTWARE_BOTTOM - 67.16 + 229.84]
    # 算 software 版心高
    software_text_h = 297.0 - SOFTWARE_TOP - SOFTWARE_BOTTOM
    text_height_targets = [GONGWEN_TEXT_H, round(software_text_h, 2)]

    if not any(abs(m.text_width_mm - tw) <= TOL for tw in text_width_targets):
        issues.append(Issue(
            check_id=CHECK_ID,
            rule_id="TEXTBOX-WIDTH",
            severity=Severity.WARN,
            title=f"版心宽度 {m.text_width_mm:g}mm ≠ 公文 156mm",
            message="版心宽度由左右边距决定，左右白边之和应为 54mm。",
            location=f"第 {section_data['section_index']+1} 节",
            actual=f"{m.text_width_mm:g}mm",
            expected="156mm",
            fix="调整左右白边至 28+26=54mm。",
            reference="GB/T 9704-2012 §6.1",
        ))
    if not any(abs(m.text_height_mm - th) <= TOL for th in text_height_targets):
        issues.append(Issue(
            check_id=CHECK_ID,
            rule_id="TEXTBOX-HEIGHT",
            severity=Severity.WARN,
            title=f"版心高度 {m.text_height_mm:g}mm 与公文两套坐标都不匹配",
            message=f"9704 严格坐标=225mm；33476.2 软件坐标={software_text_h:g}mm（±{TOL:g}mm 容差）。",
            location=f"第 {section_data['section_index']+1} 节",
            actual=f"{m.text_height_mm:g}mm",
            expected=f"225mm（9704） 或 {software_text_h:g}mm（33476.2 软件）",
            fix="若做电子公文/打印件：上下 37+35mm；若在 Word/WPS 排版：上下 34.58+32.58mm（差 7/16 字高 2.42mm）。",
            reference="GB/T 9704-2012 §6.1；GB/T 33476.2-2016 附录 A",
        ))
    return issues


def run(doc, lib) -> CheckResult:
    result = CheckResult(check_id=CHECK_ID, check_name=CHECK_NAME)
    sections = lib.read_sections(doc)
    for sec in sections:
        for issue in check_section(sec, doc_format_guess="auto"):
            result.add(issue)
    if not result.issues:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="ALL-PASS",
            severity=Severity.PASS,
            title="版心与页边距合规",
            message="所有节的边距、纸张、版心尺寸均在 GB/T 9704-2012 ±1mm 容差内。",
            reference="GB/T 9704-2012 §6.1",
        ))
    return result