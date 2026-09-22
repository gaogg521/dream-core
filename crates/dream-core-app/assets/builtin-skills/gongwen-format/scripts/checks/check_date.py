"""
check_date — 日期双轨校验

依据：GB/T 9704-2012 §8.3（成文日期数字用法）+ GB/T 33476.1-2016 §6.2

规则：
- 正文与 XML 结构：阿拉伯数字、年份标全称、**月日不编虚位**
  正确：2024年1月5日（1月不写01月，5日不写05日）
  错误：2024年01月05日；2024年1月05日；24年1月5日（年份未标全称）
- 二维条码 D10/D13：YYYYMMDD 共 8 位，**单数月日必须补零**
  正确：20240105
  错误：2024115；20241015

判定：定位段落里的"YYYY年M月D日"模式，M/D 任一为个位数但被补零 → ERROR。
条码字段另由 egongwen-barcode 处理，本检查器不涉及条码。
"""
from __future__ import annotations

import re
from typing import List

from report import Issue, Severity, CheckResult

CHECK_ID = "date-number"
CHECK_NAME = "日期双轨校验"

# 匹配 "2024年1月5日" / "2024年01月05日" / "24年1月5日"
PAT_FULL = re.compile(r"(\d{2,4})年(\d{1,2})月(\d{1,2})日")
# 匹配短年份（两位数）—— GB/T 9704 §8.3 不允许
PAT_SHORT_YEAR = re.compile(r"\b\d{2}年\d{1,2}月\d{1,2}日")


def check_paragraph(text: str, paragraph_index: int) -> List[Issue]:
    issues: List[Issue] = []
    if not text:
        return issues

    for m in PAT_FULL.finditer(text):
        year_str, month_str, day_str = m.group(1), m.group(2), m.group(3)
        # 1) 月/日是否补零？
        if month_str.startswith("0") and len(month_str) == 2 and month_str != "00":
            issues.append(Issue(
                check_id=CHECK_ID,
                rule_id="DATE-MONTH-PADDED",
                severity=Severity.ERROR,
                title=f"月份补零：'{month_str}月'（应为 '{int(month_str):d}月'）",
                message="GB/T 9704-2012 §8.3：成文日期中的月、日不编虚位。",
                location=f"第 {paragraph_index+1} 段",
                actual=f"{year_str}年{month_str}月{day_str}日",
                expected=f"{year_str}年{int(month_str):d}月{int(day_str):d}日" if day_str.startswith("0") else f"{year_str}年{int(month_str):d}月{day_str}日",
                fix=f"将 '{month_str}' 改为 '{int(month_str):d}'；同理检查日。",
                reference="GB/T 9704-2012 §8.3",
            ))
        if day_str.startswith("0") and len(day_str) == 2 and day_str != "00":
            # 避免与月份补零一起重复报——只在月份合规时单独报日的补零
            if not (month_str.startswith("0") and len(month_str) == 2):
                issues.append(Issue(
                    check_id=CHECK_ID,
                    rule_id="DATE-DAY-PADDED",
                    severity=Severity.ERROR,
                    title=f"日期补零：'{day_str}日'（应为 '{int(day_str):d}日'）",
                    message="GB/T 9704-2012 §8.3：成文日期中的月、日不编虚位。",
                    location=f"第 {paragraph_index+1} 段",
                    actual=f"{year_str}年{month_str}月{day_str}日",
                    expected=f"{year_str}年{month_str}月{int(day_str):d}日",
                    fix=f"将 '{day_str}' 改为 '{int(day_str):d}'。",
                    reference="GB/T 9704-2012 §8.3",
                ))

        # 2) 年份是否标全称？
        if len(year_str) == 2:
            issues.append(Issue(
                check_id=CHECK_ID,
                rule_id="DATE-SHORT-YEAR",
                severity=Severity.WARN,
                title=f"年份未标全称：'{year_str}年'",
                message="GB/T 9704-2012 §8.3：年份应标全称。",
                location=f"第 {paragraph_index+1} 段",
                actual=f"{year_str}年{month_str}月{day_str}日",
                expected="完整的 4 位年份",
                fix="使用 4 位年份（如 2024 而非 24）。",
                reference="GB/T 9704-2012 §8.3",
            ))

        # 3) 月日数字范围合理性（1-12 / 1-31）
        try:
            m_int = int(month_str)
            d_int = int(day_str)
            if not (1 <= m_int <= 12):
                issues.append(Issue(
                    check_id=CHECK_ID,
                    rule_id="DATE-INVALID-MONTH",
                    severity=Severity.ERROR,
                    title=f"月份超出 1-12：'{month_str}'",
                    message="月份数字必须在 1-12 范围内。",
                    location=f"第 {paragraph_index+1} 段",
                    actual=month_str,
                    expected="1-12",
                    fix=f"修正月份数字。",
                    reference="GB/T 9704-2012 §8.3",
                ))
            if not (1 <= d_int <= 31):
                issues.append(Issue(
                    check_id=CHECK_ID,
                    rule_id="DATE-INVALID-DAY",
                    severity=Severity.ERROR,
                    title=f"日期超出 1-31：'{day_str}'",
                    message="日期数字必须在 1-31 范围内。",
                    location=f"第 {paragraph_index+1} 段",
                    actual=day_str,
                    expected="1-31",
                    fix=f"修正日期数字。",
                    reference="GB/T 9704-2012 §8.3",
                ))
        except ValueError:
            pass
    return issues


def run(doc, lib) -> CheckResult:
    result = CheckResult(check_id=CHECK_ID, check_name=CHECK_NAME)
    paragraphs = lib.iter_paragraphs(doc)
    any_date = False
    for p in paragraphs:
        if not p.text:
            continue
        if not PAT_FULL.search(p.text):
            continue
        any_date = True
        issues = check_paragraph(p.text, p.paragraph_index)
        for issue in issues:
            result.add(issue)
    if not any_date:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="NO-DATE",
            severity=Severity.INFO,
            title="未发现成文日期",
            message="文档中未匹配到 'YYYY年M月D日' 格式的日期。成文日期是公文必备要素，建议确认是否遗漏。",
            reference="GB/T 9704-2012 §8.3",
        ))
    elif not result.issues:
        result.add(Issue(
            check_id=CHECK_ID,
            rule_id="ALL-PASS",
            severity=Severity.PASS,
            title="日期格式合规",
            message="所有日期均符合 GB/T 9704-2012 §8.3（年份全称、月日不补零）。",
            reference="GB/T 9704-2012 §8.3",
        ))
    return result