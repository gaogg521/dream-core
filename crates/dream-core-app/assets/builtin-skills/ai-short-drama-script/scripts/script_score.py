#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""AI短剧剧本 · 十二维爆款结构评分门禁 V2
用法:
  python script_score.py --file <剧本.md>             # 评分
  python script_score.py --text "<剧本文本>"          # 直接评文本
  python script_score.py --file <剧本.md> --optimize   # 评分 + 自动生成优化提示词
  python script_score.py --self-test                  # 自检（高/中/低三档样例）
规则: 十二维 100 分制 + 硬约束扣分；>=95 交付 / 90-94 定向迭代一次重评 / <90 回炉重写。
零依赖，Python 3.8+。
"""
import argparse, json, re, sys

# (key, 名称, 满分, 特征正则列表, 满分所需命中数)
DIMS = [
    ("hook",        "Hook冲击",   10, [r"开场|hook|前\s*\d+\s*秒", r"\d+\s*[-–—]\s*\d+\s*秒|\d+\s*秒"], 2),
    ("pacing",      "结构节奏",   10, [r"场景\s*\d|场次|场\s*\d+\s*[-–]\s*\d+|【场景", r"情绪基调|节奏|时间："], 2),
    ("dialogue",    "台词质量",   10, [r"^[^\s（【][^：\n]{1,12}：", r"！|\？|\?", r"（[^）]*）"], 2),
    ("payoff",      "爽点密度",   10, [r"打脸|反击|震惊|觉醒|反转|亮出|暴露|翻盘|身份揭"], 3),
    ("cliffhanger", "悬念设计",   10, [r"下集钩子|钩子|本集钩子", r"悬念|切断|画面黑|黑屏|待续"], 2),
    ("character",   "人物塑造",    8, [r"人物小传|小传|标签|弧线|命运弧", r"视觉特征|视觉化|外貌|服装|发型", r"动机|人设"], 2),
    ("storyboard",  "分镜适配",    8, [r"景别|特写|全景|中景|近景|远景", r"运镜|推镜|拉镜|摇|移镜|固定"], 2),
    ("consistency", "角色一致性",  8, [r"首次出场|外貌锚点|定妆|角色卡|角色C\d+", r"\d+\s*岁"], 2),
    ("compliance",  "合规安全",    8, [r"AI\s*生成标识|醒目标识", r"工具溯源|溯源材料"], 1),
    ("rhythm",      "节奏控制",    8, [r"场景转换|换景|场景\s*\d"], 1),
    ("retention",   "完播设计",   10, [r"悬念|钩子|反转|危机|爆点", r"完播|留人|情绪峰值"], 1),
    ("policy",      "政策适配",   10, [r"平台|抖音|红果|快手|腾讯视频|爱奇艺|ReelShort", r"分账|完播率|审核|投稿|备案"], 2),
]

BANNED = r"吸毒教程|制毒(方法|过程)|赌博网站|性爱场景|裸露描写|自杀(方法|过程)演示|血腥细节|宣扬迷信"
PAYOFF_WORDS = r"打脸|反击|震惊|觉醒|反转|亮出|暴露|翻盘|身份揭"


def _hits(text, patterns):
    return sum(1 for p in patterns if re.search(p, text, re.I | re.M))


def score_text(text: str):
    total, details = 0, []
    for key, name, full, patterns, need in DIMS:
        hits = _hits(text, patterns)
        if key == "retention":
            nodes = len(re.findall(patterns[0], text))
            density = nodes / (max(len(text), 1) / 300)
            s = full if (hits >= need and density >= 1.0) else round(full * min(density, 1.0))
        elif key == "payoff":
            s = min(full, 3 * len(re.findall(PAYOFF_WORDS, text)))
        else:
            s = round(full * min(hits, need) / need)
        total += s
        details.append({"dim": name, "score": s, "max": full})

    penalties = []
    long_lines = [l for l in text.splitlines()
                  if re.match(r"^[^\s（【#|>]{1,12}：", l) and len(l.split("：", 1)[-1]) > 25]
    if long_lines:
        p = min(10, 2 * len(long_lines))
        penalties.append({"item": "台词超长句(>25字)", "count": len(long_lines), "penalty": -p})
        total -= p

    scenes = len(re.findall(r"场景\s*\d|场次|【场景", text))
    if scenes > 3:
        penalties.append({"item": "场景转换超过3次", "count": scenes, "penalty": -3})
        total -= 3

    if re.search(BANNED, text):
        total = 0
        penalties.append({"item": "命中合规违禁词", "count": 1, "penalty": "一票否决(总分归零)"})

    total = max(0, min(100, total))
    verdict = ("PASS(可交付)" if total >= 95
               else "CONDITIONAL(90-94 定向迭代一次重评)" if total >= 90
               else "FAIL(回炉重写)")
    return {"total": total, "verdict": verdict, "details": details,
            "penalties": penalties, "advice": _advice(details, penalties),
            "optimize_prompt": _optimize(details, penalties)}


def _advice(details, penalties):
    weak = [d for d in details if d["max"] and d["score"] < d["max"] * 0.6]
    parts = []
    if weak:
        parts.append("薄弱维度：" + "、".join(f"{d['dim']}({d['score']}/{d['max']})" for d in weak))
    for p in penalties:
        parts.append(f"{p['item']} ×{p['count']} → {p['penalty']}")
    return ("；".join(parts) + " → 按对应 references 补齐后重评。") if parts else "全部维度达标，保持。"


FIXES = {
    "Hook冲击": "重写开场：用身份反转/绝境困局/隐藏身份/倒计时/惊天揭露五式之一，并在场景标题标注 (0-5秒)。",
    "结构节奏": "按 0-5 秒 Hook / 5-15 秒升级 / 15-45 秒对峙 / 45-55 秒高潮 / 55-60 秒悬念切断 重排，每段标注时间轴。",
    "台词质量": "台词压到单句 ≤15 字，删除解释性长句，把说明性内容改为动作或道具。",
    "爽点密度": "每集至少 3 个爽点点位（打脸/反击/反转/身份揭露），每 15 秒一次信息增量。",
    "悬念设计": "结尾补「▶ 下集钩子」，明确到最后一个画面；从 8 类悬念中轮换选用。",
    "人物塑造": "补人物小传（3-5 行：标签 + 2 个视觉化特征 + 命运弧线 3 转折点）。",
    "分镜适配": "分镜表补齐景别（远景/全景/中景/近景/特写）与运镜（推/拉/摇/移/固定）两列。",
    "角色一致性": "角色首镜锁定外貌锚点（年龄/脸型/发型/服装/标志特征），后续每镜粘贴完全相同描述，禁止同义改写。",
    "合规安全": "补「AI 生成标识」与「AI 工具溯源材料」清单（剧本/画面/配音分别注明）。",
    "节奏控制": "压缩场景转换至 ≤3 次/集，一场戏控制在 1-2 个主要场景。",
    "完播设计": "提高悬念密度至每 300 字 ≥1 个节点，并按目标平台完播口径（红果≥3秒/抖音≥70%/快手≥30%）补留人设计。",
    "政策适配": "补政策校准注记：目标平台分账/审核/投稿要求 + 备案与 AI 标识提示。",
}


def _optimize(details, penalties):
    lines = ["【自动生成的优化提示词】请按以下顺序逐项重写后再评："]
    i = 1
    for d in details:
        if d["score"] < d["max"]:
            lines.append(f"{i}. [{d['dim']} {d['score']}/{d['max']}] {FIXES.get(d['dim'], '按 references 补齐。')}")
            i += 1
    for p in penalties:
        lines.append(f"{i}. [硬约束] {p['item']} ×{p['count']} —— 必须修正（{p['penalty']}）")
        i += 1
    if i == 1:
        lines.append("全部维度达标，无需优化。")
    return "\n".join(lines)


HIGH_SAMPLE = """# 第 1 集：《弃女归来》（时长：75秒）｜平台：抖音｜AI 生成标识：是｜工具溯源：WorkBuddy/即梦/剪映

