#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""price_compare.py — 商品比价（v1.0.1）
输入多商品价格，输出最低价与性价比建议。纯标准库。
用法: python price_compare.py "商品A:10 商品B:8 商品C:12"
"""
import re
import sys


def compare(text):
    """解析 "名称:价格 名称:价格" → 排序输出。"""
    items = []
    for m in re.finditer(r"([\u4e00-\u9fa5A-Za-z0-9]+)\s*[:：]\s*([\d.]+)", text):
        items.append((m.group(1), float(m.group(2))))
    if not items:
        return {"error": "未解析到商品价格，格式: 商品A:10 商品B:8"}
    items.sort(key=lambda x: x[1])
    cheapest = items[0]
    return {
        "items": [{"name": n, "price": p} for n, p in items],
        "cheapest": {"name": cheapest[0], "price": cheapest[1]},
        "suggestion": f"最低价: {cheapest[0]} ({cheapest[1]}元)，可优先采购"
    }


def main():
    text = " ".join(sys.argv[1:])
    if not text:
        print("用法: python price_compare.py \"商品A:10 商品B:8\"")
        return 1
    import json
    print(json.dumps(compare(text), ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
