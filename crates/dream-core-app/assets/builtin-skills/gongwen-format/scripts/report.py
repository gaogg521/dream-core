"""
Issue 数据结构与报告输出层。

每个检查器产出 Issue 列表，报告模块按严重等级/分组聚合输出 Markdown + JSON。
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field, asdict
from datetime import datetime
from enum import Enum
from typing import List, Dict, Any, Optional


class Severity(str, Enum):
    ERROR = "ERROR"      # 不合规，必须改
    WARN = "WARN"        # 强烈建议改
    INFO = "INFO"        # 可选优化
    PASS = "PASS"        # 已合规（用于正向记录）


SEVERITY_RANK = {Severity.ERROR: 0, Severity.WARN: 1, Severity.INFO: 2, Severity.PASS: 3}


@dataclass
class Issue:
    check_id: str          # 来自哪个检查器
    rule_id: str           # 规则编号
    severity: Severity
    title: str             # 一句话标题
    message: str           # 详细说明（含定位）
    location: str = ""     # 节号/段号定位
    actual: str = ""       # 实际值
    expected: str = ""     # 期望值
    fix: str = ""          # 整改建议
    reference: str = ""    # 出处（如 GB/T 9704-2012 §7.3.1）

    def to_dict(self) -> Dict[str, Any]:
        d = asdict(self)
        d["severity"] = self.severity.value
        return d


@dataclass
class CheckResult:
    """单个检查器的产出。"""
    check_id: str
    check_name: str
    issues: List[Issue] = field(default_factory=list)

    def add(self, issue: Issue):
        self.issues.append(issue)

    def has_error(self) -> bool:
        return any(i.severity == Severity.ERROR for i in self.issues)

    def summary(self) -> Dict[str, int]:
        cnt = {s.value: 0 for s in Severity}
        for i in self.issues:
            cnt[i.severity.value] += 1
        return cnt


def render_markdown(path: str, checks: List[CheckResult]) -> str:
    """生成 Markdown 报告。"""
    lines = []
    lines.append(f"# 公文格式合规检查报告")
    lines.append("")
    lines.append(f"- 文件：`{path}`")
    lines.append(f"- 时间：{datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    total_err = sum(c.summary()["ERROR"] for c in checks)
    total_warn = sum(c.summary()["WARN"] for c in checks)
    total_info = sum(c.summary()["INFO"] for c in checks)
    total_pass = sum(c.summary()["PASS"] for c in checks)
    lines.append(f"- ERROR：{total_err} | WARN：{total_warn} | INFO：{total_info} | PASS：{total_pass}")
    if total_err == 0:
        lines.append("")
        lines.append("> ✅ **未发现 ERROR 级问题**")
    else:
        lines.append("")
        lines.append(f"> ❌ **发现 {total_err} 项 ERROR，必须整改**")
    lines.append("")

    # 摘要表
    lines.append("## 摘要")
    lines.append("")
    lines.append("| 检查器 | ERROR | WARN | INFO | PASS |")
    lines.append("|---|---|---|---|---|")
    for c in checks:
        s = c.summary()
        lines.append(f"| {c.check_name} | {s['ERROR']} | {s['WARN']} | {s['INFO']} | {s['PASS']} |")
    lines.append("")

    # 详情：按检查器分组
    for c in checks:
        if not c.issues:
            continue
        lines.append(f"## {c.check_name} (`{c.check_id}`)")
        lines.append("")
        # 按严重等级排序
        sorted_issues = sorted(c.issues, key=lambda i: SEVERITY_RANK[i.severity])
        for i in sorted_issues:
            badge = {"ERROR": "🔴", "WARN": "🟡", "INFO": "🔵", "PASS": "🟢"}[i.severity.value]
            lines.append(f"### {badge} [{i.severity.value}] {i.title}")
            lines.append("")
            if i.location:
                lines.append(f"- **定位**：{i.location}")
            if i.actual:
                lines.append(f"- **实际**：{i.actual}")
            if i.expected:
                lines.append(f"- **期望**：{i.expected}")
            if i.message:
                lines.append(f"- **说明**：{i.message}")
            if i.fix:
                lines.append(f"- **整改**：{i.fix}")
            if i.reference:
                lines.append(f"- **出处**：{i.reference}")
            lines.append("")
    return "\n".join(lines)


def render_json(path: str, checks: List[CheckResult], doc_summary: Dict[str, Any]) -> str:
    payload = {
        "file": path,
        "generated_at": datetime.now().isoformat(timespec="seconds"),
        "doc_summary": doc_summary,
        "totals": {
            "ERROR": sum(c.summary()["ERROR"] for c in checks),
            "WARN": sum(c.summary()["WARN"] for c in checks),
            "INFO": sum(c.summary()["INFO"] for c in checks),
            "PASS": sum(c.summary()["PASS"] for c in checks),
        },
        "checks": [
            {
                "check_id": c.check_id,
                "check_name": c.check_name,
                "summary": c.summary(),
                "issues": [i.to_dict() for i in c.issues],
            }
            for c in checks
        ],
    }
    return json.dumps(payload, ensure_ascii=False, indent=2)