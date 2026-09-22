#!/usr/bin/env python3
"""
电商利润计算器
支持单品快速计算和批量CSV分析

用法:
  单品: python profit_calculator.py --cost 20 --price 79 --shipping 5 --commission 5 --ad 15
  批量: python profit_calculator.py --csv products.csv --out result.csv

CSV 必须包含列: product_name, cost, price, shipping
可选列: commission_rate, ad_rate, other_fees, return_rate
"""

import argparse
import csv
import sys
import json


def calculate(
    cost: float,
    price: float,
    shipping: float = 0,
    packaging: float = 0,
    commission_rate: float = 0,
    ad_rate: float = 0,
    other_fees: float = 0,
    return_rate: float = 5,
) -> dict:
    """计算单品利润"""
    commission = price * commission_rate / 100
    ad_fee = price * ad_rate / 100
    return_loss = price * return_rate / 100

    total_cost = cost + shipping + packaging + commission + ad_fee + other_fees + return_loss
    gross_profit = price - total_cost
    margin = (gross_profit / price * 100) if price > 0 else 0
    roi = (gross_profit / total_cost * 100) if total_cost > 0 else 0
    breakeven_units = (total_cost / gross_profit) if gross_profit > 0 else float("inf")

    return {
        "售价": round(price, 2),
        "采购成本": round(cost, 2),
        "物流费": round(shipping, 2),
        "包材费": round(packaging, 2),
        "平台佣金": round(commission, 2),
        "推广费": round(ad_fee, 2),
        "售后损耗": round(return_loss, 2),
        "其他费用": round(other_fees, 2),
        "总成本": round(total_cost, 2),
        "毛利": round(gross_profit, 2),
        "毛利率": f"{round(margin, 1)}%",
        "ROI": f"{round(roi, 1)}%",
        "盈亏平衡": f"{max(1, int(breakeven_units))}单(累计)",
    }


def fmt_table(result: dict) -> str:
    """格式化输出利润表"""
    lines = []
    lines.append("┌──────────────────────────────┐")
    lines.append(f"│  售价: ¥{result['售价']:<10}  总成本: ¥{result['总成本']}")
    lines.append(f"│  毛利: ¥{result['毛利']:<10}  毛利率: {result['毛利率']}")
    lines.append(f"│  ROI:  {result['ROI']:<10}  盈亏平衡: {result['盈亏平衡']}")
    lines.append("└──────────────────────────────┘")
    lines.append("")
    lines.append("成本明细:")
    lines.append(f"  采购成本  ¥{result['采购成本']}")
    lines.append(f"  物流费    ¥{result['物流费']}")
    lines.append(f"  包材费    ¥{result['包材费']}")
    lines.append(f"  平台佣金  ¥{result['平台佣金']}")
    lines.append(f"  推广费    ¥{result['推广费']}")
    lines.append(f"  售后损耗  ¥{result['售后损耗']}")
    lines.append(f"  其他费用  ¥{result['其他费用']}")
    return "\n".join(lines)


def batch(csv_path: str, out_path: str = None):
    """批量处理CSV"""
    with open(csv_path, encoding="utf-8") as f:
        reader = csv.DictReader(f)
        products = list(reader)

    results = []
    for p in products:
        r = calculate(
            cost=float(p.get("cost", 0)),
            price=float(p.get("price", 0)),
            shipping=float(p.get("shipping", 0)),
            packaging=float(p.get("packaging", 0)),
            commission_rate=float(p.get("commission_rate", 0)),
            ad_rate=float(p.get("ad_rate", 0)),
            other_fees=float(p.get("other_fees", 0)),
            return_rate=float(p.get("return_rate", 5)),
        )
        r["product_name"] = p.get("product_name", "")
        results.append(r)

    # 按毛利率排序
    results.sort(key=lambda x: float(x["毛利率"].rstrip("%")), reverse=True)

    # 输出汇总
    print(f"\n{'='*70}")
    print(f" 利润分析汇总（共 {len(results)} 个产品）")
    print(f"{'='*70}\n")
    print(f"{'产品':<20} {'售价':>8} {'毛利':>8} {'毛利率':>8} {'评级':>6}")
    print("-" * 70)

    for r in results:
        margin_val = float(r["毛利率"].rstrip("%"))
        if margin_val >= 45:
            tag = "优秀"
        elif margin_val >= 30:
            tag = "良好"
        elif margin_val >= 15:
            tag = "一般"
        else:
            tag = "⚠️差"
        print(f"{r['product_name']:<20} ¥{r['售价']:>6} ¥{r['毛利']:>6} {r['毛利率']:>7} {tag:>6}")

    # 导出CSV
    if out_path:
        fieldnames = ["product_name", "售价", "采购成本", "物流费", "包材费",
                      "平台佣金", "推广费", "售后损耗", "其他费用", "总成本",
                      "毛利", "毛利率", "ROI", "盈亏平衡"]
        with open(out_path, "w", newline="", encoding="utf-8-sig") as f:
            writer = csv.DictWriter(f, fieldnames=fieldnames)
            writer.writeheader()
            writer.writerows(results)
        print(f"\n结果已导出: {out_path}")

    return results


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description="电商利润计算器",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
示例:
  单品计算:
    python profit_calculator.py --cost 20 --price 79 --shipping 5 --commission 5 --ad 15

  批量计算:
    python profit_calculator.py --csv products.csv --out result.csv

CSV格式 (只需 product_name,cost,price,shipping 为必填):
    product_name,cost,price,shipping,packaging,commission_rate,ad_rate,other_fees,return_rate
    手机壳,5,29.9,3,1,5,20,0,8
    数据线,8,39.9,3,0.5,5,15,0,5
        """,
    )

    parser.add_argument("--cost", type=float, help="采购成本 (元)")
    parser.add_argument("--price", type=float, help="售价 (元)")
    parser.add_argument("--shipping", type=float, default=0, help="物流费 (默认 0)")
    parser.add_argument("--packaging", type=float, default=0, help="包材费 (默认 0)")
    parser.add_argument("--commission", type=float, default=0, help="平台佣金率 %% (默认 0)")
    parser.add_argument("--ad", type=float, default=0, help="推广费率 %% (默认 0)")
    parser.add_argument("--other", type=float, default=0, help="其他费用 (默认 0)")
    parser.add_argument("--return-rate", type=float, default=5, help="售后退货损耗率 %% (默认 5)")
    parser.add_argument("--csv", type=str, help="批量处理 CSV 文件路径")
    parser.add_argument("--out", type=str, help="批量结果输出 CSV 路径")
    parser.add_argument("--json", action="store_true", help="以 JSON 格式输出")

    args = parser.parse_args()

    if args.csv:
        batch(args.csv, args.out)
    elif args.cost and args.price:
        result = calculate(
            cost=args.cost,
            price=args.price,
            shipping=args.shipping,
            packaging=args.packaging,
            commission_rate=args.commission,
            ad_rate=args.ad,
            other_fees=args.other,
            return_rate=args.return_rate,
        )
        if args.json:
            print(json.dumps(result, ensure_ascii=False, indent=2))
        else:
            print(f"\n{'='*40}")
            print(" 利润计算结果")
            print(f"{'='*40}\n")
            print(fmt_table(result))
    else:
        parser.print_help()
        print("\n提示: 使用 --cost + --price 做单品计算，或 --csv 批量分析")
        sys.exit(1)
