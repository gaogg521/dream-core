"""
check.py — 公文格式合规检查 CLI 主入口

用法：
    python scripts/check.py <docx文件> [--json] [--out <报告路径>]

示例：
    python scripts/check.py fixtures/sample-通知.docx
    python scripts/check.py fixtures/sample-通知.docx --json --out reports/通知.json

返回码：
    0 = 无 ERROR
    1 = 存在 ERROR
"""
from __future__ import annotations

import argparse
import os
import sys
from typing import List

# 让脚本可作为模块被 import，也可独立运行
HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)

from lib_docx import open_doc, get_doc_summary  # noqa: E402
from report import CheckResult, render_markdown, render_json  # noqa: E402
from checks import CHECKS  # noqa: E402


def run_all(path: str) -> tuple[List[CheckResult], dict]:
    doc = open_doc(path)
    summary = get_doc_summary(doc)
    results = []
    for runner, check_id, check_name in CHECKS:
        try:
            r = runner(doc, sys.modules["lib_docx"])
        except Exception as e:
            # 检查器自身崩溃不能毁掉整个流程
            r = CheckResult(check_id=check_id, check_name=check_name)
            from report import Issue, Severity
            r.add(Issue(
                check_id=check_id, rule_id="CHECK-ERROR",
                severity=Severity.ERROR,
                title=f"检查器 `{check_id}` 执行失败",
                message=f"{type(e).__name__}: {e}",
            ))
        results.append(r)
    return results, summary


def main():
    ap = argparse.ArgumentParser(description="公文格式合规检查（GB/T 9704-2012 + GB/T 33476.2-2016）")
    ap.add_argument("file", help="待检查的 .docx / .wps 文件路径")
    ap.add_argument("--json", action="store_true", help="输出 JSON 报告（默认 Markdown）")
    ap.add_argument("--out", help="报告输出路径，默认 stdout")
    args = ap.parse_args()

    if not os.path.exists(args.file):
        print(f"ERROR: 文件不存在：{args.file}", file=sys.stderr)
        sys.exit(2)

    results, summary = run_all(args.file)

    if args.json:
        out = render_json(args.file, results, summary)
    else:
        out = render_markdown(args.file, results)

    if args.out:
        with open(args.out, "w", encoding="utf-8") as f:
            f.write(out)
        print(f"✅ 报告已写入 {args.out}")
    else:
        print(out)

    # 返回码：有 ERROR 则 1
    has_error = any(any(i.severity.value == "ERROR" for i in r.issues) for r in results)
    sys.exit(1 if has_error else 0)


if __name__ == "__main__":
    main()