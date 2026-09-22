"""format.py — 公文 docx 自动排版 CLI 主入口。

把任意 docx 改写为符合 GB/T 9704-2012 + GB/T 33476.2-2016 的公文格式。

用法：
    python scripts/format.py <源docx> [-o <输出docx>] [--strict] [--report <变更报告md>]

示例：
    python scripts/format.py fixtures/bad.docx
    python scripts/format.py D:/通知.docx -o D:/通知-排版后.docx --strict
    python scripts/format.py D:/通知.docx -o D:/通知-排版后.docx --report D:/变更报告.md

排版策略：
- 默认坐标系：GB/T 33476.2 软件坐标（上 34.58 / 下 32.58 mm）
- --strict：GB/T 9704 严格坐标（上 37 / 下 35 mm）
- 自动规整日期为「YYYY年M月D日」（月日不补零）
- 自动套用字号/字体/行距/缩进/对齐
- 不动文本内容（不删字、不改字、不合并段落）
- 不主动改原页脚（避免破坏原页码 / 域）

输出三件套：
1. <输入>-formatted.docx    排版后的新文件
2. <输入>-changes.md        变更清单（人类可读）
3. 终端打印校验对比（before/after ERROR 数）
"""
from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path
from typing import List

HERE = Path(__file__).parent.resolve()
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

from docx import Document

from lib_docx import open_doc, get_doc_summary
from classify import classify_paragraphs, summarize, ClassifiedParagraph
from rewrites.styles import format_document, Change


# ============ 报告渲染 ============
def render_changes_markdown(
    src_path: str,
    out_path: str,
    changes: List[Change],
    classified: List[ClassifiedParagraph],
    summary: dict,
    before_errors: int,
    after_errors: int,
) -> str:
    """渲染变更报告为 Markdown。"""
    lines = []
    lines.append(f"# 公文排版变更报告\n")
    lines.append(f"- **源文件**：`{src_path}`")
    lines.append(f"- **输出文件**：`{out_path}`")
    lines.append(f"- **段落总数**：{summary['total_paragraphs']}")
    lines.append(f"- **低置信段落数**：{summary.get('low_confidence_count', 0)}")
    lines.append("")
    lines.append("## 一、校验对比\n")
    lines.append("| 检查项 | 排版前 | 排版后 |")
    lines.append("|---|---|---|")
    lines.append(f"| ERROR 数 | **{before_errors}** | **{after_errors}** |")
    if before_errors > 0 and after_errors == 0:
        lines.append("")
        lines.append("> 🎉 **全部 ERROR 已清零**，文件符合公文国标核心规则。")
    elif after_errors > 0:
        lines.append("")
        lines.append(f"> ⚠️ **仍有 {after_errors} 个 ERROR 未清零**，详见文末校验详情。")
    lines.append("")

    # 段落角色分布
    lines.append("## 二、段落角色识别结果\n")
    lines.append("| 角色 | 数量 | 说明 |")
    lines.append("|---|---|---|")
    role_docs = {
        "TITLE": "标题",
        "ZHUSONG": "主送机关",
        "BODY": "主体正文",
        "ATT_DESC": "附件说明",
        "SIG_ORG": "落款机关",
        "SIG_DATE": "成文日期",
        "CHAOSONG": "抄送",
        "YINFA": "印发",
        "LAYER_NUM": "层次序数",
        "EMPTY": "空段",
    }
    for role, count in sorted(summary.get("role_counts", {}).items(), key=lambda x: -x[1]):
        label = role_docs.get(role, role)
        lines.append(f"| {role} ({label}) | {count} | — |")
    lines.append("")

    # 逐段识别详情
    lines.append("## 三、逐段识别与变更\n")
    lines.append("| # | 角色 | 置信 | 原字号/字体 | 判定理由 |")
    lines.append("|---|---|---|---|---|")
    for c in classified:
        text_preview = (c.text or "").replace("|", "‖").replace("\n", " ")[:30]
        size_font = f"{c.original_size_pt or '?'}/{c.original_font or '?'}"
        lines.append(
            f"| {c.paragraph_index+1} | {c.role} | {c.confidence} | {size_font} | "
            f"{c.rationale} / `{text_preview}` |"
        )
    lines.append("")

    # 变更清单
    lines.append("## 四、变更清单\n")
    if not changes:
        lines.append("无变更（文件已合规）。")
    else:
        # 按类型分组
        sec_changes = [c for c in changes if c.change_type == "section"]
        para_changes = [c for c in changes if c.change_type == "paragraph"]
        date_changes = [c for c in changes if c.change_type == "date-normalize"]
        table_changes = [c for c in changes if c.change_type == "table"]

        if table_changes:
            lines.append("### 4.0 表格美化\n")
            lines.append("| 表格 | 改前 | 改后 | 依据 |")
            lines.append("|---|---|---|---|")
            for c in table_changes:
                lines.append(f"| {c.before} | — | {c.after} | {c.rationale[:50]} |")
            lines.append("")

        if sec_changes:
            lines.append("### 4.1 节属性变更\n")
            lines.append("| 项目 | 改前 | 改后 | 依据 |")
            lines.append("|---|---|---|---|")
            for c in sec_changes:
                lines.append(f"| {c.rationale[:40]} | {c.before} | {c.after} | GB/T 9704-2012 §6.1 |")
            lines.append("")

        if para_changes:
            lines.append("### 4.2 段落属性变更\n")
            lines.append("| # | 角色 | 改前 | 改后 | 依据 |")
            lines.append("|---|---|---|---|---|")
            for c in para_changes:
                lines.append(f"| {c.paragraph_index+1} | {c.role} | {c.before} | {c.after} | {c.rationale[:60]} |")
            lines.append("")

        if date_changes:
            lines.append("### 4.3 日期格式规整\n")
            lines.append("| # | 改前 | 改后 | 依据 |")
            lines.append("|---|---|---|---|")
            for c in date_changes:
                lines.append(f"| {c.paragraph_index+1} | {c.before} | {c.after} | {c.rationale} |")
            lines.append("")

    # 已知未覆盖项
    lines.append("## 五、已知未覆盖项（请手工核对）\n")
    lines.append("1. **页脚 PAGE 域**：本版本不主动插入页码域，避免破坏原页脚。请在 Word/WPS 中手动设置：4 号宋体、单页码居右、双页码居左。")
    lines.append("2. **印章本身**：本工具只校验/调整落款日期的右空字数，不探测印章是否加盖。")
    lines.append("3. **发文字号**：本版本不主动改正文中的发文字号格式（如发现〔〕被写成 []，建议手工改）。")
    lines.append("4. **分页位置**：22 行/页 与 28 字/行的实际行数需在 Word 中目测核对（python-docx 不渲染字体）。")
    lines.append("5. **特殊格式公文**（信函、命令、纪要）的专属版式暂不支持。")
    lines.append("")

    return "\n".join(lines)


