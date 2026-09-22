#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""平台文案「纯文本可直接复制粘贴版」生成器。
微信用 HTML 版；小红书/今日头条编辑器只认纯文本+手动插图，故生成本 .txt：
- 去掉 #/**/表格符/分隔线等 markdown 标记
- 保留 emoji
- 表格转为可读纯文本（两列用「左：右」）
- 在应插图处插入简短 🖼️ 标记，方便在 App 内点「+图片」后删除
用法：python3 build_plaintext.py <平台md路径> [platform]
  platform ∈ xhs / toutiao
"""
import sys, os, re, json

import os
SHARED = os.environ.get("BID_SHARED_DIR", os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# 注：原系统硬编码 /home/laoty/.openclaw/workspace-bid-shared，已参数化为 BID_SHARED_DIR（未设置时回退到本脚本所在目录）

def load_meta():
    try:
        return json.load(open(os.path.join(SHARED, "meta.json"), encoding="utf-8"))
    except Exception:
        return {}

def strip_md_inline(s):
    s = re.sub(r"\*\*(.+?)\*\*", r"\1", s)
    s = re.sub(r"(?<!\*)\*(?!\*)(.+?)\*(?!\*)", r"\1", s)
    s = s.replace("`", "")
    return s

def norm_hashtag_line(s):
    """话题行归一化：单个标签去空格；一行多个标签则保留标签间空格、只去标签内部空格。"""
    s = strip_md_inline(s).strip()
    parts = [p.strip() for p in re.split(r"\s+(?=#)", s) if p.strip()]
    if len(parts) <= 1:
        return s.replace(" ", "")
    return " ".join(p.replace(" ", "") for p in parts)

def table_row_to_text(cells):
    cells = [strip_md_inline(c).strip() for c in cells]
    if len(cells) == 2:
        return f"{cells[0]}：{cells[1]}"
    return "  ".join(cells)

def convert(md_path, platform):
    # 小红书视频简介：源是 scenes_xhs_*.json（非 md），直接提取 narr 口播作视频简介
    if platform == "xhs_video":
        return _convert_xhs_video(md_path)

    raw_lines = open(md_path, encoding="utf-8").read().splitlines()
    meta = load_meta()

    # 1) 去掉开头 front-matter（仅当文件以 **发布时间 等 bullet 开头时，跳到首个 ---；
    #    否则 start=0，保留标题块——避免把「标题块与正文之间的 ---」误当 front-matter 分隔）
    start = 0
    if raw_lines and raw_lines[0].strip().startswith("**"):
        for j, ln in enumerate(raw_lines):
            if ln.strip() == "---":
                start = j + 1
                break
    lines = raw_lines[start:]

    title = None
    body = []
    hashtags = []
    state = "body"          # body | title | posttip | hashtags
    for ln in lines:
        s = ln.strip()
        if s == "":
            continue
        if s == "---":
            if state == "body":
                body.append("")
            continue
        # markdown 标题：#{1,3} 后跟空白或【；话题标签 #xxx 不属此列，不得剥掉井号
        is_heading = bool(re.match(r"^#{1,3}(?:\s|【)", ln.lstrip()))
        if state == "hashtags" and not is_heading and s.startswith("#"):
            hashtags.append(norm_hashtag_line(s))
            continue
        m = re.match(r"^#{1,3}\s*(.*)$", ln) if is_heading else None
        if m:
            txt = m.group(1).strip()
            if txt.startswith("【") and txt.endswith("】"):
                marker = txt[1:-1]
                if marker.startswith("标题"):
                    state = "title"; continue
                elif marker == "发布提示":
                    state = "posttip"; continue
                elif marker.startswith("话题标签"):
                    state = "hashtags"; continue
                elif marker == "正文":
                    state = "body"; continue
                else:
                    state = "body"; continue
            else:
                # 纯标记行（# 正文 / # 正文（头条版）等）不落地为文本
                if re.match(r"^正文", txt):
                    state = "body"; continue
                # 头条 A/B 测试子标题：## 标题 A / ## 标题 B
                if txt == "标题 A":
                    state = "title"; title = "__PENDING__"; continue
                if txt == "标题 B":
                    state = "body"; continue
                state = "body"
                body.append(strip_md_inline(txt)); continue
        # 非标题行：按当前 state 处理
        if state == "title":
            if platform == "toutiao":
                # A/B 测试块：A 版行本身 + 紧随其后的标题行（可能分两行）
                # 已取到真实标题后不再覆写；引用/说明行（> 开头）即使提到“A 版”也不视为标题行
                if title not in (None, "__PENDING__"):
                    continue
                if s.startswith(">"):
                    continue
                if re.match(r"^\**A 版", s):
                    a_parts = s.split("**")
                    a_real = [p for p in a_parts if p.strip() and "A 版" not in p and "B 版" not in p]
                    if a_real and a_real[-1].strip() not in ("", "：", ":"):
                        # 2026-08-12 缺陷修复：A 版加粗标记「**A 版（…）：**」拆分后末段会带前导全角冒号，
                        # 作为标题首字符会渲染成「：43号令…」——剥除前导冒号，标题才干净。
                        title = strip_md_inline(a_real[-1]).strip().lstrip("：:")
                    else:
                        # 标题在下一行
                        title = "__PENDING__"
                elif title == "__PENDING__" and s.strip():
                    title = strip_md_inline(s).strip()
                continue
            else:
                if title is None and s:
                    title = strip_md_inline(s).strip()
                continue
        if state == "posttip":
            continue
        if state == "hashtags":
            hashtags.append(norm_hashtag_line(s))
            continue
        # state == body
        if "|" in s and not re.match(r"^[\|\-\:\s]+$", s):
            cells = [c for c in s.split("|") if c.strip() != ""]
            if len(cells) >= 2:
                body.append("· " + table_row_to_text(cells))
                if platform == "toutiao" and ("评标时长" in s or "评标劳务" in s):
                    body.append("🖼️（此处建议插入「报酬计费表」配图）")
            continue
        if re.match(r"^[\|\-\:\s]+$", s):
            continue
        # 引用块：去掉开头的 "> "，保留文字
        s = re.sub(r"^>\s*", "", s)
        lm = re.match(r"^[-*]\s+(.*)$", s)
        if lm:
            body.append("· " + strip_md_inline(lm.group(1)).strip()); continue
        body.append(strip_md_inline(s))

    if platform == "xhs":
        body = insert_image_markers(body, "xhs_image_markers", DEFAULT_XHS_MARKERS, md_path)
    elif platform == "toutiao":
        body = insert_image_markers(body, "toutiao_image_markers", [], md_path)
        # 头条优先采用 Phase 4 推荐标题（若 meta 有）
    if platform == "toutiao":
        rec = (meta.get("title_optimization_suggestions", {}).get("toutiao")
               or meta.get("platform_titles", {}).get("toutiao_alt_review"))
        if rec:
            title = rec
    elif platform in ("douyin", "shipinhao"):
        # 抖音/视频号：md 为「口播稿」样式（第一行【平台口播稿 · 主题】，正文含镜头N/快节奏前缀）
        # 发布文案 = 视频简介/描述：去掉分镜提示词、保留口播要点、补通用话题标签
        title, body = _extract_video_desc(title, body, platform, md_path)

    final = []
    if title and title != "__PENDING__":
        final.append(title)
        final.append("")
    final.extend(body)
    if hashtags:
        final.append("")
        final.append(" ".join(hashtags))
    return "\n".join(final).strip() + "\n"

# 抖音/视频号口播稿 → 视频简介提取器（2026-08-19 新增）
# 通用话题标签：仅用与账号定位强相关、不涉具体政策的固定标签，避免编造
VIDEO_DEFAULT_HASHTAGS = {
    "douyin": "#评标专家 #专家入库 #副业搞钱 #招投标",
    "shipinhao": "#评标专家 #专家入库 #副业 #招投标",
}

def _extract_video_desc(title, body, platform, md_path):
    """从口播稿 md 提取视频简介文案。
    - 标题：优先用 md 文件名里的主题串（YYYYMMDD-<平台>-<主题>.md）
    - 正文：去「镜头N（封面）：」「（快节奏）」「镜头N：」等分镜前缀，保留口播文字
    - 声明：md 末尾若已含「本文由 AI 辅助创作」则保留，否则补通用声明
    - 话题标签：追加平台固定通用标签（不编造具体政策标签）
    """
    m = re.search(r"\d{8}-(?:抖音|微信视频号)-(.*?)\.md$", os.path.basename(md_path))
    theme = m.group(1) if m else (title or "")
    cleaned = []
    # 剔除 md 首行的【平台口播稿 · 主题】整行（标题已由文件名主题串提供，避免重复）
    if body and re.match(r"^【.*口播稿.*】$", body[0].strip()):
        body = body[1:]
    for ln in body:
        s = ln.strip()
        if not s:
            continue
        s = re.sub(r"^【?镜头\d+(?:（[^）]*）)?】?[：:]?", "", s)
        s = s.replace("（快节奏）", "").replace("（封面）", "").replace("（慢节奏）", "")
        s = strip_md_inline(s).strip()
        if s:
            cleaned.append(s)
    has_ai = any("AI 辅助创作" in c for c in cleaned)
    if not has_ai:
        cleaned.append("声明：本文由 AI 辅助创作，关键政策请以官方原文为准。")
    cleaned.append("")
    cleaned.append(VIDEO_DEFAULT_HASHTAGS.get(platform, ""))
    return theme, cleaned


# 小红书视频简介提取器（2026-08-19 新增）：源为 scenes_xhs_*.json
XHS_VIDEO_HASHTAGS = "#评标专家 #专家入库 #副业 #招投标"

def _convert_xhs_video(json_path):
    """从 scenes_xhs_*.json 提取视频简介（小红书视频发布用，无插图标记）。
    - 标题：文件名主题串（scenes_xhs_<主题>.json → 主题）
    - 正文：每镜 narr 拼接，去分镜/封面提示词
    - 声明：末镜已含 AI 声明则保留，否则补通用声明
    - 话题标签：平台固定通用标签
    """
    import json as _json
    data = _json.load(open(json_path, encoding="utf-8"))
    m = re.search(r"scenes_xhs_(.*?)\.json$", os.path.basename(json_path))
    theme = m.group(1) if m else ""
    cleaned = []
    for sc in data:
        narr = sc.get("narr", "").strip()
        if not narr:
            continue
        narr = re.sub(r"^【?镜头\d+(?:（[^）]*）)?】?[：:]?", "", narr)
        narr = narr.replace("（快节奏）", "").replace("（封面）", "").replace("（慢节奏）", "")
        narr = strip_md_inline(narr).strip()
        if narr:
            cleaned.append(narr)
    has_ai = any("AI 辅助创作" in c for c in cleaned)
    if not has_ai:
        cleaned.append("声明：本文由 AI 辅助创作，关键政策请以官方原文为准。")
    cleaned.append("")
    cleaned.append(XHS_VIDEO_HASHTAGS)
    return theme + "\n\n" + "\n".join(cleaned).strip() + "\n"

# 默认（历史 20260805）标记；当 meta.json 提供 xhs_image_markers 时优先用 meta 参数化配置
DEFAULT_XHS_MARKERS = [
    ["三件会要命", "🖼️插入配图2（四省对比 · 续聘窗口自查表）"],
    ["今天就做三件", "🖼️插入配图3（今天三件事 · 行动清单）"],
]

def insert_image_markers(out, meta_key="xhs_image_markers", default=None, md_path=None):
    """在匹配行后插入 🖼️ 提示。
    markers 优先从 meta.json 取（参数化）；但仅当 meta.run_date 与目标文件所属运行日一致时才采用，
    避免用今日 meta 去重生历史日期目录的文案时丢失/错配插图标记。"""
    meta = load_meta()
    markers = meta.get(meta_key)
    if markers and md_path:
        m = re.search(r"(\d{8})", os.path.basename(md_path))
        # 2026-08-10 修复：run_date 为 "YYYY-MM-DD"，文件名为 "YYYYMMDD"，
        # 原先直接字符串比较恒不相等 → 插图标记被静默丢弃。此处统一去非数字后再比。
        run_digits = re.sub(r"\D", "", str(meta.get("run_date", "")))
        if m and run_digits != m.group(1):
            markers = None
    if not markers:
        markers = default if default is not None else []
    if not markers:
        return out
    res = []
    for line in out:
        res.append(line)
        for pair in markers:
            try:
                key, tip = pair[0], pair[1]
            except (IndexError, TypeError):
                continue
            if key and key in line:
                res.append(tip)
    return res

if __name__ == "__main__":
    md = sys.argv[1]
    platform = sys.argv[2] if len(sys.argv) > 2 else "xhs"
    txt = convert(md, platform)
    if platform == "xhs_video":
        # 先把 scenes_xhs_xxx.json 改成 小红书-视频-xxx-可直接复制粘贴版.txt
        out_path = re.sub(r"scenes_xhs_(.*?)\.json$",
                          r"小红书-视频-\1-可直接复制粘贴版.txt", md)
    else:
        out_path = re.sub(r"\.md$", "-可直接复制粘贴版.txt", md)
    open(out_path, "w", encoding="utf-8").write(txt)
    print("PLAINTEXT ->", out_path, len(txt), "chars")