## 人物小传
苏若，22岁，被弃真千金。视觉特征：洗旧运动服、右眼下方浅色小痣。命运弧线：隐忍→亮证→接管。

## 场景 1｜内景 - 苏家宴会厅 - 夜（0-15秒 开场Hook）
【情绪基调：压抑→爆发】【景别：全景→特写】【运镜：缓慢推镜】【时长：4s】
（苏若，22岁，洗旧运动服，短发素颜，眼神克制。首次出场。）
苏雪：（轻蔑）你也配？
苏若：（平静）你说完了？

## 场景 2｜同场（15-55秒 爽点反转）
（苏若亮出继承人社印，全场震惊，打脸开始。特写印章。）
【音效：酒杯碎裂】【景别：特写】【运镜：固定】【时长：3s】

## 结尾（55-75秒 悬念切断）
苏父的手开始颤抖。画面黑。
▶ 下集钩子：印章真伪验证现场，悬念留待第2集。

【完播设计】抖音完播门槛≥70%单集时长，中段每15秒一次信息增量。
【政策注记】抖音分账70%-80%，严重同质化逆袭套路难获推荐，本剧以原创身份差设定规避。
"""

MID_SAMPLE = """# 第 1 集
## 场景 1 内景 宴会厅 夜
苏雪：你也配参加苏家的宴会？我觉得你完全没有任何资格站在这里说话。
苏若：你说完了？
（苏若亮出印章，全场震惊。）
结尾：苏父的手颤抖。
"""

LOW_SAMPLE = """这是一个剧本。
她起床了，今天天气很好。
她吃了早饭，然后出门散步，心情不错。
（完）
"""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--file"); ap.add_argument("--text")
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--optimize", action="store_true")
    a = ap.parse_args()

    if a.self_test:
        res = {k: score_text(v) for k, v in
               [("高分样例", HIGH_SAMPLE), ("中档样例", MID_SAMPLE), ("低分样例", LOW_SAMPLE)]}
        for k, r in res.items():
            print(f"[自检] {k}: {r['total']}分 {r['verdict']}")
        ok = res["高分样例"]["total"] >= 95 and res["中档样例"]["total"] < 95 and res["低分样例"]["total"] < 90
        print("[自检结果]", "PASS" if ok else "FAIL")
        sys.exit(0 if ok else 1)

    text = open(a.file, encoding="utf-8").read() if a.file else (a.text or "")
    if not text.strip():
        print("用法: --file <剧本.md> [--optimize] | --text <文本> | --self-test"); sys.exit(2)
    r = score_text(text)
    if not a.optimize:
        r.pop("optimize_prompt")
    print(json.dumps(r, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
