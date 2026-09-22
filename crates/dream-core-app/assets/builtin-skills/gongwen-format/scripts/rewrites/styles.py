"""按段落角色套用国标样式（GB/T 9704-2012 + GB/T 33476.2-2016）。

核心策略：
- **节属性**：A4 + 上下白边 34.58/32.58mm（软件坐标）+ 左右白边 28/26mm（默认；可切 --strict）
- **段属性**：按角色定字号/字体/行距/缩进/对齐
- **不动文本**：本工具只改样式，不改字
- **字体回退**：直接写国标推荐字体名，由 Word/WPS 在缺字体时自动回退
- **日期规整**：成文日期改写为「YYYY年M月D日」（月日不补零；只在日期段做，条码不动）
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from typing import List, Optional, Tuple

from docx import Document
from docx.document import Document as DocxDoc
from docx.shared import Pt, Mm
from docx.enum.text import WD_ALIGN_PARAGRAPH
from docx.enum.table import WD_TABLE_ALIGNMENT
from docx.oxml.ns import qn
from docx.oxml import OxmlElement

from classify import ClassifiedParagraph, PAT_DATE
from lib_docx import paragraph_has_image


# ============ 国标常量（与 check_*.py 同步） ============
# 字体
TITLE_FONT = "方正小标宋简体"            # 标题 2 号
BODY_FONT = "仿宋"                       # 正文 3 号 / 抄送印发 4 号 / 三四级层次
LEVEL1_FONT = "黑体"                     # 一级层次「一、」
LEVEL2_FONT = "楷体"                     # 二级层次「（一）」
PAGE_FONT = "宋体"                       # 页码 4 号

# 字号（pt）
TITLE_SIZE = 22.0                        # 2 号 = 22pt
BODY_SIZE = 16.0                         # 3 号 = 16pt
SECONDARY_SIZE = 14.0                    # 4 号 = 14pt（抄送、印发、页码）
LEVEL_SIZE = 16.0                        # 层次序数同正文 3 号（黑/楷/仿）

# 行距
BODY_LINE_SPACING = 28.0                 # 正文固定值 28 磅
SECONDARY_LINE_SPACING = 20.0            # 抄送/印发 4 号 → 20 磅（经验）

# 字符宽（pt）— 1 字符 ≈ 16pt（三号字），用于「左空 2 字」「首行缩进 2 字符」等
CHAR_WIDTH_BODY_PT = 16.0
CHAR_WIDTH_SECONDARY_PT = 14.0

# 节属性（mm）— 默认 33476.2 软件坐标
MARGIN_TOP_SOFTWARE = 34.58
MARGIN_BOTTOM_SOFTWARE = 32.58
# 严格 9704 坐标（上下白边 37 + 35 = 72mm）
MARGIN_TOP_STRICT = 37.0
MARGIN_BOTTOM_STRICT = 35.0
# 左右白边
MARGIN_LEFT = 28.0
MARGIN_RIGHT = 26.0
# 纸张
PAGE_WIDTH_MM = 210.0
PAGE_HEIGHT_MM = 297.0


# ============ 变更记录 ============
@dataclass
class Change:
    """单个变更条目。"""

    paragraph_index: int
    role: str
    change_type: str             # "section" / "paragraph" / "date-normalize" / "page-number"
    before: str                  # 改前简述
    after: str                   # 改后简述
    rationale: str               # 为什么这么改


# ============ 节属性 ============
def apply_section(doc: DocxDoc, strict: bool = False) -> List[Change]:
    """套用节属性：A4 + 上下白边 + 左右白边。

    strict=True  → 9704 严格坐标（上 37 / 下 35）
    strict=False → 33476.2 软件坐标（上 34.58 / 下 32.58）
    """
    changes: List[Change] = []
    for sec_idx, section in enumerate(doc.sections):
        old_w = section.page_width.mm if section.page_width else None
        old_h = section.page_height.mm if section.page_height else None
        section.page_width = Mm(PAGE_WIDTH_MM)
        section.page_height = Mm(PAGE_HEIGHT_MM)
        if abs((old_w or 0) - PAGE_WIDTH_MM) > 0.5 or abs((old_h or 0) - PAGE_HEIGHT_MM) > 0.5:
            changes.append(Change(
                paragraph_index=-1,
                role="SECTION",
                change_type="section",
                before=f"纸张 {old_w:.1f}×{old_h:.1f}mm",
                after=f"纸张 {PAGE_WIDTH_MM:.1f}×{PAGE_HEIGHT_MM:.1f}mm (A4)",
                rationale="公文用纸一律 A4（GB/T 9704-2012 §6.1）。",
            ))

        if strict:
            top, bottom = MARGIN_TOP_STRICT, MARGIN_BOTTOM_STRICT
        else:
            top, bottom = MARGIN_TOP_SOFTWARE, MARGIN_BOTTOM_SOFTWARE
        old_top = section.top_margin.mm if section.top_margin else None
        old_bot = section.bottom_margin.mm if section.bottom_margin else None
        section.top_margin = Mm(top)
        section.bottom_margin = Mm(bottom)
        if abs((old_top or 0) - top) > 0.1 or abs((old_bot or 0) - bottom) > 0.1:
            changes.append(Change(
                paragraph_index=-1,
                role="SECTION",
                change_type="section",
                before=f"上下白边 {old_top:.1f}/{old_bot:.1f}mm",
                after=f"上下白边 {top:.1f}/{bottom:.1f}mm",
                rationale="上下白边之和：9704 严格=72mm；33476.2 软件=67.16mm。",
            ))

        old_l = section.left_margin.mm if section.left_margin else None
        old_r = section.right_margin.mm if section.right_margin else None
        section.left_margin = Mm(MARGIN_LEFT)
        section.right_margin = Mm(MARGIN_RIGHT)
        if abs((old_l or 0) - MARGIN_LEFT) > 0.1 or abs((old_r or 0) - MARGIN_RIGHT) > 0.1:
            changes.append(Change(
                paragraph_index=-1,
                role="SECTION",
                change_type="section",
                before=f"左右白边 {old_l:.1f}/{old_r:.1f}mm",
                after=f"左右白边 {MARGIN_LEFT:.1f}/{MARGIN_RIGHT:.1f}mm",
                rationale="左右白边之和=54mm，版心宽=156mm（GB/T 9704-2012 §6.1）。",
            ))

    return changes


# ============ 段落样式 ============
def apply_paragraph(
    doc: DocxDoc,
    classified: ClassifiedParagraph,
    normalize_dates: bool = True,
) -> List[Change]:
    """按角色套用段落样式。

    返回该段的变更列表。
    """
    para = doc.paragraphs[classified.paragraph_index]
    changes: List[Change] = []

    role = classified.role

    # 自动编号段校正：Word 多级列表项（w:numPr）不是主送机关/普通正文，
    # 统一走层次标题处理——编号格式（"一、"/"（一）"/"1."）决定字体层级
    if role in ("ZHUSONG", "BODY") and _has_numbering(para):
        role = "LAYER_NUM"

    if role == "EMPTY":
        return changes

    if role == "IMAGE":
        # 图片段：居中 + 单倍行距（固定行距会把图片裁成一条）+ 段前后留白
        para.alignment = WD_ALIGN_PARAGRAPH.CENTER
        _set_single_line_spacing(para)
        _reset_indent(para)
        _set_space_before_after(para, before_pt=6, after_pt=6)
        changes.append(Change(
            paragraph_index=classified.paragraph_index,
            role=role,
            change_type="paragraph",
            before=f"对齐 {classified.original_align or '?'} / 行距继承原值",
            after="居中 / 单倍行距 / 段前后 6 磅",
            rationale="含图段落禁用固定行距（否则图片被裁剪），居中排布更美观（排版惯例）。",
        ))
        return changes

    if role == "CAPTION":
        # 图题/表题：4 号仿宋居中、单倍行距、段前后留白
        _set_run_font(para, BODY_FONT, SECONDARY_SIZE)
        para.alignment = WD_ALIGN_PARAGRAPH.CENTER
        _set_single_line_spacing(para)
        _reset_indent(para)
        _set_space_before_after(para, before_pt=3, after_pt=6)
        changes.append(Change(
            paragraph_index=classified.paragraph_index,
            role=role,
            change_type="paragraph",
            before=f"字号 {classified.original_size_pt or '?'}pt / 字体 {classified.original_font or '?'}",
            after=f"字号 {SECONDARY_SIZE}pt / 字体 {BODY_FONT} / 居中 / 单倍行距",
            rationale="图题表题：4 号仿宋、居中、紧跟图/表（排版惯例，国标未强制）。",
        ))
        return changes

    if role == "HEADER":
        # 发文字号：方括号 → 六角括号；同时去掉「第」和顺序号补零
        original = para.text
        new_text = _normalize_header_text(original)
        if new_text != original:
            _replace_paragraph_text(para, new_text)
        # 字体：与标题同款（3 号小标宋，但公文常用 3 号仿宋；这里用 3 号仿宋）
        _set_run_font(para, BODY_FONT, BODY_SIZE)
        para.alignment = WD_ALIGN_PARAGRAPH.CENTER
        changes.append(Change(
            paragraph_index=classified.paragraph_index,
            role=role,
            change_type="paragraph",
            before=f"原文: {original!r}",
            after=f"改后: {new_text!r} (3 号仿宋居中)",
            rationale="发文字号用六角括号〔〕；去掉「第」；顺序号不补零（GB/T 9704-2012 §7.2.5）。",
        ))
        return changes

    if role == "TITLE":
        _set_run_font(para, TITLE_FONT, TITLE_SIZE)
        para.alignment = WD_ALIGN_PARAGRAPH.CENTER
        _set_outline_level(para, 1)
        changes.append(Change(
            paragraph_index=classified.paragraph_index,
            role=role,
            change_type="paragraph",
            before=f"字号 {classified.original_size_pt or '?'}pt / 字体 {classified.original_font or '?'} / 对齐 {classified.original_align or '?'}",
            after=f"字号 {TITLE_SIZE}pt / 字体 {TITLE_FONT} / 对齐 居中 / 大纲 1 级",
            rationale="公文标题：2 号方正小标宋简体，居中（GB/T 9704-2012 §7.3.1）；大纲 1 级供导航窗格显示层级。",
        ))

    elif role == "ZHUSONG":
        _set_run_font(para, BODY_FONT, BODY_SIZE)
        para.alignment = WD_ALIGN_PARAGRAPH.LEFT
        _reset_indent(para)
        changes.append(Change(
            paragraph_index=classified.paragraph_index,
            role=role,
            change_type="paragraph",
            before=f"字号 {classified.original_size_pt or '?'}pt / 字体 {classified.original_font or '?'}",
            after=f"字号 {BODY_SIZE}pt / 字体 {BODY_FONT} / 顶格",
            rationale="主送机关：3 号仿宋，顶格书写（GB/T 9704-2012 §7.3.2）。",
        ))

    elif role == "BODY":
        _set_run_font(para, BODY_FONT, BODY_SIZE)
        para.alignment = WD_ALIGN_PARAGRAPH.JUSTIFY
        if _has_numbering(para):
            # 自动编号段（Word 多级列表）：左缩进清零 + 首行 2 字，
            # 段落级 w:ind 完全覆盖编号定义，避免"左缩进+首行"叠加错位
            _set_numbering_indent_chars(para, 2.0, char_pt=BODY_SIZE)
        else:
            _set_first_line_indent_chars(para, 2.0, char_pt=BODY_SIZE)
        if paragraph_has_image(para):
            # 含图正文段：单倍行距，避免固定行距裁剪图片
            _set_single_line_spacing(para)
        else:
            _set_exact_line_spacing_pt(para, BODY_LINE_SPACING)
        changes.append(Change(
            paragraph_index=classified.paragraph_index,
            role=role,
            change_type="paragraph",
            before=f"字号 {classified.original_size_pt or '?'}pt / 字体 {classified.original_font or '?'}",
            after=f"字号 {BODY_SIZE}pt / 字体 {BODY_FONT} / 首行缩进 2 字符 / {'单倍行距(含图)' if paragraph_has_image(para) else '28 磅固定行距'}",
            rationale="正文：3 号仿宋，首行缩进 2 字符，固定行距 28 磅（GB/T 9704-2012 §7.3.3）；含图段落改单倍行距防裁剪。",
        ))

    elif role == "ATT_DESC":
        _set_run_font(para, BODY_FONT, BODY_SIZE)
        para.alignment = WD_ALIGN_PARAGRAPH.LEFT
        _set_left_indent_chars(para, 2.0, char_pt=BODY_SIZE)
        _reset_first_line_indent(para)
        # 附件说明末尾不带标点（已有标点的去掉）
        _strip_trailing_punctuation(para)
        changes.append(Change(
            paragraph_index=classified.paragraph_index,
            role=role,
            change_type="paragraph",
            before=f"字号 {classified.original_size_pt or '?'}pt / 字体 {classified.original_font or '?'}",
            after=f"字号 {BODY_SIZE}pt / 字体 {BODY_FONT} / 左空 2 字 / 末尾无标点",
            rationale="附件说明：3 号仿宋，左空 2 字，末尾不加标点（GB/T 9704-2012 §7.3.5）。",
        ))

    elif role == "SIG_ORG":
        _set_run_font(para, BODY_FONT, BODY_SIZE)
        para.alignment = WD_ALIGN_PARAGRAPH.RIGHT
        _set_exact_line_spacing_pt(para, BODY_LINE_SPACING)
        changes.append(Change(
            paragraph_index=classified.paragraph_index,
            role=role,
            change_type="paragraph",
            before=f"字号 {classified.original_size_pt or '?'}pt / 字体 {classified.original_font or '?'}",
            after=f"字号 {BODY_SIZE}pt / 字体 {BODY_FONT} / 右对齐",
            rationale="落款机关：3 号仿宋，右对齐（GB/T 9704-2012 §8.2）。",
        ))

    elif role == "SIG_DATE":
        _set_run_font(para, BODY_FONT, BODY_SIZE)
        para.alignment = WD_ALIGN_PARAGRAPH.RIGHT
        _set_left_indent_chars(para, 2.0, char_pt=BODY_SIZE)  # 右空 2 字
        _set_exact_line_spacing_pt(para, BODY_LINE_SPACING)
        # 日期格式规整
        if normalize_dates:
            new_text, date_changed = _normalize_date_text(para.text)
            if date_changed:
                _replace_paragraph_text(para, new_text)
                changes.append(Change(
                    paragraph_index=classified.paragraph_index,
                    role=role,
                    change_type="date-normalize",
                    before=f"日期文本: {para.text!r}",
                    after=f"日期文本: {new_text!r}",
                    rationale="成文日期采用「YYYY年M月D日」格式（GB/T 9704-2012 §8.3.3）。",
                ))
        changes.append(Change(
            paragraph_index=classified.paragraph_index,
            role=role,
            change_type="paragraph",
            before=f"字号 {classified.original_size_pt or '?'}pt / 字体 {classified.original_font or '?'}",
            after=f"字号 {BODY_SIZE}pt / 字体 {BODY_FONT} / 右对齐 / 右空 2 字",
            rationale="成文日期：3 号仿宋，右对齐，右空 2 字（GB/T 9704-2012 §8.3）。",
        ))

    elif role == "CHAOSONG":
        _set_run_font(para, BODY_FONT, SECONDARY_SIZE)
        para.alignment = WD_ALIGN_PARAGRAPH.LEFT
        _reset_indent(para)
        _set_exact_line_spacing_pt(para, SECONDARY_LINE_SPACING)
        # 抄送机关末尾要加句号
        _ensure_trailing_period(para)
        changes.append(Change(
            paragraph_index=classified.paragraph_index,
            role=role,
            change_type="paragraph",
            before=f"字号 {classified.original_size_pt or '?'}pt / 字体 {classified.original_font or '?'}",
            after=f"字号 {SECONDARY_SIZE}pt / 字体 {BODY_FONT} / 顶格 / 末尾句号",
            rationale="抄送：4 号仿宋，顶格；抄送机关末尾加句号（GB/T 9704-2012 §9.4.2）。",
        ))

    elif role == "YINFA":
        _set_run_font(para, BODY_FONT, SECONDARY_SIZE)
        para.alignment = WD_ALIGN_PARAGRAPH.LEFT
        _reset_indent(para)
        _set_exact_line_spacing_pt(para, SECONDARY_LINE_SPACING)
        # 印发日期也要规整（如有）
        if normalize_dates:
            new_text, date_changed = _normalize_date_text(para.text)
            if date_changed:
                _replace_paragraph_text(para, new_text)
                changes.append(Change(
                    paragraph_index=classified.paragraph_index,
                    role=role,
                    change_type="date-normalize",
                    before=f"印发日期: {para.text!r}",
                    after=f"印发日期: {new_text!r}",
                    rationale="印发日期同正文日期格式（GB/T 9704-2012 §9.4.3）。",
                ))
        changes.append(Change(
            paragraph_index=classified.paragraph_index,
            role=role,
            change_type="paragraph",
            before=f"字号 {classified.original_size_pt or '?'}pt / 字体 {classified.original_font or '?'}",
            after=f"字号 {SECONDARY_SIZE}pt / 字体 {BODY_FONT} / 顶格",
            rationale="印发机关和印发日期：4 号仿宋，置于抄送之下（GB/T 9704-2012 §9.4.3）。",
        ))

    elif role == "LAYER_NUM":
        # 层次序数：第一层（一、）→ 黑体；第二层（（一））→ 楷体；其他 → 仿宋
        text_stripped = (classified.text or "").strip()
        num_info = _numbering_level_info(para)
        if num_info:
            # 自动编号段：由编号格式 lvlText 判层级（文本里没有"一、"字样）
            lvl_text = num_info[0]
            if lvl_text.startswith("（") or lvl_text.startswith("("):
                font, label = LEVEL2_FONT, "楷体"
                outline_lvl, outline_label = 3, "大纲 3 级"
            elif "、" in lvl_text:
                font, label = LEVEL1_FONT, "黑体"
                outline_lvl, outline_label = 2, "大纲 2 级"
            else:
                font, label = BODY_FONT, "仿宋"
                outline_lvl, outline_label = 4, "大纲 4 级"
        elif text_stripped and _is_layer1(text_stripped):
            font, label = LEVEL1_FONT, "黑体"
            outline_lvl, outline_label = 2, "大纲 2 级"
        elif text_stripped and _is_layer2(text_stripped):
            font, label = LEVEL2_FONT, "楷体"
            outline_lvl, outline_label = 3, "大纲 3 级"
        else:
            font, label = BODY_FONT, "仿宋"
            outline_lvl, outline_label = 4, "大纲 4 级"
        _set_run_font(para, font, LEVEL_SIZE)
        para.alignment = WD_ALIGN_PARAGRAPH.LEFT
        if _has_numbering(para):
            # 自动编号的层次标题（Word 多级列表"一、/（一）"）：
            # 左缩进清零 + 首行 2 字，编号统一落在 2 字位置、回行顶格
            _set_numbering_indent_chars(para, 2.0, char_pt=LEVEL_SIZE)
        else:
            _set_first_line_indent_chars(para, 2.0, char_pt=LEVEL_SIZE)
        if paragraph_has_image(para):
            _set_single_line_spacing(para)
        else:
            _set_exact_line_spacing_pt(para, BODY_LINE_SPACING)
        # 大纲级别：一级标题→2，二级→3，三四级→4（导航窗格层级树）
        _set_outline_level(para, outline_lvl)
        changes.append(Change(
            paragraph_index=classified.paragraph_index,
            role=role,
            change_type="paragraph",
            before=f"字号 {classified.original_size_pt or '?'}pt / 字体 {classified.original_font or '?'}",
            after=f"字号 {LEVEL_SIZE}pt / 字体 {font} ({label}) / 首行缩进 2 字符 / {outline_label}",
            rationale=f"层次序数四层字体：第一层黑体、第二层楷体、第三四层仿宋（GB/T 9704-2012 §7.3.3.2）；大纲级别供 Word 导航窗格显示层级。",
        ))

    return changes


# ============ 公开 API：按 role 直接套样式（无需 ClassifiedParagraph）============
def apply_role_to_paragraph(
    doc: DocxDoc,
    paragraph_index: int,
    role: str,
    *,
    text: Optional[str] = None,
    normalize_dates: bool = True,
) -> List[Change]:
    """给指定段落按 role 套用国标样式（不需要 ClassifiedParagraph）。

    适用场景：
    - Markdown 路径：角色由 HTML 注释显式指定，无需启发式
    - 复用样式逻辑，省去从 markdown 后的二次识别
    """
    from classify import ClassifiedParagraph
    para = doc.paragraphs[paragraph_index]

    cp = ClassifiedParagraph(
        paragraph_index=paragraph_index,
        text=text if text is not None else para.text,
        role=role,
        confidence="markdown-explicit",
        rationale=f"markdown 显式指定角色：{role}",
        original_size_pt=None,
        original_font=None,
        original_align=(
            "center" if para.alignment == 1
            else "right" if para.alignment == 2
            else "left" if para.alignment == 0
            else None
        ),
    )
    return apply_paragraph(doc, cp, normalize_dates=normalize_dates)


# ============ 页脚页码（占位 — 完整版留待扩展）============
def apply_page_number_footer(doc: DocxDoc) -> List[Change]:
    """在页脚插入 PAGE 域 + 4 号宋体（占位实现：先标 INFO，不主动插域）。

    完整实现涉及 sectPr.footerReference + fldChar，需要修改 OOXML。
    为避免误改用户原有页脚，本版本仅给提示。
    """
    return [Change(
        paragraph_index=-1,
        role="FOOTER",
        change_type="page-number",
        before="未检查/未改",
        after="提示",
        rationale="页脚 PAGE 域（4 号宋体、单页码居右/双页码居左）需手工核对；本版本不主动插入，避免破坏原页脚。",
    )]


# ============ 内部工具函数 ============
def _set_run_font(para, font_name: str, size_pt: float):
    """给段落所有 run 设中文字体 + 字号。"""
    for run in para.runs:
        run.font.name = font_name
        run.font.size = Pt(size_pt)
        # 中文字体设置（eastAsia）
        rPr = run._element.get_or_add_rPr()
        rFonts = rPr.find(qn("w:rFonts"))
        if rFonts is None:
            rFonts = OxmlElement("w:rFonts")
            rPr.insert(0, rFonts)
        rFonts.set(qn("w:eastAsia"), font_name)
        rFonts.set(qn("w:ascii"), font_name)
        rFonts.set(qn("w:hAnsi"), font_name)


def _set_first_line_indent_chars(para, n_chars: float, char_pt: float = 16.0):
    """首行缩进 n 字符（按 char_pt 折算磅值）。"""
    pt = n_chars * char_pt
    pf = para.paragraph_format
    pf.first_line_indent = Pt(pt)


def _set_left_indent_chars(para, n_chars: float, char_pt: float = 16.0):
    """整段左缩进 n 字符。"""
    pt = n_chars * char_pt
    pf = para.paragraph_format
    pf.left_indent = Pt(pt)
    pf.first_line_indent = Pt(0)


def _has_numbering(para) -> bool:
    """段落是否挂了 Word 自动编号（w:numPr 且 numId 有效）。"""
    pPr = para._p.pPr
    if pPr is None:
        return False
    numPr = pPr.find(qn("w:numPr"))
    if numPr is None:
        return False
    numId = numPr.find(qn("w:numId"))
    return numId is not None and numId.get(qn("w:val")) not in (None, "0")


def _numbering_level_info(para):
    """读取编号格式 lvlText，返回 (lvl_text, ilvl)；无编号返回 None。

    例如 "%1、" → ("一、"类一层)、"（%1）" → 二层、"%1." → 三层。
    """
    pPr = para._p.pPr
    numPr = pPr.find(qn("w:numPr")) if pPr is not None else None
    if numPr is None:
        return None
    numId_el = numPr.find(qn("w:numId"))
    ilvl_el = numPr.find(qn("w:ilvl"))
    num_id = numId_el.get(qn("w:val")) if numId_el is not None else None
    ilvl = ilvl_el.get(qn("w:val")) if ilvl_el is not None else "0"
    if num_id in (None, "0"):
        return None
    try:
        numbering = para.part.numbering_part.element
    except Exception:
        return None
    for num in numbering.findall(qn("w:num")):
        if num.get(qn("w:numId")) != num_id:
            continue
        abs_el = num.find(qn("w:abstractNumId"))
        if abs_el is None:
            return None
        abs_id = abs_el.get(qn("w:val"))
        for an in numbering.findall(qn("w:abstractNum")):
            if an.get(qn("w:abstractNumId")) != abs_id:
                continue
            for lvl in an.findall(qn("w:lvl")):
                if lvl.get(qn("w:ilvl")) == str(ilvl):
                    lt = lvl.find(qn("w:lvlText"))
                    return (lt.get(qn("w:val")) if lt is not None else "", ilvl)
    return None


def _set_numbering_indent_chars(para, n_chars: float, char_pt: float = 16.0):
    """自动编号段的国标缩进：左缩进 0 + 首行 n 字。

    段落级 w:ind 完全覆盖编号定义（abstractNum/lvl）的 w:ind，
    因此编号符号与首行文字统一落在 n 字位置，回行顶格，
    消除"段落左缩进 + 首行缩进"与编号定义叠加导致的错位。
    同时写入 w:firstLineChars，让 Word 按字符数自适应缩进。
    """
    pf = para.paragraph_format
    pf.left_indent = Pt(0)
    pf.first_line_indent = Pt(n_chars * char_pt)
    pPr = para._p.get_or_add_pPr()
    ind = pPr.find(qn("w:ind"))
    if ind is not None:
        ind.set(qn("w:firstLineChars"), str(int(n_chars * 100)))


def _reset_first_line_indent(para):
    para.paragraph_format.first_line_indent = Pt(0)


def _reset_indent(para):
    pf = para.paragraph_format
    pf.first_line_indent = Pt(0)
    pf.left_indent = Pt(0)


def _set_exact_line_spacing_pt(para, pt_value: float):
    """行距 = 固定值 pt_value 磅。"""
    pf = para.paragraph_format
    pf.line_spacing = Pt(pt_value)
    # 强制 lineRule = exact（fixed）
    pPr = para._p.get_or_add_pPr()
    spacing = pPr.find(qn("w:spacing"))
    if spacing is None:
        spacing = OxmlElement("w:spacing")
        pPr.append(spacing)
    spacing.set(qn("w:line"), str(int(pt_value * 20)))  # 1/20 pt
    spacing.set(qn("w:lineRule"), "exact")


def _set_single_line_spacing(para):
    """行距 = 单倍（lineRule=auto, line=240）。用于含图段落/图题/表格内。"""
    pPr = para._p.get_or_add_pPr()
    spacing = pPr.find(qn("w:spacing"))
    if spacing is None:
        spacing = OxmlElement("w:spacing")
        pPr.append(spacing)
    spacing.set(qn("w:line"), "240")
    spacing.set(qn("w:lineRule"), "auto")


def _set_space_before_after(para, before_pt: float, after_pt: float):
    """设置段前/段后间距（磅）。"""
    pf = para.paragraph_format
    pf.space_before = Pt(before_pt)
    pf.space_after = Pt(after_pt)


def _set_outline_level(para, level: int):
    """设置大纲级别（1-9）。1 级 → w:outlineLvl val=0。

    效果：Word「视图 → 导航窗格」出现层级树；大纲视图可折叠；
    引用 → 目录 也可基于大纲级别一键生成。
    """
    if not (1 <= level <= 9):
        return
    pPr = para._p.get_or_add_pPr()
    # 移除旧的大纲级别设置，避免残留
    for old in pPr.findall(qn("w:outlineLvl")):
        pPr.remove(old)
    outline = OxmlElement("w:outlineLvl")
    outline.set(qn("w:val"), str(level - 1))
    pPr.append(outline)


# ============ 表格美化 ============
def format_tables(
    doc: DocxDoc,
    *,
    cell_size_pt: float = 14.0,
) -> List[Change]:
    """统一文档内全部表格的样式（美化，不改正文内容）。

    规则（公文排版惯例，国标未强制）：
    - 表格整体居中
    - 表内文字：4 号（14pt），单倍行距，水平居中，去缩进
    - 表头行（首行）：黑体；数据行：仿宋
    - 不改单元格文字内容，不增删行列，不动边框（保留原边框）
    """
    changes: List[Change] = []
    for t_idx, table in enumerate(doc.tables):
        # 表格整体居中
        try:
            table.alignment = WD_TABLE_ALIGNMENT.CENTER
        except Exception:
            pass

        n_rows = len(table.rows)
        n_cells = 0
        for r_idx, row in enumerate(table.rows):
            is_header = (r_idx == 0)
            for cell in row.cells:
                for para in cell.paragraphs:
                    if not (para.text or "").strip():
                        # 空单元格：仍统一行距，防止行高不齐
                        _set_single_line_spacing(para)
                        _reset_indent(para)
                        continue
                    font = LEVEL1_FONT if is_header else BODY_FONT
                    _set_run_font(para, font, cell_size_pt)
                    para.alignment = WD_ALIGN_PARAGRAPH.CENTER
                    _reset_indent(para)
                    _set_single_line_spacing(para)
                    n_cells += 1

        changes.append(Change(
            paragraph_index=-1,
            role="TABLE",
            change_type="table",
            before=f"表格 {t_idx+1}（{n_rows} 行）原样式",
            after=f"居中 / 表头黑体 {cell_size_pt:g}pt / 数据仿宋 {cell_size_pt:g}pt / 单倍行距 / 单元格居中",
            rationale="表格统一 4 号字、表头黑体、整体居中（公文排版惯例，GB/T 9704 未强制）。",
        ))

    return changes


def _strip_trailing_punctuation(para):
    """去掉段尾所有标点（附件说明末尾不应有标点）。"""
    text = para.text
    if not text:
        return
    stripped = text.rstrip()
    while stripped and stripped[-1] in "。，；：、！？. ,;:!?…—":
        stripped = stripped[:-1]
    if stripped != text.rstrip():
        # 仅当确实需要删除时
        _replace_paragraph_text_keep_first_run_props(para, stripped)


def _ensure_trailing_period(para):
    """抄送机关末尾补句号。"""
    text = (para.text or "").rstrip()
    if text and not text.endswith(("。", ".")):
        new = text + "。"
        _replace_paragraph_text_keep_first_run_props(para, new)


def _normalize_date_text(text: str) -> Tuple[str, bool]:
    """规整日期格式为「YYYY年M月D日」（月日不补零）。

    返回 (新文本, 是否变更)。
    """
    if not text:
        return text, False
    stripped = text.strip()

    m = PAT_DATE.match(stripped)
    if not m:
        return text, False

    y, mo, d = m.groups()
    y = _normalize_year(y)
    new_stripped = f"{y}年{int(mo)}月{int(d)}日"

    # 保留前缀空白
    leading = text[: len(text) - len(text.lstrip())]
    trailing = text[len(text.rstrip()) :]  # 通常为空
    new_text = f"{leading}{new_stripped}{trailing}"
    return new_text, new_text != text


def _normalize_header_text(text: str) -> str:
    """规整发文字号：
    1. 方括号 [ ] → 六角括号 〔 〕
    2. 去「第」字
    3. 顺序号去前导零（如 010 → 10）
    """
    if not text:
        return text
    new = text
    # 方括号 → 六角括号（仅成对出现，且内含 2-4 位数字）
    new = re.sub(r"\[\s*(\d{2,4})\s*\]", r"〔\1〕", new)
    # 去「第」
    new = new.replace("第", "")
    # 顺序号去前导零（〔YYYY〕010号 → 〔YYYY〕10号）
    # 使用中文 〔〕 字符匹配
    new = re.sub(r"(〔\d{2,4}〕)0+(\d)", r"\1\2", new)
    return new


def _normalize_year(y: str) -> str:
    """两位年份 → 四位：25..69 → 20xx；其它 → 19xx。"""
    if len(y) == 2:
        n = int(y)
        if 25 <= n <= 69:
            return f"20{y}"
        return f"19{y}"
    return y


def _is_layer1(text: str) -> bool:
    """中文数字 + 顿号：一、二、三、..."""
    return bool(re.match(r"^[一二三四五六七八九十]+、", text))


def _is_layer2(text: str) -> bool:
    """带括号中文数字：（一）（二）..."""
    return bool(re.match(r"^[（(][一二三四五六七八九十]+[)）]", text))


def _replace_paragraph_text(para, new_text: str):
    """替换段落文本（保留第一段 run 的属性）。

    简化策略：清空所有 run，往第一个 run 写新文本；其余 run 移除。
    """
    if not para.runs:
        run = para.add_run(new_text)
        return
    first = para.runs[0]
    first.text = new_text
    # 删除其余 run
    for run in para.runs[1:]:
        run._element.getparent().remove(run._element)


def _replace_paragraph_text_keep_first_run_props(para, new_text: str):
    """替换段落文本（保留第一个 run 的字体字号属性，简化版）。"""
    _replace_paragraph_text(para, new_text)


# ============ 主调度 ============
def format_document(
    doc: DocxDoc,
    classified_list: List[ClassifiedParagraph],
    strict: bool = False,
    normalize_dates: bool = True,
) -> List[Change]:
    """套用全套样式，返回变更清单。"""
    changes: List[Change] = []

    # 节属性
    changes.extend(apply_section(doc, strict=strict))

    # 段落属性
    for cp in classified_list:
        changes.extend(apply_paragraph(doc, cp, normalize_dates=normalize_dates))

    # 表格美化
    changes.extend(format_tables(doc))

    # 页脚提示（占位）
    # changes.extend(apply_page_number_footer(doc))

    return changes
