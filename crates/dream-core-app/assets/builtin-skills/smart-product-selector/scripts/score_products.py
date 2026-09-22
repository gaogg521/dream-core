#!/usr/bin/env python3
"""
选品评分工具 - 五维度加权评分模型
读取候选品类数据，计算综合得分并排名

用法:
  python score_products.py --csv scores.csv
  python score_products.py --csv scores.csv --json
  python score_products.py --csv scores.csv --weights "0.3,0.2,0.25,0.3,0.15"

CSV 必须包含列:
  product_name, market_score, trend_score, competition_score, profit_score, supply_score

所有 score 列取值 1-5（1=最差，5=最优）
可选列: notes（备注）
"""

import argparse
import csv
import json
import sys

DEFAULT_WEIGHTS = {
    "market": 0.30,
    "trend": 0.20,
    "competition": 0.25,
    "profit": 0.30,
    "supply": 0.15,
}

DIM_LABELS = {
    "market_score": "市场容量",
    "trend_score": "增长趋势",
    "competition_score": "竞争进入",
    "profit_score": "利润空间",
    "supply_score": "供应链",
}

SCORE_COLS = list(DIM_LABELS.keys())


def recommend(score: float) -> str:
    if score >= 4.0:
        return "强烈推荐"
    elif score >= 3.0:
        return "值得尝试"
    elif score >= 2.0:
        return "谨慎考虑"
    else:
        return "建议放弃"


def tag_emoji(score: float) -> str:
    if score >= 4.0:
        return "🟢"
    elif score >= 3.0:
        return "🔵"
    elif score >= 2.0:
        return "🟡"
    else:
        return "🔴"


def run(csv_path: str, weights: dict, output_json: bool = False):
    with open(csv_path, encoding="utf-8") as f:
        reader = csv.DictReader(f)
        products = list(reader)

    if not products:
        print("CSV 文件为空，请检查数据")
        sys.exit(1)

    # 校验必要列
    required = ["product_name"] + SCORE_COLS
    missing = [c for c in required if c not in products[0]]
    if missing:
        print(f"CSV 缺少必要列: {missing}")
        print(f"需要: {required}")
        sys.exit(1)

    results = []
    for p in products:
        name = p.get("product_name", "未知")
        notes = p.get("notes", "")

        raw_scores = {}
        weighted = 0
        for col in SCORE_COLS:
            try:
                val = float(p[col])
            except (ValueError, KeyError):
                print(f"警告: {name} 的 {col} 值无效，跳过")
                val = 0
            raw_scores[col] = val

        for col in SCORE_COLS:
            dim = col.replace("_score", "")
            w = weights.get(dim, 0)
            weighted += raw_scores[col] * w

        results.append({
            "name": name,
            "scores": raw_scores,
            "weighted": round(weighted, 2),
            "recommendation": recommend(weighted),
            "emoji": tag_emoji(weighted),
            "notes": notes,
        })

    # 按综合分排序
    results.sort(key=lambda x: x["weighted"], reverse=True)

    if output_json:
        print(json.dumps(results, ensure_ascii=False, indent=2))
        return results

    # 格式化输出
    print(f"\n{'='*78}")
    print(f" 选品综合评分排名")
    print(f" 权重: 市场{weights['market']:.0%} | 趋势{weights['trend']:.0%} | "
          f"竞争{weights['competition']:.0%} | 利润{weights['profit']:.0%} | "
          f"供应链{weights['supply']:.0%}")
    print(f"{'='*78}\n")

    # 表头
    header = f"{'排名':^4} {'品类':<18} {'市场':>4} {'趋势':>4} {'竞争':>4} {'利润':>4} {'供应链':>5} {'综合':>5} {'建议':<8}"
    print(header)
    print("-" * 78)

    for i, r in enumerate(results, 1):
        s = r["scores"]
        medal = {1: "🥇", 2: "🥈", 3: "🥉"}.get(i, f" {i}")
        print(
            f"{medal:^4} {r['name']:<18} "
            f"{s['market_score']:>4.0f} {s['trend_score']:>4.0f} "
            f"{s['competition_score']:>4.0f} {s['profit_score']:>4.0f} "
            f"{s['supply_score']:>5.0f} {r['weighted']:>5.2f} "
            f"{r['emoji']}{r['recommendation']}"
        )

    print(f"\n{'─'*78}")
    print(" 评级说明:")
    print("   🟢 ≥4.0 强烈推荐 → 立即着手入场，抢占窗口")
    print("   🔵 3.0-3.9 值得尝试 → 找准差异化切入点，小批量测试")
    print("   🟡 2.0-2.9 谨慎考虑 → 需明确独特竞争优势")
    print("   🔴 <2.0  建议放弃 → 风险大于收益")

    # 输出每个品类的详细分析
    print(f"\n{'='*78}")
    print(" 详细分析")
    print(f"{'='*78}")

    for i, r in enumerate(results, 1):
        s = r["scores"]
        print(f"\n  #{i} {r['emoji']} {r['name']}  (综合得分: {r['weighted']})")
        print(f"  ┌─────────────────────────────────────────┐")
        for col in SCORE_COLS:
            label = DIM_LABELS[col]
            val = s[col]
            bar = "█" * int(val) + "░" * (5 - int(val))
            print(f"  │ {label:<8} {bar} {val:.0f}/5  │")
        print(f"  └─────────────────────────────────────────┘")
        if r["notes"]:
            print(f"  备注: {r['notes']}")

    return results


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description="选品五维度加权评分工具",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
示例:
  python score_products.py --csv scores.csv
  python score_products.py --csv scores.csv --json
  python score_products.py --csv scores.csv --weights "0.3,0.2,0.25,0.3,0.15"

CSV格式:
  product_name,market_score,trend_score,competition_score,profit_score,supply_score,notes
  手机壳,4,3,2,4,5,标品走量
  智能音箱,3,4,2,3,2,需品牌投入
  宠物零食,4,5,3,4,3,高增长赛道

所有 score 列取值 1-5 (1=最差 5=最优)
        """,
    )

    parser.add_argument("--csv", required=True, help="评分数据 CSV 文件")
    parser.add_argument(
        "--weights",
        type=str,
        help="自定义权重(逗号分隔5个): 市场,趋势,竞争,利润,供应链",
    )
    parser.add_argument("--json", action="store_true", help="JSON 格式输出")

    args = parser.parse_args()

    weights = DEFAULT_WEIGHTS.copy()
    if args.weights:
        parts = [float(x) for x in args.weights.split(",")]
        if len(parts) != 5:
            print("错误: 权重必须为5个值")
            sys.exit(1)
        weights = {
            "market": parts[0],
            "trend": parts[1],
            "competition": parts[2],
            "profit": parts[3],
            "supply": parts[4],
        }

    run(args.csv, weights, args.json)
