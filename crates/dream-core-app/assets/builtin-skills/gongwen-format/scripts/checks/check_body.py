"""
check_body — 主体校验

依据：GB/T 9704-2012 §7（主体）

核心规则：
1. 标题：2 号（22pt）小标宋，居中，单行/换行合格（梯形/菱形），"关于"不单留上行
2. 主送机关：3 号仿宋，左对齐，顶格（不缩进），以"："结尾
3. 正文：3 号仿宋，首行缩进 2 字符，行距 28 磅
4. 层次序数四层字体：
   - "一、" 黑体
   - "（一）" 楷体
   - "1." 仿宋
   - "（1）" 仿宋
5. 附件说明：正文下空一行 + "左空二字" + 「附件：」+ 名称 + **名称后不加标点**
6. 附注：左空二字 + 圆括号

本检查器侧重可机器检查项：字号、字体、缩进、对齐、附件标点。
"""
from __future__ import annotations

import re
from typing import List

from report import Issue, Severity, CheckResult

CHECK_ID = "body"
CHECK_NAME = "主体校验"

# 公文正文字体期望
TARGET_FONT = "仿宋"
TARGET_TITLE_FONT = "方正小标宋简体"
TARGET_BODY_SIZE = 16.0
TARGET_TITLE_SIZE = 22.0

# 层次序数模式
PAT_LEVEL1 = re.compile(r"^[一二三四五六七八九十]+、")  # 一、
PAT_LEVEL2 = re.compile(r"^（[一二三四五六七八九十]+）")  # （一）
PAT_LEVEL3 = re.compile(r"^\d+\.")  # 1.
PAT_LEVEL4 = re.compile(r"^（\d+）")  # （1）

# 附件说明
PAT_ATTACH_DESC = re.compile(r"^附件[:：]\s*(.+)$")


def _is_title(p) -> bool:
    """启发：居中 + 字号≥18 + 文本较短 → 标题。"""
    return (
        p.alignment == "center"
        and p.fonts and p.fonts[0].size_pt and p.fonts[0].size_pt >= 18
        and p.text and len(p.text.strip()) < 60
    )


def _is_attachment_desc(text: str) -> bool:
    return bool(PAT_ATTACH_DESC.match(text.strip()))


