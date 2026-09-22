"""
check_pagination — 页码与版记分页校验

依据：GB/T 9704-2012 §7.6（页码）+ §7.5（版记）

核心规则：
1. 页码：4 号半角宋体阿拉伯数字，单页码居右、双页码居左
2. 版记：必须落在偶数页；不在正文同一面
3. 空白页：偶数末页应插空白页（不插页码）

python-docx 不能精确推算分页（取决于渲染），但能读 sections 的页眉/页脚设置。

本检查器侧重可机器检查项：
- 是否有页码域设置（footer 中是否包含 PAGE 域）
- 页码字体是否为 4 号半角宋体
"""
from __future__ import annotations

from typing import List

from report import Issue, Severity, CheckResult
from docx.oxml.ns import qn

CHECK_ID = "pagination"
CHECK_NAME = "页码与版记分页校验"

TARGET_FONT = "宋体"
TARGET_SIZE_PT = 14.0


def _has_page_field(footer_part) -> bool:
    """检查 footer XML 中是否含 PAGE 域。"""
    if footer_part is None:
        return False
    try:
        xml_str = footer_part.element.xml if hasattr(footer_part, "element") else footer_part.blob.decode("utf-8", "ignore")
    except Exception:
        return False
    return "PAGE" in xml_str and "fldChar" in xml_str or "instrText" in xml_str


def run(doc, lib) -> CheckResult:
    result = CheckResult(check_id=CHECK_ID, check_name=CHECK_NAME)

    page_field_found = False
    page_size_ok = False
    page_font_ok = False

    for sec_idx, section in enumerate(doc.sections):
        footer = section.footer
        if footer is None:
            continue
        for p in footer.paragraphs:
            if not (p.text or "").strip():
                continue
            # 检查 run 级字号字体
            for run in p.runs:
                sz = run.font.size
                if sz and sz.pt == TARGET_SIZE_PT:
                    page_size_ok = True
                if run.font.name and TARGET_FONT in run.font.name:
                    page_font_ok = True
            # 段落级 rPr（域不会覆盖）
            from docx.oxml.ns import qn
            pPr = p._p.find(qn("w:pPr"))
            if pPr is not None:
                ppr_rPr = pPr.find(qn("w:rPr"))
                if ppr_rPr is not None:
                    sz_el = ppr_rPr.find(qn("w:sz"))
                    if sz_el is not None:
                        sz_pt = int(sz_el.get(qn("w:val"))) / 2.0
                        if abs(sz_pt - TARGET_SIZE_PT) <= 0.5:
                            page_size_ok = True
                    rfonts = ppr_rPr.find(qn("w:rFonts"))
                    if rfonts is not None:
                        ea = rfonts.get(qn("w:eastAsia")) or rfonts.get(qn("w:ascii")) or ""
                        if TARGET_FONT in ea:
                            page_font_ok = True
        # 检查 PAGE 域
        if _has_page_field(footer.part):
            page_field_found = True

    # 分页提示（机器检查有限，仅给 INFO）
    result.add(Issue(
        check_id=CHECK_ID,
        rule_id="PAGINATION-INFO",
        severity=Severity.INFO,
        title="页码与版记分页需在 Word 中人工核对",
        message=(
            "GB/T 9704-2012 §7.6：页码 4 号半角宋体阿拉伯数字，单页码居右、双页码居左。"
            "§7.5：版记位于偶数页最末一面。"
            "Python-docx 无法精确推算分页位置，请在 Word 中打开'草稿视图'或'打印预览'核对："
            "(1) 检查版记是否在偶数页；(2) 单页码是否居右、双页码是否居左。"
        ),
        reference="GB/T 9704-2012 §7.5, §7.6",
    ))

    if not page_field_found:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="NO-PAGE-FIELD",
            severity=Severity.WARN,
            title="页脚未检测到 PAGE 域",
            message="公文应有页码。请在 Word「插入 → 页码」中插入页码域，并设置 4 号宋体阿拉伯数字。",
            fix="Word「插入 → 页码 → 页面底端」，然后选中页码改字号为四号。",
            reference="GB/T 9704-2012 §7.6",
        ))
    elif page_size_ok and page_font_ok:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="ALL-PASS",
            severity=Severity.PASS,
            title="页码格式合规",
            message="页脚包含 PAGE 域，字号 14pt、字体宋体，符合 §7.6 要求。",
            reference="GB/T 9704-2012 §7.6",
        ))
    else:
        # 有 PAGE 域但字号字体未必合规
        missing = []
        if not page_size_ok:
            missing.append("字号应为 14pt（四号）")
        if not page_font_ok:
            missing.append("字体应为宋体")
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="PAGE-FORMAT",
            severity=Severity.WARN,
            title="页码字号字体可能不合规",
            message=f"页脚含 PAGE 域但 { '、'.join(missing) }。",
            fix="Word 选中页码 → 字号改为「四号」、字体改为「宋体」。",
            reference="GB/T 9704-2012 §7.6",
        ))
    return result