# ============ 校验闭环 ============
def count_errors(doc_path: str) -> int:
    """跑全量 check，统计 ERROR 数。"""
    from check import run_all
    from report import Severity
    results, _ = run_all(doc_path)
    err = 0
    for r in results:
        for issue in r.issues:
            if issue.severity == Severity.ERROR:
                err += 1
    return err


# ============ 主流程 ============
def main():
    parser = argparse.ArgumentParser(description="公文 docx 自动排版工具")
    parser.add_argument("source", help="源 docx 文件路径")
    parser.add_argument("-o", "--output", help="输出 docx 路径（默认：<源>-formatted.docx）")
    parser.add_argument("--strict", action="store_true",
                        help="使用 GB/T 9704 严格坐标（上下 37/35mm），默认用 33476.2 软件坐标")
    parser.add_argument("--no-date-normalize", action="store_true",
                        help="不规整日期格式（默认规整）")
    parser.add_argument("--report", help="变更报告 Markdown 路径")
    parser.add_argument("--quiet", action="store_true", help="不打印中间日志")
    args = parser.parse_args()

    src = Path(args.source)
    if not src.exists():
        print(f"❌ 源文件不存在：{src}", file=sys.stderr)
        sys.exit(2)

    if args.output:
        out = Path(args.output)
    else:
        out = src.with_name(src.stem + "-formatted.docx")

    if args.report:
        report_path = Path(args.report)
    else:
        report_path = src.with_name(src.stem + "-changes.md")

    def log(msg):
        if not args.quiet:
            print(msg)

    log(f"📖 读取：{src}")
    # 排版前先校验
    before_errors = count_errors(str(src))
    log(f"   排版前 ERROR 数：{before_errors}")

    # 读取 + 识别 + 改写
    doc = open_doc(str(src))
    paragraphs_view = list(__import__("lib_docx").iter_paragraphs(doc))
    log(f"   共 {len(paragraphs_view)} 个段落，开始识别角色…")

    classified = classify_paragraphs(paragraphs_view)
    summary = summarize(classified)
    log(f"   角色分布：{summary['role_counts']}")

    changes = format_document(
        doc,
        classified,
        strict=args.strict,
        normalize_dates=not args.no_date_normalize,
    )
    log(f"   共 {len(changes)} 项变更")

    # 保存
    doc.save(str(out))
    log(f"💾 保存：{out}")

    # 排版后再校验
    after_errors = count_errors(str(out))
    log(f"   排版后 ERROR 数：{after_errors}")

    # 报告
    md = render_changes_markdown(
        src_path=str(src),
        out_path=str(out),
        changes=changes,
        classified=classified,
        summary=summary,
        before_errors=before_errors,
        after_errors=after_errors,
    )
    report_path.write_text(md, encoding="utf-8")
    log(f"📝 变更报告：{report_path}")

    # 退出码：有 ERROR → 1；全清零 → 0
    sys.exit(1 if after_errors > 0 else 0)


if __name__ == "__main__":
    main()
