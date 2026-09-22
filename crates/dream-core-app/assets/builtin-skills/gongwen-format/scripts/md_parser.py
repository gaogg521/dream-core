"""Markdown → 公文 docx 解析器。

约定语法
========

本解析器读入用户编写的 Markdown 文件，按 HTML 注释行的角色标记把段落
转换成 [(role, text), ...]，再交给 styles.apply_role_to_paragraph 套国标样式。

**段落级角色标记**：单独一行写 `<!--role: NAME-->`，它下方连续的段落都属于该角色，
直到下一个 role 标记或文件结束。

支持的 role 名（必须大写）：

    TITLE         标题（22pt 方正小标宋 居中）
    HAO           发文字号（16pt 仿宋 居中，含 [ ]→〔 〕字符规整）
    ZHUSONG       主送机关（16pt 仿宋 顶格）
    BODY          正文（16pt 仿宋 首行缩进2字 28磅行距）
    LAYER_NUM     层次序数（一层黑体/二层楷体/三四层仿宋）
    ATT_DESC      附件说明（16pt 仿宋 左空2字 末尾无标点）
    SIG_ORG       落款机关（16pt 仿宋 右对齐）
    SIG_DATE      成文日期（16pt 仿宋 右对齐 右空2字 月日不补零）
    CHAOSONG      抄送（14pt 仿宋 末尾加句号）
    YINFA         印发（14pt 仿宋）
    EMPTY         空段（仅占位）

未标记 role 的段落默认 BODY。

Markdown 语法提示
=================

- `# xxx` → 标题（如果没显式 role，作为提示）
- `## xxx` → 二级标题
- `- xxx` → bullet（视为 BODY）
- 多行连续文本 → 合并为一个段落
- 空行 → 分段分隔

示例 markdown
=============

<!--role: TITLE-->
# 关于印发《萌宠养成运营方案》的通知

<!--role: HAO-->
花宠运〔2024〕10号

<!--role: ZHUSONG-->
各部门、各分公司：

<!--role: BODY-->
为加强萌宠养成项目的运营管理，规范工作流程，提升服务品质，
经研究决定，现印发《萌宠养成运营方案》。请各单位认真贯彻执行。

<!--role: LAYER_NUM-->
一、目标与原则

<!--role: BODY-->
坚持用户导向，突出数据驱动，推动业务可持续发展。

<!--role: ATT_DESC-->
附件：萌宠养成运营方案全文

<!--role: SIG_ORG-->
花小弄宠物服务有限公司

<!--role: SIG_DATE-->
2024年1月5日

<!--role: CHAOSONG-->
抄送：公司领导、各业务部门。

<!--role: YINFA-->
花小弄宠物服务有限公司办公室  2024年1月8日印发
"""
from __future__ import annotations

import re
import sys
from pathlib import Path
from typing import List, Tuple

# 合法角色名（与 styles.py 对齐）
VALID_ROLES = {
    "TITLE", "HAO", "ZHUSONG", "BODY", "LAYER_NUM",
    "ATT_DESC", "SIG_ORG", "SIG_DATE", "CHAOSONG", "YINFA", "CAPTION", "EMPTY",
}

# 行级 HTML 注释角色标记：必须独立成行，前后允许空白
PAT_ROLE_COMMENT = re.compile(r"^\s*<!--\s*role:\s*([A-Z_]+)\s*-->\s*$")
# 节级标记（保留供将来扩展）：<!--sec: A4_STRICT-->
PAT_SEC_COMMENT = re.compile(r"^\s*<!--\s*sec:\s*([A-Z_0-9]+)\s*-->\s*$")
# 图片：![alt](path)
PAT_IMAGE = re.compile(r"^\s*!\[([^\]]*)\]\(([^)]+)\)\s*$")


