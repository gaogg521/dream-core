"""段落角色识别：把每个段落标成公文八股中的一种。

角色清单（按优先级从上到下）：
1. EMPTY            空段
2. IMAGE            纯图片段（无文字、含内联图片）
3. HEADER           发文字号（如〔2024〕3号）
4. ATT_DESC         附件说明
5. CAPTION          图题/表题（"图1 xxx" / "表2-1 xxx"）
6. CHAOSONG         抄送
7. YINFA            印发
8. SIG_DATE         成文日期（落款日期）
9. SIG_ORG          落款机关
10. ZHUSONG         主送机关
11. TITLE           标题
12. LAYER_NUM       层次序数（一/（一）/1./①②）
13. BODY            主体正文（默认）

识别启发：
- 强特征优先（关键词、位置、字号、格式）
- 位置权重：标题常在第 1 段、主送在标题后、落款/日期在结尾
- 字号权重：标题 ≥ 18pt、正文 16pt、抄送/印发 14pt
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from typing import List, Optional

# ============ 正则常量 ============
# 日期：YYYY 年 M 月 D 日 / YYYY 年 MM 月 DD 日 / YYYY-M-D / YYYY/M/D
PAT_DATE = re.compile(
    r"^(\d{2,4})[\s年\-/]+(\d{1,2})[\s月\-/]+(\d{1,2})\s*日?$"
)
# 日期（带"成文日期"前缀）
PAT_DATE_PREFIX = re.compile(r"成文日期[:：]?\s*(\d{2,4})[\s年\-/]+(\d{1,2})[\s月\-/]+(\d{1,2})\s*日?")

# 附件说明
PAT_ATT_DESC = re.compile(r"^\s*附件[:：]\s*\S")
PAT_ATT_DESC_DOT = re.compile(r"^\s*附件\d+[.．]\s*\S")

# 发文字号（含六角括号或方括号两种）
PAT_HEADER_BRACKET = re.compile(r"〔\s*\d{2,4}\s*〕")   # 〔2024〕
PAT_HEADER_WRONG_BRACKET = re.compile(r"\[\s*\d{2,4}\s*\]")   # [2024]

# 抄送
PAT_CHAOSONG = re.compile(r"^\s*抄送[:：]\s*")
# 印发
PAT_YINFA = re.compile(r"^\s*印发\s*机关")

# 主送机关：以"XX："或"XX:"结尾、顶格、短
PAT_ZHUSONG_END = re.compile(r"[：:]\s*$")

# 图题/表题：以"图N"/"表N"开头（支持 图-1、图 2-1、表3.1 等变体），且短
PAT_CAPTION = re.compile(r"^[图表]\s*[-－—~～]?\s*\d+([\-－—.．]\d+)?([\s：:.、]|$)")

# 层次序数
PAT_LAYER = re.compile(
    r"^\s*("
    r"[一二三四五六七八九十]+、|"  # 一、
    r"[（(][一二三四五六七八九十]+[)）]|"  # （一）
    r"\d+[\.．]|"  # 1.
    r"[①②③④⑤⑥⑦⑧⑨⑩]|"  # ①
    r"第[一二三四五六七八九十]+条"
    r")"
)


# ============ 数据结构 ============
@dataclass
class ClassifiedParagraph:
    """识别后的段落。"""

    paragraph_index: int          # 原始索引
    text: str                     # 文本
    role: str                     # 角色（见上方清单）
    confidence: str               # high / medium / low
    rationale: str                # 判定理由（人类可读）
    original_size_pt: Optional[float] = None  # 原字号
    original_font: Optional[str] = None         # 原字体
    original_align: Optional[str] = None       # 原对齐


# ============ 主入口 ============
def classify_paragraphs(paragraphs: List) -> List[ClassifiedParagraph]:
    """对段落列表逐一识别。

    paragraphs: lib.iter_paragraphs(doc) 返回的 ParagraphView 列表，
                含 text / alignment / fonts / indent 字段。
    """
    total = len(paragraphs)
    results: List[ClassifiedParagraph] = []
    # 正文区收口标记：层次序数/正文/图题/图片段一旦出现，即进入正文区，
    # 其后的冒号短行是正文小标题而非主送机关（主送只紧随标题出现）
    body_started = False

    for i, p in enumerate(paragraphs):
        text = p.text or ""
        stripped = text.strip()

        # 1b) 纯图片段：无文字但含内联图片
        if not stripped and getattr(p, "has_image", False):
            results.append(
                ClassifiedParagraph(
                    paragraph_index=i,
                    text=text,
                    role="IMAGE",
                    confidence="high",
                    rationale="空文字 + 内联图片 → 图片段（单倍行距防裁剪，居中）",
                    original_size_pt=None,
                    original_font=None,
                    original_align=p.alignment,
                )
            )
            continue

        # 取首个 run 的字号 / 字体
        size_pt = None
        font_name = None
        if p.fonts:
            for f in p.fonts:
                if f.size_pt and size_pt is None:
                    size_pt = f.size_pt
                if f.eastasia and font_name is None:
                    font_name = f.eastasia
                if size_pt and font_name:
                    break

        align = p.alignment  # 'left' / 'center' / 'right' / None

        role, confidence, rationale = _classify_one(
            i=i,
            total=total,
            stripped=stripped,
            align=align,
            size_pt=size_pt,
        )

        # 主送机关收口：正文区已开始后，冒号短行降级为正文小标题行
        if role == "ZHUSONG" and body_started:
            role, confidence, rationale = (
                "BODY",
                "medium",
                "正文区已开始，冒号短行判为正文小标题行：" + stripped[:20],
            )

        if role in ("LAYER_NUM", "BODY", "CAPTION", "IMAGE"):
            body_started = True

        results.append(
            ClassifiedParagraph(
                paragraph_index=i,
                text=text,
                role=role,
                confidence=confidence,
                rationale=rationale,
                original_size_pt=size_pt,
                original_font=font_name,
                original_align=align,
            )
        )

    return results


# ============ 单段识别 ============
def _classify_one(
    i: int,
    total: int,
    stripped: str,
    align: Optional[str],
    size_pt: Optional[float],
) -> tuple[str, str, str]:
    """返回 (role, confidence, rationale)。"""

    # 1) 空段
    if not stripped:
        return "EMPTY", "high", "空段"

    # 2) 发文字号（含〔YYYY〕 或 [YYYY] + 号）
    has_header_bracket = bool(PAT_HEADER_BRACKET.search(stripped) or PAT_HEADER_WRONG_BRACKET.search(stripped))
    is_in_header_zone = i > 0 and i <= 3
    if has_header_bracket and is_in_header_zone and "号" in stripped and len(stripped) <= 30:
        return "HEADER", "high", f"发文字号格式：{stripped[:20]}"

    # 3) 附件说明（"附件："开头，可能带序号）
    if PAT_ATT_DESC.match(stripped) or PAT_ATT_DESC_DOT.match(stripped):
        return "ATT_DESC", "high", f"以「附件：」开头：{stripped[:20]}"

    # 4) 抄送
    if PAT_CHAOSONG.match(stripped):
        return "CHAOSONG", "high", f"以「抄送：」开头：{stripped[:20]}"

    # 5) 印发
    if PAT_YINFA.match(stripped):
        return "YINFA", "high", f"以「印发机关」开头：{stripped[:20]}"

    # 6) 成文日期（右对齐 + 在后 1/4 段 + 匹配日期）
    near_end = i >= max(0, total - max(2, total // 4))
    if align == "right" and PAT_DATE.match(stripped) and near_end:
        return "SIG_DATE", "high", f"右对齐 + 末段 + 日期格式：{stripped}"

    # 6b) 任意位置 + 纯日期
    if PAT_DATE.match(stripped) and (align == "right" or near_end):
        return "SIG_DATE", "medium", f"日期格式 + 后段：{stripped}"

    # 5b) 图题/表题（"图1 xxx" / "表2-1 xxx"），短、无句末标点
    #     必须在主送机关启发之前判定，否则短图题会被误判为主送机关
    if (
        PAT_CAPTION.match(stripped)
        and len(stripped) <= 30
        and "。" not in stripped
    ):
        return "CAPTION", "high", f"图题/表题格式：{stripped[:20]}"

    # 7) 落款机关（右对齐 + 文末 + 非日期）
    if align == "right" and near_end and not PAT_DATE.match(stripped):
        return "SIG_ORG", "medium", f"右对齐 + 文末 + 非日期：{stripped[:20]}"

    # 8) 主送机关（以冒号结尾 + 顶格 + 短 + 紧随标题的头部区域）
    #    排除层次序数开头（"一、xxx："是层级小标题，不是主送）；
    #    区域收紧到前 min(8, total//3) 段——主送只出现在标题后、正文前
    if (
        PAT_ZHUSONG_END.search(stripped)
        and len(stripped) <= 40
        and i > 0
        and i <= min(8, max(2, total // 3))
        and not PAT_LAYER.match(stripped)
    ):
        return "ZHUSONG", "high", f"顶格 + 以「：」结尾 + 标题后紧邻区：{stripped[:20]}"

    # 8b) 主送机关（容忍缺冒号：顶格 + 短 + 标题后极窄区域；排除层次序数）
    if (
        i > 0
        and i <= min(4, max(2, total // 3))
        and len(stripped) <= 30
        and not any(c in stripped for c in "。；，、：:")
        and "号" not in stripped
        and not PAT_LAYER.match(stripped)
    ):
        return "ZHUSONG", "medium", f"顶格 + 短 + 标题后紧邻区（容忍缺冒号）：{stripped[:20]}"

    # 9) 标题（首段 + 居中 + 字号大 + 短）
    is_first = i == 0
    is_short = len(stripped) <= 60
    is_centered = align == "center"
    is_big = (size_pt or 0) >= 18

    if is_first and is_short and (is_centered or is_big):
        return "TITLE", "high", f"首段 + 短 + {'居中' if is_centered else '字号大'}：{stripped[:30]}"

    if is_first and is_short and len(stripped) > 4:
        return "TITLE", "medium", f"首段 + 短文本（默认标题）：{stripped[:30]}"

    # 10) 层次序数
    if PAT_LAYER.match(stripped):
        return "LAYER_NUM", "high", f"层次序数开头：{stripped[:20]}"

    # 11) 默认：正文
    return "BODY", "medium", "默认正文"


# ============ 汇总 ============
def summarize(results: List[ClassifiedParagraph]) -> dict:
    """汇总识别结果，给前端展示用。"""
    counts = {}
    for r in results:
        counts[r.role] = counts.get(r.role, 0) + 1
    return {
        "total_paragraphs": len(results),
        "role_counts": counts,
        "low_confidence_count": sum(1 for r in results if r.confidence == "low"),
    }
