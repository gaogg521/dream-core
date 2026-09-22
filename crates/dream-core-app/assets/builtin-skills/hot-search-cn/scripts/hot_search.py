# -*- coding: utf-8 -*-
import sys, io, os, json, argparse, urllib.request, urllib.parse, re
sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')
sys.stderr = io.TextIOWrapper(sys.stderr.buffer, encoding='utf-8', errors='replace')

UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"

def fetch_json(url, timeout=10):
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode("utf-8"))

def fetch_text(url, timeout=10):
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.read().decode("utf-8", errors="replace")

# --- 微博热搜 ---
def weibo(limit=50):
    try:
        data = fetch_json("https://weibo.com/ajax/side/hotSearch")
        items = data.get("data", {}).get("realtime", [])
        return [{"rank": i+1, "title": x.get("word", ""), "hot": x.get("num", 0),
                 "label": x.get("label_name", ""), "platform": "weibo"}
                for i, x in enumerate(items[:limit]) if x.get("word")]
    except Exception as e:
        return [{"error": str(e), "platform": "weibo"}]

# --- 知乎热榜 ---
def zhihu(limit=50):
    try:
        data = fetch_json("https://www.zhihu.com/api/v3/feed/topstory/hot-lists/total?limit=50")
        items = data.get("data", [])
        return [{"rank": i+1, "title": x.get("target", {}).get("title", ""),
                 "hot": int(x.get("detail_text", "0").replace("万", "0000").replace(" 热度", "")),
                 "platform": "zhihu"} for i, x in enumerate(items[:limit])]
    except Exception as e:
        return [{"error": str(e), "platform": "zhihu"}]

# --- 百度热搜 ---
def baidu(limit=50):
    try:
        html = fetch_text("https://top.baidu.com/board?tab=realtime")
        pattern = r'"word":"([^"]+)"[^}]*"hotScore":"?(\d+)"?'
        matches = re.findall(pattern, html)
        return [{"rank": i+1, "title": m[0], "hot": int(m[1]), "platform": "baidu"}
                for i, m in enumerate(matches[:limit])]
    except Exception as e:
        return [{"error": str(e), "platform": "baidu"}]

# --- B站热门 ---
def bilibili(limit=50):
    try:
        data = fetch_json("https://api.bilibili.com/x/web-interface/popular?ps=50&pn=1")
        items = data.get("data", {}).get("list", [])
        return [{"rank": i+1, "title": x.get("title", ""),
                 "hot": x.get("stat", {}).get("view", 0),
                 "author": x.get("owner", {}).get("name", ""),
                 "platform": "bilibili"} for i, x in enumerate(items[:limit])]
    except Exception as e:
        return [{"error": str(e), "platform": "bilibili"}]

# --- 抖音热点 ---
def douyin(limit=50):
    try:
        data = fetch_json("https://www.douyin.com/aweme/v1/web/hot/search/list/")
        items = data.get("data", {}).get("word_list", [])
        return [{"rank": i+1, "title": x.get("word", ""), "hot": x.get("hot_value", 0),
                 "platform": "douyin"} for i, x in enumerate(items[:limit])]
    except Exception as e:
        return [{"error": str(e), "platform": "douyin"}]

# --- 今日头条 ---
def toutiao(limit=50):
    try:
        data = fetch_json("https://www.toutiao.com/hot-event/hot-board/?origin=toutiao_pc")
        items = data.get("data", [])
        return [{"rank": i+1, "title": x.get("Title", ""), "hot": x.get("HotValue", 0),
                 "platform": "toutiao"} for i, x in enumerate(items[:limit])]
    except Exception as e:
        return [{"error": str(e), "platform": "toutiao"}]

# --- 36氪 ---
def kr36(limit=50):
    try:
        html = fetch_text("https://36kr.com/hot-list/catalog")
        titles = re.findall(r'"templateMaterial":\{[^}]*"widgetTitle":"([^"]+)"', html)
        return [{"rank": i+1, "title": t, "platform": "36kr"} for i, t in enumerate(titles[:limit])]
    except Exception as e:
        return [{"error": str(e), "platform": "36kr"}]

# --- 贴吧 ---
def tieba(limit=50):
    try:
        data = fetch_json("https://tieba.baidu.com/hottopic/browse/topicList")
        items = data.get("data", {}).get("bang_topic", {}).get("topic_list", [])
        return [{"rank": i+1, "title": x.get("topic_name", ""), "hot": x.get("discuss_num", 0),
                 "platform": "tieba"} for i, x in enumerate(items[:limit])]
    except Exception as e:
        return [{"error": str(e), "platform": "tieba"}]

PLATFORMS = {"weibo": weibo, "zhihu": zhihu, "baidu": baidu, "bilibili": bilibili,
             "douyin": douyin, "toutiao": toutiao, "36kr": kr36, "tieba": tieba}

def cmd_all(args):
    result = {}
    for name, fn in PLATFORMS.items():
        result[name] = fn(args.limit)
    print(json.dumps(result, ensure_ascii=False, indent=2))

def cmd_platform(args):
    fn = PLATFORMS.get(args.platform)
    if fn:
        print(json.dumps(fn(args.limit), ensure_ascii=False, indent=2))

def cmd_compare(args):
    all_topics = {}
    for name, fn in PLATFORMS.items():
        items = fn(args.limit)
        for item in items:
            if "title" not in item:
                continue
            key = item["title"].strip()
            all_topics.setdefault(key, []).append(name)
    cross = [(t, ps) for t, ps in all_topics.items() if len(ps) >= 2]
    cross.sort(key=lambda x: len(x[1]), reverse=True)
    print(json.dumps([{"title": t, "platforms": ps, "cross_count": len(ps)}
                      for t, ps in cross[:args.limit]], ensure_ascii=False, indent=2))

def main():
    p = argparse.ArgumentParser(description="Chinese Platform Hot Search Aggregator")
    sub = p.add_subparsers(dest="cmd")
    a = sub.add_parser("all"); a.add_argument("--limit", type=int, default=20)
    pl = sub.add_parser("platform"); pl.add_argument("platform", choices=list(PLATFORMS.keys())); pl.add_argument("--limit", type=int, default=20)
    c = sub.add_parser("compare"); c.add_argument("--limit", type=int, default=30)
    for name in PLATFORMS:
        sp = sub.add_parser(name); sp.add_argument("--limit", type=int, default=20)
    args = p.parse_args()
    if not args.cmd:
        p.print_help(); return
    if args.cmd == "all":
        cmd_all(args)
    elif args.cmd == "compare":
        cmd_compare(args)
    elif args.cmd in PLATFORMS:
        args.platform = args.cmd
        cmd_platform(args)

if __name__ == "__main__":
    main()