def parse_markdown(input_path: Path) -> List[Tuple[str, object]]:
    """解析 markdown 文件，返回 [(role, content), ...] 列表。

    content 通常是 str（段落文本）；两类特殊条目：
    - ("IMAGE", (alt_text, image_path))  图片
    - ("TABLE", [[cell, ...], ...])      表格（已解析的行，不含分隔行）
    """
    text = input_path.read_text(encoding="utf-8")
    lines = text.splitlines()

    items: List[Tuple[str, object]] = []
    current_role = "BODY"  # 默认值
    current_sec = "A4_DEFAULT"
    buffer: List[str] = []
    table_buffer: List[str] = []

    def flush() -> None:
        """把缓冲里的多行合并成一个段，追加到 items。"""
        nonlocal buffer
        if buffer:
            joined = " ".join(s.strip() for s in buffer if s.strip())
            if joined:
                items.append((current_role, joined))
        buffer = []

    def flush_table() -> None:
        """把连续的表格行解析成 ("TABLE", rows) 条目。"""
        nonlocal table_buffer
        if table_buffer:
            rows = _parse_table_rows(table_buffer)
            if rows:
                items.append(("TABLE", rows))
            table_buffer = []

    for raw_line in lines:
        line = raw_line.rstrip("\r\n").rstrip()

        # 角色注释
        m_role = PAT_ROLE_COMMENT.match(line)
        if m_role:
            flush()
            flush_table()
            role = m_role.group(1).upper()
            current_role = role if role in VALID_ROLES else "BODY"
            continue

        # 节注释（保留解析但不立即生效，留给后续扩展）
        m_sec = PAT_SEC_COMMENT.match(line)
        if m_sec:
            current_sec = m_sec.group(1).upper()
            continue

        # 表格行（以 | 开头）：累积到表格缓冲
        if line.lstrip().startswith("|"):
            flush()
            table_buffer.append(line.strip())
            continue
        else:
            flush_table()

        # 空行：段落分隔
        if not line.strip():
            flush()
            continue

        # 图片行：![alt](path)
        m_img = PAT_IMAGE.match(line)
        if m_img:
            flush()
            items.append(("IMAGE", (m_img.group(1).strip(), m_img.group(2).strip())))
            continue

        # 去 markdown 修饰
        clean = line.strip()
        if clean.startswith("# "):
            clean = clean[2:].strip()
        elif clean.startswith("## "):
            clean = clean[3:].strip()
        elif clean.startswith("### "):
            clean = clean[4:].strip()
        elif clean.startswith("---"):
            # 水平线：作为分隔（忽略）
            continue
        elif clean.startswith("- "):
            clean = "• " + clean[2:].strip()
        elif clean.startswith("* "):
            clean = "• " + clean[2:].strip()
        elif clean.startswith(">"):
            clean = clean.lstrip(">").strip()

        buffer.append(clean)

    flush()
    flush_table()
    return items


# 表格分隔行：| --- | :---: | --- |
_PAT_TABLE_SEP_CELL = re.compile(r"^:?-{2,}:?$")


def _parse_table_rows(table_lines: List[str]) -> List[List[str]]:
    """把 markdown 表格行解析成二维数组（跳过分隔行，列数对齐）。"""
    rows: List[List[str]] = []
    for ln in table_lines:
        cells = [c.strip() for c in ln.strip().strip("|").split("|")]
        if cells and all(_PAT_TABLE_SEP_CELL.match(c) for c in cells):
            continue  # 分隔行
        cleaned = []
        for c in cells:
            c = re.sub(r"\*\*(.+?)\*\*", r"\1", c)  # 去 **加粗**
            c = c.replace("`", "")
            cleaned.append(c)
        rows.append(cleaned)
    # 列数对齐
    if rows:
        ncol = max(len(r) for r in rows)
        for r in rows:
            while len(r) < ncol:
                r.append("")
    return rows


