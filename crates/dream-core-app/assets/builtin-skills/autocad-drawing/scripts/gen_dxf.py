#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
gen_dxf.py — 通用 DXF 图纸生成器（机械/建筑/电气等）

依赖：ezdxf（pip install ezdxf）

用法：
  python gen_dxf.py blank --size A3 --out blank_a3.dxf
  python gen_dxf.py spec  --json drawing.json --out result.dxf
  python gen_dxf.py plate --w 100 --h 60 --holes "20,30;80,30" --out plate.dxf
  python gen_dxf.py floorplan --json floorplan.json --out plan.dxf
"""

import argparse
import json
import math
import os
import sys

try:
    import ezdxf
    from ezdxf import units
    from ezdxf.math import Vec2
except ImportError:
    sys.stderr.write("[错误] 未安装 ezdxf。请先执行：\n"
                     "  pip install ezdxf\n")
    sys.exit(2)


# ---------------------------------------------------------------------------
# 图幅与图框（GB/T 14689）
# ---------------------------------------------------------------------------
PAPER = {
    "A0": (1189.0, 841.0),
    "A1": (841.0, 594.0),
    "A2": (594.0, 420.0),
    "A3": (420.0, 297.0),
    "A4": (297.0, 210.0),
}


def border(size):
    """返回 (装订边 a, 其余边 c)。"""
    a = 25.0
    c = 10.0 if size in ("A0", "A1") else 5.0
    return a, c


# 图层定义：名称 -> (颜色号, 线型)
LAYERS = {
    "粗实线": (7, "CONTINUOUS"),
    "细实线": (4, "CONTINUOUS"),
    "虚线": (2, "DASHED"),
    "点划线": (1, "CENTER"),
    "双点划线": (6, "PHANTOM"),
    "标注": (3, "CONTINUOUS"),
    "文字": (7, "CONTINUOUS"),
    "图框": (7, "CONTINUOUS"),
}


# ---------------------------------------------------------------------------
# 绘图环境初始化
# ---------------------------------------------------------------------------
def setup_doc(size="A4", landscape=True):
    """创建文档并建立图层/线型/文字样式/标注样式。"""
    doc = ezdxf.new(dxfversion="R2018", setup=True)
    doc.units = units.MM

    # 线型（setup=True 不一定自带全部）
    for name, pattern in {
        "DASHED": "A,5.0,-2.0",
        "CENTER": "A,15.0,-2.0,1.0,-2.0",
        "PHANTOM": "A,15.0,-2.0,1.0,-2.0,1.0,-2.0",
    }.items():
        if name not in doc.linetypes:
            doc.linetypes.add(name, pattern=pattern)

    # 图层
    for lname, (color, ltype) in LAYERS.items():
        if lname not in doc.layers:
            doc.layers.add(lname, color=color, linetype=ltype)

    # 文字样式（支持中文大字体）
    if "GB" not in doc.styles:
        st = doc.styles.add("GB", font="gbeitc.shx")
        st.dxf.bigfont = "gbcbig.shx"

    # 标注样式
    if "GB" not in doc.dimstyles:
        ds = doc.dimstyles.add("GB")
        ds.dxf.dimtxt = 3.5          # 文字高
        ds.dxf.dimasz = 3.0          # 箭头长
        ds.dxf.dimexe = 2.0          # 尺寸界线超出
        ds.dxf.dimexo = 0.5          # 尺寸界线起点偏移
        ds.dxf.dimdli = 7.0          # 基线间距
        ds.dxf.dimdec = 0            # 小数位
        ds.dxf.dimtad = 1            # 文字在尺寸线上方
        ds.dxf.dimtxsty = "GB"
        ds.dxf.dimltype = "BYBLOCK"

    return doc


def draw_frame(msp, size, landscape=True):
    """绘制图纸外框（细线）、内框（粗线）与标题栏。"""
    w, h = PAPER[size]
    if not landscape:
        w, h = h, w
    a, c = border(size)

    # 外框（细线，图纸边界）
    msp.add_lwpolyline([(0, 0), (w, 0), (w, h), (0, h), (0, 0)],
                       dxfattribs={"layer": "细实线"})
    # 内框（粗线）
    msp.add_lwpolyline([(a, c), (w - c, c), (w - c, h - c), (a, h - c), (a, c)],
                       dxfattribs={"layer": "图框"})
    # 标题栏（简化 180x56，右下角）
    tb_w, tb_h = 180.0, 56.0
    x0, y0 = w - c - tb_w, c
    draw_title_block(msp, x0, y0, tb_w, tb_h)


def draw_title_block(msp, x0, y0, w=180.0, h=56.0):
    """绘制简化国标标题栏网格与占位文字。"""
    L = "图框"
    msp.add_lwpolyline([(x0, y0), (x0 + w, y0), (x0 + w, y0 + h),
                        (x0, y0 + h), (x0, y0)], dxfattribs={"layer": L})
    # 竖向分栏
    xs = [x0 + 50, x0 + 90, x0 + 120, x0 + 150]
    for x in xs:
        msp.add_line((x, y0), (x, y0 + h), dxfattribs={"layer": "细实线"})
    # 横向分栏（上下两行，上行分三格）
    msp.add_line((x0, y0 + h * 0.5), (x0 + w, y0 + h * 0.5),
                 dxfattribs={"layer": "细实线"})
    msp.add_line((x0 + 50, y0 + h * 0.5), (x0 + 50, y0 + h),
                 dxfattribs={"layer": "细实线"})
    # 占位文字
    def txt(x, y, s, height=3.5):
        msp.add_text(s, height=height, dxfattribs={
            "layer": "文字", "style": "GB"}).set_placement(
            (x + 1.0, y + 1.0), align=ezdxf.enums.TextEntityAlignment.BOTTOM_LEFT)

    txt(x0, y0 + h * 0.5, "单位名称", 3.5)
    txt(x0 + 50, y0 + h * 0.5, "图名", 5.0)
    txt(x0 + 50, y0, "图号", 3.5)
    txt(x0 + 90, y0, "比例", 3.5)
    txt(x0 + 120, y0, "材料", 3.5)
    txt(x0 + 150, y0, "设计", 3.5)


# ---------------------------------------------------------------------------
# 实体绘制（JSON 规格）
# ---------------------------------------------------------------------------
def render_entities(msp, entities):
    for e in entities:
        typ = e.get("type")
        layer = e.get("layer", "粗实线")
        attrs = {"layer": layer}
        if "color" in e:
            attrs["color"] = e["color"]
        try:
            if typ == "line":
                msp.add_line(tuple(e["start"]), tuple(e["end"]),
                             dxfattribs=attrs)
            elif typ == "circle":
                c = tuple(e["center"])
                msp.add_circle(c, e["radius"], dxfattribs=attrs)
                if e.get("centerline"):
                    r = e["radius"]
                    ext = 5.0
                    msp.add_line((c[0] - r - ext, c[1]), (c[0] + r + ext, c[1]),
                                 dxfattribs={"layer": "点划线"})
                    msp.add_line((c[0], c[1] - r - ext), (c[0], c[1] + r + ext),
                                 dxfattribs={"layer": "点划线"})
            elif typ == "arc":
                msp.add_arc(tuple(e["center"]), e["radius"],
                            e.get("start_angle", 0), e.get("end_angle", 90),
                            dxfattribs=attrs)
            elif typ == "rect":
                x, y = e["corner"]
                w, h = e["width"], e["height"]
                msp.add_lwpolyline([(x, y), (x + w, y), (x + w, y + h),
                                    (x, y + h), (x, y)], dxfattribs=attrs)
            elif typ == "polyline":
                pts = [tuple(p) for p in e["points"]]
                msp.add_lwpolyline(pts, close=e.get("closed", False),
                                   dxfattribs=attrs)
            elif typ == "text":
                t = msp.add_text(e["text"], height=e.get("height", 3.5),
                                 dxfattribs=dict(attrs, style="GB"))
                t.set_placement(tuple(e["insert"]),
                                align=ezdxf.enums.TextEntityAlignment.BOTTOM_LEFT)
                if e.get("rotation"):
                    t.dxf.rotation = e["rotation"]
            elif typ == "dim":
                p1 = tuple(e["p1"])
                p2 = tuple(e["p2"])
                offset = e.get("offset", 10.0)
                if abs(p1[0] - p2[0]) >= abs(p1[1] - p2[1]):
                    p = (p1[0], p2[0], (p1[1] + p2[1]) / 2 - offset)
                    dim = msp.add_linear_dim(base=p, p1=p1, p2=p2,
                                             angle=0, dxfattribs=attrs)
                else:
                    p = ((p1[0] + p2[0]) / 2 - offset, p1[1], p2[1])
                    dim = msp.add_linear_dim(base=p, p1=p1, p2=p2,
                                             angle=90, dxfattribs=attrs)
                dim.render()
            elif typ == "block_ref":
                msp.add_blockref(e["name"], tuple(e["insert"]),
                                 dxfattribs=attrs)
        except KeyError as ke:
            sys.stderr.write("[警告] 实体缺少字段 %s，已跳过：%s\n" % (ke, e))


# ---------------------------------------------------------------------------
# 子命令实现
# ---------------------------------------------------------------------------
def cmd_blank(args):
    doc = setup_doc(args.size, args.landscape)
    msp = doc.modelspace()
    draw_frame(msp, args.size, args.landscape)
    doc.saveas(args.out)
    return doc


def cmd_spec(args):
    with open(args.json, "r", encoding="utf-8") as f:
        spec = json.load(f)
    size = spec.get("size", "A4")
    landscape = spec.get("landscape", True)
    doc = setup_doc(size, landscape)
    msp = doc.modelspace()
    if spec.get("title_block", True):
        draw_frame(msp, size, landscape)
    render_entities(msp, spec.get("entities", []))
    doc.saveas(args.out)
    return doc


def cmd_plate(args):
    w, h = float(args.w), float(args.h)
    doc = setup_doc("A4", True)
    msp = doc.modelspace()
    draw_frame(msp, "A4", True)
    # 外轮廓
    msp.add_lwpolyline([(0, 0), (w, 0), (w, h), (0, h), (0, 0)],
                       dxfattribs={"layer": "粗实线"})
    # 中心线
    msp.add_line((-10, h / 2), (w + 10, h / 2), dxfattribs={"layer": "点划线"})
    msp.add_line((w / 2, -10), (w / 2, h + 10), dxfattribs={"layer": "点划线"})
    # 孔
    if args.holes:
        for hstr in args.holes.split(";"):
            cx, cy = [float(v) for v in hstr.split(",")]
            msp.add_circle((cx, cy), args.r, dxfattribs={"layer": "粗实线"})
            ext = args.r + 5
            msp.add_line((cx - ext, cy), (cx + ext, cy),
                         dxfattribs={"layer": "点划线"})
            msp.add_line((cx, cy - ext), (cx, cy + ext),
                         dxfattribs={"layer": "点划线"})
    # 标注
    dim = msp.add_linear_dim(base=(0, -10, h), p1=(0, 0), p2=(w, 0), angle=0,
                             dxfattribs={"layer": "标注"})
    dim.render()
    doc.saveas(args.out)
    return doc


def cmd_floorplan(args):
    with open(args.json, "r", encoding="utf-8") as f:
        fp = json.load(f)
    size = fp.get("size", "A3")
    scale = fp.get("scale", 100.0)
    wt = fp.get("wall_thickness", 240.0)
    doc = setup_doc(size, True)
    msp = doc.modelspace()
    draw_frame(msp, size, True)

    def s(v):
        return v / scale

    # 墙体（双线）用粗实线，轴线用点划线
    for room in fp.get("rooms", []):
        x, y = room["x"], room["y"]
        w, h = room["w"], room["h"]
        X, Y, W, H = s(x), s(y), s(w), s(h)
        # 内墙线
        msp.add_lwpolyline([(X, Y), (X + W, Y), (X + W, Y + H), (X, Y + H),
                            (X, Y)], dxfattribs={"layer": "粗实线"})
        # 房间名
        t = msp.add_text(room["name"], height=3.5,
                         dxfattribs={"layer": "文字", "style": "GB"})
        t.set_placement((X + W / 2, Y + H / 2),
                        align=ezdxf.enums.TextEntityAlignment.MIDDLE_CENTER)

    # 门洞示意
    for d in fp.get("doors", []):
        x, y = s(d["x"]), s(d["y"])
        dw = s(d.get("width", 900.0))
        if d.get("horizontal", True):
            msp.add_line((x, y), (x + dw, y), dxfattribs={"layer": "粗实线"})
            # 门扇弧线
            msp.add_arc((x, y), dw, 0, 90, dxfattribs={"layer": "细实线"})
        else:
            msp.add_line((x, y), (x, y + dw), dxfattribs={"layer": "粗实线"})
            msp.add_arc((x, y), dw, 0, 90, dxfattribs={"layer": "细实线"})

    doc.saveas(args.out)
    return doc


# ---------------------------------------------------------------------------
def main():
    p = argparse.ArgumentParser(description="通用 DXF 图纸生成器")
    sub = p.add_subparsers(dest="cmd", required=True)

    pb = sub.add_parser("blank", help="空白图纸（含图框标题栏）")
    pb.add_argument("--size", default="A4", choices=list(PAPER))
    pb.add_argument("--landscape", action="store_true", default=True)
    pb.add_argument("--out", default="blank.dxf")
    pb.set_defaults(func=cmd_blank)

    ps = sub.add_parser("spec", help="从 JSON 规格生成")
    ps.add_argument("--json", required=True)
    ps.add_argument("--out", default="result.dxf")
    ps.set_defaults(func=cmd_spec)

    pp = sub.add_parser("plate", help="机械矩形板带孔")
    pp.add_argument("--w", type=float, default=100)
    pp.add_argument("--h", type=float, default=60)
    pp.add_argument("--r", type=float, default=8, help="孔径半径")
    pp.add_argument("--holes", default="", help="孔心坐标，分号分隔，如 20,30;80,30")
    pp.add_argument("--out", default="plate.dxf")
    pp.set_defaults(func=cmd_plate)

    pf = sub.add_parser("floorplan", help="建筑平面（矩形房间）")
    pf.add_argument("--json", required=True)
    pf.add_argument("--out", default="plan.dxf")
    pf.set_defaults(func=cmd_floorplan)

    args = p.parse_args()
    doc = args.func(args)
    print("[完成] 已生成：%s" % os.path.abspath(args.out))
    print("[提示] 用 AutoCAD 打开后如需 DWG，另存为即可。")


if __name__ == "__main__":
    main()
