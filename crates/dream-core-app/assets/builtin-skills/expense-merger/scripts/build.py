#!/usr/bin/env python3
"""
build.py <目录>
读取 <目录>/.manifest_final.json（须先复核 .manifest.json 并补齐 city / desc / category 后另存），
把 OFD 全部转 PDF，加封面汇总页，按城市分组合并为 <目录名>_报销汇总.pdf。
"""
import os, sys, json
from pathlib import Path

sys_path_dir = os.path.dirname(os.path.abspath(__file__))
if sys_path_dir not in sys.path:
    sys.path.insert(0, sys_path_dir)
from config import ENTERTAINMENT_CATEGORY, ENTERTAINMENT_TAX_NOTE, RISK_NOTES
from collections import OrderedDict

# ---------- 1. 先注册中文字体（必须早于 easyofd 调用） ----------
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont

SONGTI = "/System/Library/Fonts/Supplemental/Songti.ttc"
HEITI = "/System/Library/Fonts/STHeiti Light.ttc"
# 用宋体兜底所有 OFD 内引用的“楷体/KaiTi”等
for alias in ("宋体", "SimSun", "楷体", "KaiTi"):
    pdfmetrics.registerFont(TTFont(alias, SONGTI))
for alias in ("黑体", "SimHei", "PF"):
    pdfmetrics.registerFont(TTFont(alias, HEITI))
CN_FONT = "PF"

# ---------- 2. easyofd 空签名块的 assert 兜底 ----------
import easyofd.parser_ofd.ofd_parser as _op
_orig_get_xml_obj = _op.OFDParser.get_xml_obj


def _safe_get_xml_obj(self, label):
    if not label:
        return {}
    return _orig_get_xml_obj(self, label)


_op.OFDParser.get_xml_obj = _safe_get_xml_obj

from easyofd.ofd import OFD
from pypdf import PdfReader, PdfWriter
from reportlab.lib.pagesizes import A4
from reportlab.lib import colors
from reportlab.lib.styles import getSampleStyleSheet, ParagraphStyle
from reportlab.lib.units import cm
from reportlab.platypus import (SimpleDocTemplate, Paragraph, Spacer, Table,
                                 TableStyle)


def convert_ofds(src: str, work: Path, manifest):
    out_map = {}
    for r in manifest:
        if not r["file"].lower().endswith(".ofd"):
            continue
        outp = work / (r["file"][:-4] + ".pdf")
        if outp.exists():
            out_map[r["file"]] = str(outp)
            continue
        try:
            ofd = OFD()
            ofd.read(os.path.join(src, r["file"]), fmt="path")
            data = ofd.to_pdf()
            with open(outp, "wb") as f:
                f.write(data[0] if isinstance(data, list) else data)
            out_map[r["file"]] = str(outp)
            ofd.del_data()
        except Exception as e:
            print(f"  失败 {r['file']}: {e}", file=sys.stderr)
    return out_map


_CN_NUM = ["零", "一", "二", "三", "四", "五", "六", "七", "八", "九", "十"]


def _cn_num(n: int) -> str:
    """章节号用中文，节数是动态的（有招待费时多一节）"""
    return _CN_NUM[n] if 0 <= n < len(_CN_NUM) else str(n)