def render_to_docx(
    items: List[Tuple[str, object]],
    output_path: Path,
    *,
    strict: bool = False,
    base_dir: Path = None,
) -> List[dict]:
    """把 [(role, content), ...] 渲染成符合国标的 docx。

    支持 TABLE / IMAGE 特殊条目。base_dir 用于解析 markdown 里的相对图片路径。
    返回：变更清单（每段一条 dict），用于生成 .md 变更报告。
    """
    from docx import Document
    from docx.shared import Mm
    from dataclasses import asdict

    # 加入 scripts 目录到 sys.path 以便 import styles
    scripts_dir = Path(__file__).resolve().parent
    if str(scripts_dir) not in sys.path:
        sys.path.insert(0, str(scripts_dir))

    from rewrites import styles

    base = base_dir or Path.cwd()

    # ---- 1. 创建空白 docx + 应用节属性 ----
    doc = Document()
    sec = doc.sections[0]
    sec.page_width = Mm(210)
    sec.page_height = Mm(297)
    sec.top_margin = Mm(37 if strict else 34.58)
    sec.bottom_margin = Mm(35 if strict else 32.58)
    sec.left_margin = Mm(28)
    sec.right_margin = Mm(26)

    # ---- 2. 应用节属性的变更记录 ----
    changes: List[dict] = []
    changes.append({
        "type": "section",
        "role": "SECTION",
        "before": f"默认 A4 / 默认边距",
        "after": (
            f"A4 (210×297) / 上下白边 "
            f"{(37 if strict else 34.58):.2f}/{(35 if strict else 32.58):.2f}mm "
            f"/ 左右 28/26mm"
        ),
        "rationale": "GB/T 9704-2012 §6.1（{}坐标系）".format("严格" if strict else "33476.2 软件"),
    })

    # ---- 3. 按 role 添加段落/表格/图片 + 套样式 ----
    # 注意：doc.paragraphs 只统计顶层段落，TABLE 不占段落索引，
    #       因此用 p_idx 独立计数
    p_idx = 0
    for role, content in items:
        if role == "EMPTY":
            doc.add_paragraph()
            p_idx += 1
            changes.append({
                "type": "paragraph",
                "role": "EMPTY",
                "before": "—",
                "after": "空段",
                "rationale": "占位空段",
            })
            continue

        if role == "TABLE":
            rows = content  # List[List[str]]
            if not rows:
                continue
            n_rows, n_cols = len(rows), len(rows[0])
            table = doc.add_table(rows=n_rows, cols=n_cols)
            try:
                table.style = "Table Grid"  # 带边框
            except Exception:
                pass
            for r, row in enumerate(rows):
                for c, val in enumerate(row):
                    if c < n_cols:
                        table.rows[r].cells[c].text = val
            changes.append({
                "type": "table",
                "role": "TABLE",
                "before": f"markdown 表格 {n_rows}×{n_cols}",
                "after": "Word 表格（Table Grid 边框；随后统一定制样式）",
                "rationale": "表格转 Word 原生表格，表头黑体、数据仿宋、整体居中（排版惯例）。",
            })
            continue

        if role == "IMAGE":
            alt, img_path = content
            resolved = Path(img_path)
            if not resolved.is_absolute():
                resolved = base / resolved
            if resolved.exists():
                p = doc.add_paragraph()
                p_idx += 1
                run = p.add_run()
                try:
                    run.add_picture(str(resolved))
                    # 超出版心宽（约 156mm）则等比缩到 150mm
                    MAX_W_MM = 150
                    pic = doc.inline_shapes[-1]
                    if pic.width > Mm(MAX_W_MM):
                        ratio = Mm(MAX_W_MM) / pic.width
                        pic.height = int(pic.height * ratio)
                        pic.width = int(Mm(MAX_W_MM))
                    # 套图片段样式（居中、单倍行距）
                    ch_list = styles.apply_role_to_paragraph(
                        doc, paragraph_index=p_idx - 1, role="IMAGE", text="",
                    )
                    for ch in ch_list:
                        changes.append(asdict(ch))
                    # alt 非空 → 自动生成图题段
                    if alt:
                        cap = doc.add_paragraph(alt)
                        p_idx += 1
                        ch_list = styles.apply_role_to_paragraph(
                            doc, paragraph_index=p_idx - 1, role="CAPTION", text=alt,
                        )
                        for ch in ch_list:
                            changes.append(asdict(ch))
                except Exception as e:
                    p.add_run(f"[图片插入失败: {img_path} ({e})]")
            else:
                p = doc.add_paragraph(f"[图片缺失: {img_path}]")
                p_idx += 1
                changes.append({
                    "type": "paragraph",
                    "role": "IMAGE",
                    "before": f"![]({img_path})",
                    "after": f"占位文本（文件不存在：{resolved}）",
                    "rationale": "图片路径无法解析，保留占位避免丢内容。",
                })
            continue

        text = content  # 普通段落
        p = doc.add_paragraph()
        p.add_run(text)
        p_idx += 1

        ch_list = styles.apply_role_to_paragraph(
            doc,
            paragraph_index=p_idx - 1,
            role=role,
            text=text,
            normalize_dates=True,
        )

        for ch in ch_list:
            changes.append(asdict(ch))

    # ---- 4. 统一表格样式（表头黑体/数据仿宋/居中/单倍行距）----
    for ch in styles.format_tables(doc):
        changes.append(asdict(ch))

    # ---- 5. 保存 ----
    doc.save(str(output_path))
    return changes