def run(doc, lib) -> CheckResult:
    result = CheckResult(check_id=CHECK_ID, check_name=CHECK_NAME)
    paragraphs = lib.iter_paragraphs(doc)

    title_count = 0
    title_pass = 0
    attachment_desc_count = 0
    attachment_desc_pass = 0
    level_issues = 0

    for p in paragraphs:
        text = (p.text or "").strip()
        if not text:
            continue

        # === 标题 ===
        if _is_title(p):
            title_count += 1
            # 字号校验
            sz = p.fonts[0].size_pt
            if abs(sz - TARGET_TITLE_SIZE) > 0.5:
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="TITLE-SIZE",
                    severity=Severity.WARN,
                    title=f"标题字号 {sz:g}pt ≠ 22pt（二号）",
                    message="公文标题应使用二号小标宋（22pt）。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual=f"{sz:g}pt",
                    expected="22pt（二号）",
                    fix="Word 选中标题 → 字号改为「二号」。",
                    reference="GB/T 9704-2012 §7.3.1",
                ))
            # 字体校验（容忍"华文中宋"/"宋体"作为方正小标宋简体缺省回退）
            font = p.fonts[0].eastasia or p.fonts[0].ascii or ""
            if font and not any(k in font for k in ("小标宋", "华文中宋", "方正小标宋")):
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="TITLE-FONT",
                    severity=Severity.WARN,
                    title=f"标题字体 {font!r} 不是小标宋",
                    message="公文标题应使用方正小标宋简体；缺则回退到华文中宋/宋体。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual=font,
                    expected="方正小标宋简体",
                    fix="Word 选中标题 → 字体改为「方正小标宋简体」。",
                    reference="GB/T 9704-2012 §7.3.1",
                ))
            # "关于"是否单留上行
            if text.startswith("关于") and "\n关于" in text:
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="TITLE-GUANYU",
                    severity=Severity.WARN,
                    title="标题回行时把'关于'单留上行",
                    message="GB/T 9704-2012 §7.3.2：标题回行应词意完整、长短适宜、呈梯形或菱形。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual="'关于'单独上行",
                    expected="'关于'与主要动宾结构同行",
                    fix="调整标题回行位置，避免把'关于'留上行。",
                    reference="GB/T 9704-2012 §7.3.2",
                ))
            title_pass += 1

        # === 附件说明 ===
        m = PAT_ATTACH_DESC.match(text)
        if m:
            attachment_desc_count += 1
            name = m.group(1).strip()
            # 名称后不能有标点（GB/T 9704 §7.3.5）
            if name and name[-1] in "。，；：、！？.,;:":
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="ATTACHMENT-DESC-PUNCT",
                    severity=Severity.ERROR,
                    title="附件说明名称后加了标点",
                    message="GB/T 9704-2012 §7.3.5：附件说明中，附件名称后不加标点。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual=f"附件：{name}",
                    expected=f"附件：{name.rstrip('。，；：、！？.,;:')}",
                    fix=f"删除附件名称末尾的标点符号。",
                    reference="GB/T 9704-2012 §7.3.5",
                ))
            else:
                attachment_desc_pass += 1

        # === 层次序数 ===
        if PAT_LEVEL1.match(text):
            font = (p.fonts[0].eastasia if p.fonts else None) or ""
            if font and "黑体" not in font and "SimHei" not in font and font != "黑体":
                level_issues += 1
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="LEVEL1-FONT",
                    severity=Severity.WARN,
                    title=f"层次'一、'应为黑体（实际 {font!r}）",
                    message="公文四层字体：第一层 黑体。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual=font,
                    expected="黑体",
                    fix="Word 选中 → 字体改为「黑体」。",
                    reference="GB/T 9704-2012 §7.3.4",
                ))
        elif PAT_LEVEL2.match(text):
            font = (p.fonts[0].eastasia if p.fonts else None) or ""
            if font and "楷" not in font and "Kai" not in font:
                level_issues += 1
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="LEVEL2-FONT",
                    severity=Severity.WARN,
                    title=f"层次'（一）'应为楷体（实际 {font!r}）",
                    message="公文四层字体：第二层 楷体。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual=font,
                    expected="楷体",
                    fix="Word 选中 → 字体改为「楷体」。",
                    reference="GB/T 9704-2012 §7.3.4",
                ))
        elif PAT_LEVEL3.match(text):
            font = (p.fonts[0].eastasia if p.fonts else None) or ""
            if font and "仿宋" not in font and "FangSong" not in font:
                level_issues += 1
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="LEVEL3-FONT",
                    severity=Severity.WARN,
                    title=f"层次'1.'应为仿宋（实际 {font!r}）",
                    message="公文四层字体：第三层 仿宋。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual=font,
                    expected="仿宋",
                    fix="Word 选中 → 字体改为「仿宋」。",
                    reference="GB/T 9704-2012 §7.3.4",
                ))
        elif PAT_LEVEL4.match(text):
            font = (p.fonts[0].eastasia if p.fonts else None) or ""
            if font and "仿宋" not in font and "FangSong" not in font:
                level_issues += 1
                result.add(Issue(
                    check_id=CHECK_ID,
                    rule_id="LEVEL4-FONT",
                    severity=Severity.WARN,
                    title=f"层次'（1）'应为仿宋（实际 {font!r}）",
                    message="公文四层字体：第四层 仿宋。",
                    location=f"第 {p.paragraph_index+1} 段",
                    actual=font,
                    expected="仿宋",
                    fix="Word 选中 → 字体改为「仿宋」。",
                    reference="GB/T 9704-2012 §7.3.4",
                ))

    # 总结
    if title_count == 0:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="NO-TITLE",
            severity=Severity.INFO,
            title="未识别到标题段落",
            message="文档中未识别到居中+大字号+短文本的标题段落。请确认是否遗漏标题。",
            reference="GB/T 9704-2012 §7.3.1",
        ))

    if not result.issues:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="ALL-PASS",
            severity=Severity.PASS,
            title="主体格式合规",
            message=f"标题 {title_count} 个、附件说明 {attachment_desc_pass}/{attachment_desc_count} 合规、层次序数字体均符合标准。",
            reference="GB/T 9704-2012 §7.3",
        ))
    return result