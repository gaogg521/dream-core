#!/usr/bin/env python3
"""
天气查询 Skill v1.1.0
入口文件，由 WorkBuddy 调用
"""

import sys
import re
import json
import urllib.request
import urllib.parse
import urllib.error
from datetime import datetime, timedelta

# ========== 缓存 ==========
_cache = {}
CACHE_TTL = 600  # 10分钟


def get_cache(key):
    if key in _cache:
        val, ts = _cache[key]
        if (datetime.now() - ts).seconds < CACHE_TTL:
            return val
        del _cache[key]
    return None


def set_cache(key, val):
    _cache[key] = (val, datetime.now())


# ========== API ==========
def fetch_weather(city, days=3):
    cache_key = f"{city}:{days}"
    cached = get_cache(cache_key)
    if cached:
        return cached

    encoded = urllib.parse.quote(city)
    url = f"https://wttr.in/{encoded}?format=j1&lang=zh"

    try:
        req = urllib.request.Request(
            url,
            headers={
                "User-Agent": "Mozilla/5.0 (compatible; WeatherSkill/1.1.0)",
                "Accept-Language": "zh-CN,zh;q=0.9,en;q=0.8",
            }
        )
        with urllib.request.urlopen(req, timeout=10) as resp:
            data = json.loads(resp.read().decode("utf-8"))
        set_cache(cache_key, data)
        return data
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return {"error": f"找不到城市「{city}」，请检查城市名"}
        return {"error": f"查询失败（HTTP {e.code}）"}
    except urllib.error.URLError:
        return {"error": "网络错误，无法连接天气服务"}
    except Exception as e:
        return {"error": f"查询出错：{str(e)}"}


# ========== emoji ==========
def emoji(desc):
    d = desc.lower()
    if any(k in d for k in ["晴", "sunny", "clear"]):
        return "☀️"
    elif any(k in d for k in ["雷", "thunder"]):
        return "⛈"
    elif any(k in d for k in ["暴雨", "大暴雨"]):
        return "🌊"
    elif any(k in d for k in ["雨", "rain", "shower"]):
        return "🌧"
    elif any(k in d for k in ["雪", "snow"]):
        return "❄️"
    elif any(k in d for k in ["雾", "fog", "mist", "霾"]):
        return "🌫"
    elif any(k in d for k in ["云", "cloud", "阴", "overcast"]):
        return "☁️"
    return "🌤"


# ========== 解析 ==========
def parse_query(text):
    text = text.strip()
    days = 0
    if "明天" in text:
        days = 1
    elif "后天" in text:
        days = 2
    m = re.search(r"(\d+)\s*天", text)
    if m:
        days = min(int(m.group(1)) - 1, 2)

    clean = re.sub(r"(天气|气温|温度|下雨|下雪|多少度|热|冷|穿衣|带伞|适合出门|怎么样|如何|查询|查一下)", "", text)
    clean = re.sub(r"(今天|明天|后天|\d+天)", "", clean).strip()
    cities = [c.strip() for c in re.split(r"[和、&,/]", clean) if c.strip()]
    return cities[:3] if cities else ["北京"], days


def format_current(city, c):
    return f"""📍 {city}
{emoji(c['weatherDesc'][0]['value'])} {c['weatherDesc'][0]['value']}
🌡 {c['temp_C']}°C（体感 {c['FeelsLikeC']}°C）
💧 湿度 {c['humidity']}%
🌬 {c['winddir16Point']} {c['windspeedKmph']}km/h"""


def format_day(day, idx):
    desc = day['hourly'][4]['weatherDesc'][0]['value']
    return f"📅 {['今天','明天','后天'][idx]}：{emoji(desc)} {desc}，{day['mintempC']}~{day['maxtempC']}°C"


# ========== 主入口 ==========
def main():
    if len(sys.argv) < 2:
        print("请输入城市名，如：北京天气")
        sys.exit(1)

    query = " ".join(sys.argv[1:])
    cities, days = parse_query(query)

    # 多城市对比
    if len(cities) > 1:
        for city in cities:
            data = fetch_weather(city)
            if "error" in data:
                print(f"❌ {city}: {data['error']}")
                continue
            c = data["current_condition"][0]
            print(format_current(city, c))
            print()
        sys.exit(0)

    # 单城市
    city = cities[0]
    data = fetch_weather(city, days=days + 1)
    if "error" in data:
        print(f"❌ {data['error']}")
        sys.exit(1)

    c = data["current_condition"][0]
    print(format_current(city, c))

    # 附加预报
    if days == 0 and "weather" in data:
        for i, day in enumerate(data["weather"][1:3], 1):
            print(format_day(day, i))

    sys.exit(0)


if __name__ == "__main__":
    main()