def build_cover(cover_path: Path, batch_name: str, manifest, groups):
    styles = getSampleStyleSheet()
    title_style = ParagraphStyle("T", parent=styles["Title"], fontName=CN_FONT,
                                  fontSize=20, leading=26, alignment=1)
    h2 = ParagraphStyle("H2", parent=styles["Heading2"], fontName=CN_FONT,
                         fontSize=14, leading=20, spaceBefore=10, spaceAfter=6)
    body = ParagraphStyle("B", parent=styles["BodyText"], fontName=CN_FONT,
                           fontSize=11, leading=16)

    doc = SimpleDocTemplate(str(cover_path), pagesize=A4,
                              leftMargin=2 * cm, rightMargin=2 * cm,
                              topMargin=1.8 * cm, bottomMargin=1.8 * cm)
    story = []
    story.append(Paragraph(f"{batch_name} 报销汇总", title_style))
    story.append(Paragraph(f"批次：{batch_name}", body))
    story.append(Spacer(1, 6))

    total = sum(r["amount"] or 0 for r in manifest)

    # 一、城市汇总
    sum_data = [["城市", "笔数", "小计(元)", "占比"]]
    for city, rows in groups.items():
        sub = sum(r["amount"] or 0 for r in rows)
        sum_data.append([city, str(len(rows)), f"{sub:,.2f}",
                          f"{(sub / total * 100) if total else 0:.1f}%"])
    sum_data.append(["合计", str(len(manifest)), f"{total:,.2f}", "100.0%"])

    sum_tbl = Table(sum_data, colWidths=[3 * cm, 2 * cm, 3 * cm, 2.5 * cm])
    sum_tbl.setStyle(TableStyle([
        ("FONT", (0, 0), (-1, -1), CN_FONT, 11),
        ("BACKGROUND", (0, 0), (-1, 0), colors.HexColor("#2E5C8A")),
        ("TEXTCOLOR", (0, 0), (-1, 0), colors.white),
        ("BACKGROUND", (0, -1), (-1, -1), colors.HexColor("#F0F4F8")),
        ("ALIGN", (0, 0), (-1, -1), "CENTER"),
        ("GRID", (0, 0), (-1, -1), 0.5, colors.grey),
        ("VALIGN", (0, 0), (-1, -1), "MIDDLE"),
        ("ROWBACKGROUNDS", (0, 1), (-1, -2), [colors.white, colors.HexColor("#FAFAFA")]),
    ]))
    story.append(Paragraph("一、城市汇总", h2))
    story.append(sum_tbl)
    story.append(Spacer(1, 12))

    # 二、费用科目汇总
    # 城市回答“钱花在哪”，科目回答“记到哪个账上”——财务要的是后者。
    cat_map = {}
    for r in manifest:
        cat_map.setdefault(r.get("category") or r.get("category_guess") or "待定", []).append(r)

    cat_data = [["费用科目", "笔数", "小计(元)", "占比"]]
    for cat, rows in sorted(cat_map.items(), key=lambda kv: -sum(x["amount"] or 0 for x in kv[1])):
        sub = sum(r["amount"] or 0 for r in rows)
        cat_data.append([cat, str(len(rows)), f"{sub:,.2f}",
                         f"{(sub / total * 100) if total else 0:.1f}%"])
    cat_data.append(["合计", str(len(manifest)), f"{total:,.2f}", "100.0%"])

    cat_tbl = Table(cat_data, colWidths=[4.5 * cm, 2 * cm, 3 * cm, 2.5 * cm])
    cat_tbl.setStyle(TableStyle([
        ("FONT", (0, 0), (-1, -1), CN_FONT, 11),
        ("BACKGROUND", (0, 0), (-1, 0), colors.HexColor("#2E5C8A")),
        ("TEXTCOLOR", (0, 0), (-1, 0), colors.white),
        ("BACKGROUND", (0, -1), (-1, -1), colors.HexColor("#F0F4F8")),
        ("ALIGN", (0, 0), (-1, -1), "CENTER"),
        ("ALIGN", (0, 1), (0, -1), "LEFT"),
        ("GRID", (0, 0), (-1, -1), 0.5, colors.grey),
        ("VALIGN", (0, 0), (-1, -1), "MIDDLE"),
        ("ROWBACKGROUNDS", (0, 1), (-1, -2), [colors.white, colors.HexColor("#FAFAFA")]),
    ]))
    story.append(Paragraph("二、费用科目汇总", h2))
    story.append(cat_tbl)
    story.append(Spacer(1, 12))

    # 三、业务招待费明细（只在真有招待费时出现）
    ent_rows = [r for r in manifest
                if (r.get("category") or r.get("category_guess")) == ENTERTAINMENT_CATEGORY]
    sec_no = 3
    if ent_rows:
        ent_data = [["日期", "招待事由", "被招待单位", "人数", "金额(元)"]]
        missing_any = False
        for r in ent_rows:
            e = r.get("entertainment") or {}
            reason = e.get("reason") or "【待补】"
            party = e.get("party") or "【待补】"
            head = str(e.get("headcount") or "【待补】")
            if "【待补】" in (reason + party + head):
                missing_any = True
            ent_data.append([r.get("date", ""), reason, party, head,
                             f"{(r['amount'] or 0):,.2f}"])
        ent_total = sum(r["amount"] or 0 for r in ent_rows)
        ent_data.append(["", "", "", "合计", f"{ent_total:,.2f}"])

        ent_tbl = Table(ent_data, colWidths=[2.2 * cm, 5.2 * cm, 4.2 * cm, 1.6 * cm, 2.6 * cm])
        ent_tbl.setStyle(TableStyle([
            ("FONT", (0, 0), (-1, -1), CN_FONT, 9),
            ("BACKGROUND", (0, 0), (-1, 0), colors.HexColor("#8A5C2E")),
            ("TEXTCOLOR", (0, 0), (-1, 0), colors.white),
            ("BACKGROUND", (0, -1), (-1, -1), colors.HexColor("#F8F4F0")),
            ("ALIGN", (0, 0), (-1, -1), "CENTER"),
            ("ALIGN", (1, 1), (2, -2), "LEFT"),
            ("ALIGN", (4, 1), (4, -1), "RIGHT"),
            ("GRID", (0, 0), (-1, -1), 0.4, colors.grey),
            ("VALIGN", (0, 0), (-1, -1), "MIDDLE"),
        ]))
        story.append(Paragraph("三、业务招待费明细", h2))
        story.append(ent_tbl)
        if missing_any:
            story.append(Paragraph(
                "<b>注意：</b>标【待补】的项目缺“招待事由、被招待单位、参与人数”中的一项或多项。"
                "多数企业财务制度要求这三项齐全，请补齐后再提交。", body))
        story.append(Paragraph(ENTERTAINMENT_TAX_NOTE, body))
        story.append(Spacer(1, 12))
        sec_no = 4

    # 发票明细
    det_data = [["#", "城市", "科目", "类型", "日期", "说明", "金额(元)"]]
    i = 1
    for city, rows in groups.items():
        for r in rows:
            desc = r.get("desc", r.get("route_or_seller", ""))
            # 风险标记直接挂在说明后面，不另开一列——表已经够宽了
            flags = r.get("flags") or []
            if flags:
                desc = f"{desc}（{'、'.join(flags)}）"
            cat = r.get("category") or r.get("category_guess") or "待定"
            det_data.append([str(i), city, cat, r["type"], r.get("date", ""),
                              desc, f"{(r['amount'] or 0):,.2f}"])
            i += 1
    det_data.append(["", "", "", "", "", "合计", f"{total:,.2f}"])

    det_tbl = Table(det_data, colWidths=[0.7 * cm, 1.3 * cm, 2.8 * cm, 2.2 * cm,
                                              1.9 * cm, 5.2 * cm, 2.2 * cm])
    det_tbl.setStyle(TableStyle([
        ("FONT", (0, 0), (-1, -1), CN_FONT, 9),
        ("BACKGROUND", (0, 0), (-1, 0), colors.HexColor("#2E5C8A")),
        ("TEXTCOLOR", (0, 0), (-1, 0), colors.white),
        ("BACKGROUND", (0, -1), (-1, -1), colors.HexColor("#F0F4F8")),
        ("FONT", (0, -1), (-1, -1), CN_FONT, 10),
        ("ALIGN", (0, 0), (-1, -1), "CENTER"),
        ("ALIGN", (5, 1), (5, -2), "LEFT"),
        ("ALIGN", (6, 1), (6, -1), "RIGHT"),
        ("GRID", (0, 0), (-1, -1), 0.4, colors.grey),
        ("VALIGN", (0, 0), (-1, -1), "MIDDLE"),
        ("ROWBACKGROUNDS", (0, 1), (-1, -2), [colors.white, colors.HexColor("#FAFAFA")]),
    ]))
    story.append(Paragraph(f"{_cn_num(sec_no)}、发票明细", h2))
    story.append(det_tbl)
    story.append(Spacer(1, 16))

    # 三、说明（动态生成）
    city_lines = []
    for idx, (city, rows) in enumerate(groups.items(), 1):
        types = sorted(set(r["type"] for r in rows))
        sub = sum(r["amount"] or 0 for r in rows)
        dates = sorted(r["date"] for r in rows if r.get("date"))
        date_span = ""
        if dates:
            date_span = f"{dates[0][5:]}–{dates[-1][5:]} " if dates[0] != dates[-1] else f"{dates[0][5:]} "
        city_lines.append(
            f"{idx + 2}. {date_span}{city}：{'+'.join(types)}，{len(rows)} 张，¥{sub:,.2f}；"
        )

    n_ofd = sum(1 for r in manifest if r["ext"] == "ofd")
    n_pdf = sum(1 for r in manifest if r["ext"] == "pdf")
    # 把全册出现过的风险标记汇总成人话，附在说明里
    all_flags = {}
    for r in manifest:
        for f in (r.get("flags") or []):
            all_flags.setdefault(f, 0)
            all_flags[f] += 1
    risk_lines = [f"<br/>· {f}（{c} 张）：{RISK_NOTES.get(f, '需人工确认')}"
                  for f, c in sorted(all_flags.items(), key=lambda kv: -kv[1])]

    note_parts = [
        f"1. 本汇总包含 {len(manifest)} 张发票（{n_ofd} 张 OFD + {n_pdf} 张 PDF），已统一合并为单 PDF；",
        f"2. 城市分类依据：高铁/机票按行程目的地、酒店按销售方所在地；",
        *[f"<br/>{line}" for line in city_lines],
        f"<br/>{len(city_lines) + 3}. 合计金额：<b>¥{total:,.2f}</b>。",
    ]
    if risk_lines:
        note_parts.append(f"<br/>{len(city_lines) + 4}. 需人工确认的项：")
        note_parts.extend(risk_lines)
    story.append(Paragraph(f"{_cn_num(sec_no + 1)}、说明", h2))
    story.append(Paragraph("<br/>".join(note_parts), body))

    doc.build(story)