def main() -> None:
    """CLI 入口：python md_parser.py input.md [-o output.docx] [--strict]"""
    import argparse

    parser = argparse.ArgumentParser(description="Markdown → 公文 docx 转换器")
    parser.add_argument("input", type=Path, help="输入 Markdown 文件路径")
    parser.add_argument("-o", "--output", type=Path, help="输出 docx 路径（默认：<input>-formatted.docx）")
    parser.add_argument("--strict", action="store_true", help="使用 9704 严格坐标（上下白边 37/35）")
    parser.add_argument("--report", type=Path, help="变更报告 Markdown 路径")

    args = parser.parse_args()

    input_path = args.input
    output_path = args.output or input_path.with_name(input_path.stem + "-formatted.docx")
    report_path = args.report or input_path.with_name(input_path.stem + "-md-changes.md")

    if not input_path.exists():
        print(f"❌ 输入文件不存在：{input_path}")
        return

    print(f"📖 解析 Markdown：{input_path}")
    items = parse_markdown(input_path)
    print(f"   → 识别 {len(items)} 个段落（{sorted(set(r for r, _ in items))}）")

    print(f"📝 渲染 docx：{output_path}")
    changes = render_to_docx(items, output_path, strict=args.strict, base_dir=input_path.parent)

    print(f"📋 变更报告：{report_path}")
    _write_report(report_path, input_path, output_path, items, changes)

    print(f"\n✅ 完成。")
    print(f"   docx:   {output_path}")
    print(f"   report: {report_path}")


def _write_report(report_path: Path, input_path: Path, output_path: Path,
                   items: List[Tuple[str, str]], changes: List[dict]) -> None:
    """生成变更报告 Markdown。"""
    lines = []
    lines.append(f"# Markdown → 公文 docx 排版变更报告")
    lines.append("")
    lines.append(f"- 源文件：`{input_path}`")
    lines.append(f"- 产物：`{output_path}`")
    lines.append(f"- 段落数：{len(items)}")
    lines.append(f"- 角色种类：{len(set(r for r, _ in items))}（{', '.join(sorted(set(r for r, _ in items)))}）")
    lines.append(f"- 变更条数：{len(changes)}")
    lines.append("")
    lines.append("## 角色识别清单")
    lines.append("")
    lines.append("| # | 角色 | 文本（前 40 字）|")
    lines.append("|---|---|---|")
    for i, (role, text) in enumerate(items):
        # 特殊条目转摘要文本
        if role == "TABLE":
            rows = text
            show = f"表格 {len(rows)} 行 × {len(rows[0]) if rows else 0} 列"
        elif role == "IMAGE":
            show = f"图片: {text[1]}"
        elif isinstance(text, str):
            show = (text[:40] + "…") if len(text) > 40 else text
        else:
            show = str(text)
        show = str(show).replace("|", "\\|").replace("\n", " ")
        lines.append(f"| {i+1} | `{role}` | {show} |")
    lines.append("")
    lines.append("## 变更详情")
    lines.append("")
    for i, ch in enumerate(changes, 1):
        lines.append(f"### 变更 {i}：{ch['role']}")
        lines.append(f"- **改前**：{ch['before']}")
        lines.append(f"- **改后**：{ch['after']}")
        lines.append(f"- **依据**：{ch['rationale']}")
        lines.append("")

    report_path.write_text("\n".join(lines), encoding="utf-8")


if __name__ == "__main__":
    main()
