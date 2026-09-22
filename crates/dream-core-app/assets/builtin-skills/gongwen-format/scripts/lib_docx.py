"""
统一读取层 —— 把 python-docx 复杂 API 包成稳定的中文单位 API。

核心目标：
1. 节边距/纸张：EMU(914400/inch) → mm，精度 0.01mm
2. 字号：Word 半磅整数 → 中文字号字符串（"三号"/"四号"/...）
3. 字体：东亚字体优先（rFonts:eastAsia），缺省回落 ascii
4. 行距：磅值 / 单倍 / 多倍 / 固定值 → 统一判定
5. 段落：首行缩进(字符) / 对齐 / 段前段后

依赖：python-docx, lxml
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Optional, List, Dict, Any

from docx import Document
from docx.document import Document as _Document
from docx.oxml.ns import qn
from docx.shared import Pt, Mm, Emu, Twips
from lxml import etree


# === 单位转换 ===

def emu_to_mm(emu: int) -> float:
    """1 inch = 914400 EMU = 25.4 mm"""
    return round(emu / 914400 * 25.4, 3)


def mm_to_emu(mm: float) -> int:
    return int(round(mm / 25.4 * 914400))


def pt_to_mm(pt: float) -> float:
    return round(pt * 25.4 / 72, 3)


# === 中文字号映射 ===

# Word 中文字号 → 半磅值。来源：GB/T 9704-2012 附录及 Office 标准对照表
# "初号"=42pt, "小初"=36pt, "一号"=26pt, "小一"=24pt, "二号"=22pt, "小二"=18pt,
# "三号"=16pt, "小三"=15pt, "四号"=14pt, "小四"=12pt, "五号"=10.5pt, "小五"=9pt,
# "六号"=7.5pt, "小六"=6.5pt, "七号"=5.5pt, "八号"=5pt

_CN_SIZE_PT = {
    "初号": 42, "小初": 36,
    "一号": 26, "小一": 24,
    "二号": 22, "小二": 18,
    "三号": 16, "小三": 15,
    "四号": 14, "小四": 12,
    "五号": 10.5, "小五": 9,
    "六号": 7.5, "小六": 6.5,
    "七号": 5.5, "八号": 5,
}


def pt_to_cn_size(pt: float) -> str:
    """把 pt 值映射到最近的中文字号。"""
    best_name = ""
    best_diff = float("inf")
    for name, ref in _CN_SIZE_PT.items():
        diff = abs(pt - ref)
        if diff < best_diff:
            best_diff = diff
            best_name = name
    # 偏差超过 0.5pt 直接返回 pt 值
    if best_diff > 0.5:
        return f"{pt:g}pt"
    return best_name


def cn_size_to_pt(name: str) -> Optional[float]:
    return _CN_SIZE_PT.get(name)


# === 节边距读取 ===

@dataclass
class SectionMargins:
    """一个节的页面边距（mm），含版心尺寸推导。"""
    top_mm: float
    bottom_mm: float
    left_mm: float
    right_mm: float
    header_mm: float  # 页眉距上
    footer_mm: float  # 页脚距下
    gutter_mm: float  # 装订线
    page_width_mm: float
    page_height_mm: float

    @property
    def text_width_mm(self) -> float:
        """版心宽度 = 纸宽 − 左 − 右"""
        return round(self.page_width_mm - self.left_mm - self.right_mm, 2)

    @property
    def text_height_mm(self) -> float:
        """版心高度 = 纸高 − 上 − 下"""
        return round(self.page_height_mm - self.top_mm - self.bottom_mm, 2)


def read_section(section, idx: int) -> Dict[str, Any]:
    """读一个节的页面/边距/纸张，返回 dict（key 友好）。"""
    sp = section._sectPr
    pgSz = sp.find(qn("w:pgSz"))
    pgMar = sp.find(qn("w:pgMar"))

    page_w = int(pgSz.get(qn("w:w"))) if pgSz is not None else 11906  # A4 默认 twips
    page_h = int(pgSz.get(qn("w:h"))) if pgSz is not None else 16838

    # pgMar 单位是 twips (1/20 pt = 1/1440 inch)
    def _tw(t):
        return round(int(t) / 1440 * 25.4, 3) if t else 0.0

    margins = SectionMargins(
        top_mm=_tw(pgMar.get(qn("w:top"))) if pgMar is not None else 0,
        bottom_mm=_tw(pgMar.get(qn("w:bottom"))) if pgMar is not None else 0,
        left_mm=_tw(pgMar.get(qn("w:left"))) if pgMar is not None else 0,
        right_mm=_tw(pgMar.get(qn("w:right"))) if pgMar is not None else 0,
        header_mm=_tw(pgMar.get(qn("w:header"))) if pgMar is not None else 0,
        footer_mm=_tw(pgMar.get(qn("w:footer"))) if pgMar is not None else 0,
        gutter_mm=_tw(pgMar.get(qn("w:gutter"))) if pgMar is not None else 0,
        page_width_mm=round(page_w / 1440 * 25.4, 3),
        page_height_mm=round(page_h / 1440 * 25.4, 3),
    )

    return {
        "section_index": idx,
        "margins": margins,
        "page_size_code": pgSz.get(qn("w:code")) if pgSz is not None else None,
        "orient": pgSz.get(qn("w:orient")) if pgSz is not None else "portrait",
    }


# === 段落/字体/字号读取 ===

@dataclass
class FontInfo:
    """一段文字的字体信息（含中英/东亚）。"""
    eastasia: Optional[str] = None  # 中文字体（东亚）
    ascii: Optional[str] = None
    size_pt: Optional[float] = None
    bold: Optional[bool] = None
    color_hex: Optional[str] = None


@dataclass
class ParaInfo:
    """一个段落的结构化信息。"""
    text: str
    style_name: Optional[str]
    alignment: Optional[str]  # left/center/right/justify
    indent_first_chars: Optional[float]  # 首行缩进字符数（按本段字号估算）
    indent_left_chars: Optional[float]
    space_before_pt: Optional[float]
    space_after_pt: Optional[float]
    line_spacing: Optional[Dict[str, Any]]  # {"type":"exact"/"single"/"multiple","value":...}
    fonts: List[FontInfo] = field(default_factory=list)
    is_first_para: bool = False
    paragraph_index: int = 0
    has_image: bool = False  # 段内是否含内联图片（drawing/pict/object）


def paragraph_has_image(paragraph) -> bool:
    """检测段落是否包含内联图片/图形。

    覆盖三种 OOXML 载体：
    - w:drawing  （现代 Word 图片/形状，DrawingML）
    - w:pict     （旧版 VML 图片）
    - w:object   （嵌入 OLE 对象，如公式、图表）
    用 .// 深度查找，可命中 mc:AlternateContent 等包装结构内的图片。
    """
    p = paragraph._p
    for tag in ("w:drawing", "w:pict", "w:object"):
        if p.findall(".//" + qn(tag)):
            return True
    return False


def _read_rfonts(rPr) -> Dict[str, Optional[str]]:
    """读 rFonts 元素。"""
    rfonts = rPr.find(qn("w:rFonts"))
    if rfonts is None:
        return {"ascii": None, "eastAsia": None, "hAnsi": None}
    return {
        "ascii": rfonts.get(qn("w:ascii")),
        "eastAsia": rfonts.get(qn("w:eastAsia")),
        "hAnsi": rfonts.get(qn("w:hAnsi")),
    }


def _read_rfonts_from_run(run) -> Dict[str, Optional[str]]:
    """从 run 节点读字体。优先用 run.rPr.rFonts，其次 paragraph 默认。"""
    rPr = run._r.find(qn("w:rPr"))
    if rPr is not None:
        f = _read_rfonts(rPr)
        if f["eastAsia"]:
            return f
    # 段落的 rPr
    p_rPr = run._r.getparent().getparent().find(qn("w:pPr"))
    if p_rPr is not None:
        ppr_rPr = p_rPr.find(qn("w:rPr"))
        if ppr_rPr is not None:
            return _read_rfonts(ppr_rPr)
    return {"ascii": None, "eastAsia": None, "hAnsi": None}


def _read_size_from_run(run) -> Optional[float]:
    """读字号（半磅 → pt）。"""
    rPr = run._r.find(qn("w:rPr"))
    if rPr is None:
        # 段落级
        pPr = run._r.getparent().find(qn("w:pPr"))
        if pPr is not None:
            ppr_rPr = pPr.find(qn("w:rPr"))
            if ppr_rPr is not None:
                rPr = ppr_rPr
    if rPr is None:
        return None
    sz = rPr.find(qn("w:sz"))
    if sz is None:
        return None
    return int(sz.get(qn("w:val"))) / 2.0


def _read_color_from_run(run) -> Optional[str]:
    rPr = run._r.find(qn("w:rPr"))
    if rPr is None:
        return None
    color = rPr.find(qn("w:color"))
    if color is None:
        return None
    return color.get(qn("w:val"))


def _read_alignment(paragraph) -> Optional[str]:
    pPr = paragraph._p.find(qn("w:pPr"))
    if pPr is None:
        return None
    jc = pPr.find(qn("w:jc"))
    if jc is None:
        return None
    val = jc.get(qn("w:val"))
    return {"left": "left", "center": "center", "right": "right",
            "both": "justify", "distribute": "distribute"}.get(val, val)


def _read_indent_chars(paragraph, default_size_pt: float = 16.0) -> Dict[str, Optional[float]]:
    """读段落缩进（twips → 字符数估算）。1 字符 ≈ 字号 pt。"""
    pPr = paragraph._p.find(qn("w:pPr"))
    if pPr is None:
        return {"first": None, "left": None}
    ind = pPr.find(qn("w:ind"))
    if ind is None:
        return {"first": None, "left": None}
    # firstLineChars 单位 1/100 字符
    first_chars = ind.get(qn("w:firstLineChars"))
    first = int(first_chars) / 100.0 if first_chars else None
    left_chars = ind.get(qn("w:leftChars"))
    left = int(left_chars) / 100.0 if left_chars else None
    if first is None:
        first_tw = ind.get(qn("w:firstLine"))
        if first_tw:
            first = round(int(first_tw) / 1440 * 72 / default_size_pt, 2)
    if left is None:
        left_tw = ind.get(qn("w:left"))
        if left_tw:
            left = round(int(left_tw) / 1440 * 72 / default_size_pt, 2)
    return {"first": first, "left": left}


def _read_spacing(paragraph) -> Dict[str, Optional[Any]]:
    """读段前段后 + 行距。"""
    pPr = paragraph._p.find(qn("w:pPr"))
    if pPr is None:
        return {"before_pt": None, "after_pt": None, "line": None}
    sp = pPr.find(qn("w:spacing"))
    if sp is None:
        return {"before_pt": None, "after_pt": None, "line": None}
    # before/after 单位 twips
    before = sp.get(qn("w:before"))
    after = sp.get(qn("w:after"))
    line = sp.get(qn("w:line"))
    lineRule = sp.get(qn("w:lineRule"))
    result = {
        "before_pt": round(int(before) / 20, 2) if before else None,
        "after_pt": round(int(after) / 20, 2) if after else None,
        "line": None,
    }
    if line:
        result["line"] = {
            "type": {"auto": "single", "exact": "exact", "atLeast": "atLeast"}.get(lineRule, lineRule or "single"),
            "value": round(int(line) / 20, 2) if lineRule in ("exact", "atLeast") else round(int(line) / 240, 3),
            "raw_twips": int(line),
        }
    return result


def read_paragraph(paragraph, index: int) -> ParaInfo:
    """读一个段落的全部结构化信息。"""
    runs = paragraph.runs
    fonts: List[FontInfo] = []
    default_size = 16.0  # 三号字默认
    if runs:
        for r in runs:
            f_dict = _read_rfonts_from_run(r)
            sz = _read_size_from_run(r)
            if sz:
                default_size = sz
            color = _read_color_from_run(r)
            bold = None
            rPr = r._r.find(qn("w:rPr"))
            if rPr is not None:
                b = rPr.find(qn("w:b"))
                bold = (b is not None and b.get(qn("w:val")) != "0") if b is not None else None
            fonts.append(FontInfo(
                eastasia=f_dict.get("eastAsia"),
                ascii=f_dict.get("ascii"),
                size_pt=sz,
                bold=bold,
                color_hex=color,
            ))

    align = _read_alignment(paragraph)
    indent = _read_indent_chars(paragraph, default_size)
    spacing = _read_spacing(paragraph)
    style_name = paragraph.style.name if paragraph.style else None

    return ParaInfo(
        text=paragraph.text,
        style_name=style_name,
        alignment=align,
        indent_first_chars=indent["first"],
        indent_left_chars=indent["left"],
        space_before_pt=spacing["before_pt"],
        space_after_pt=spacing["after_pt"],
        line_spacing=spacing["line"],
        fonts=fonts,
        paragraph_index=index,
        has_image=paragraph_has_image(paragraph),
    )


# === 顶层封装 ===

def open_doc(path: str) -> _Document:
    """打开 .docx/.wps 文件。WPS 文档也是 zip+OOXML 格式，python-docx 能读，但兼容性提示。"""
    return Document(path)


def iter_paragraphs(doc: _Document) -> List[ParaInfo]:
    """遍历全部段落（仅 body 顶层；表内段落由调用方单独处理）。"""
    result = []
    for i, p in enumerate(doc.paragraphs):
        info = read_paragraph(p, i)
        result.append(info)
    return result


def read_sections(doc: _Document) -> List[Dict[str, Any]]:
    """读所有节的边距/纸张。"""
    return [read_section(s, i) for i, s in enumerate(doc.sections)]


def get_doc_summary(doc: _Document) -> Dict[str, Any]:
    """文档级摘要：节数、段落数、表格数。"""
    return {
        "n_sections": len(doc.sections),
        "n_paragraphs": len(doc.paragraphs),
        "n_tables": len(doc.tables),
    }