def main(src_dir: str):
    src_dir = os.path.abspath(src_dir)
    batch = os.path.basename(src_dir.rstrip("/"))
    manifest_path = os.path.join(src_dir, ".manifest_final.json")
    if not os.path.exists(manifest_path):
        print(f"ERROR: 找不到 {manifest_path}（请先复核 .manifest.json，补齐 city / desc / category 后另存为 .manifest_final.json）",
              file=sys.stderr)
        sys.exit(1)
    with open(manifest_path, encoding="utf-8") as f:
        manifest = json.load(f)

    # 按城市分组（保持文件顺序，城市出现顺序由 manifest 决定）
    groups = OrderedDict()
    for r in manifest:
        groups.setdefault(r["city"], []).append(r)

    work = Path.home() / ".cache" / "expense-merger-work" / batch
    work.mkdir(parents=True, exist_ok=True)
    for f in work.iterdir():
        if f.is_file():
            f.unlink()

    print(f"[1/3] OFD → PDF …", file=sys.stderr)
    ofd_pdfs = convert_ofds(src_dir, work, manifest)

    print(f"[2/3] 生成封面 …", file=sys.stderr)
    cover = work / "_cover.pdf"
    build_cover(cover, batch, manifest, groups)

    print(f"[3/3] 合并 …", file=sys.stderr)
    writer = PdfWriter()
    for pg in PdfReader(str(cover)).pages:
        writer.add_page(pg)
    for city, rows in groups.items():
        for r in rows:
            p = ofd_pdfs.get(r["file"]) or os.path.join(src_dir, r["file"])
            for pg in PdfReader(p).pages:
                writer.add_page(pg)

    out = os.path.join(src_dir, f"{batch}_报销汇总.pdf")
    with open(out, "wb") as f:
        writer.write(f)

    total = sum(r["amount"] or 0 for r in manifest)
    print(json.dumps({
        "output": out,
        "pages": len(writer.pages),
        "size_bytes": os.path.getsize(out),
        "n_invoices": len(manifest),
        "total_amount": round(total, 2),
        "by_city": {c: round(sum(r["amount"] or 0 for r in rs), 2)
                     for c, rs in groups.items()},
    }, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("用法: build.py <目录>", file=sys.stderr)
        sys.exit(1)
    main(sys.argv[1])
