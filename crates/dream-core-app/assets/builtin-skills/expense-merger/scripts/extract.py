#!/usr/bin/env python3
"""
extract.py <目录>
扫描目录里所有 OFD + PDF 发票，提取金额、类型、日期、行程/卖方、初步城市猜测，
写入 <目录>/.manifest.json 供人工或 AI 复核。
"""
import os, re, sys, json, zipfile, subprocess

from config import (
    HOME_CITIES, TRANSIT_HINTS, CATEGORY_BY_TYPE, AMBIGUOUS_TYPES,
    ENTERTAINMENT_CATEGORY, ALCOHOL_KEYWORDS, TOBACCO_KEYWORDS,
)

# 兼容旧变量名
HOME_KEYS = HOME_CITIES

# ---------------- 行政区划 ----------------
# 统一社会信用代码 18 位：第 1 位登记管理部门，第 2 位机构类别，
# **第 3-8 位是登记管理机关行政区划码**（6 位：前 2 位省、3-4 位市、5-6 位区县）。
#
# 旧版本这里错在把省级码当成了市级码——"9132" 被写成石家庄，
# 实际 32 是江苏；"9134" 写成唐山，实际 34 是安徽。而且字典里有重复键，
# 后写的静默覆盖先写的，秦皇岛、邯郸那两条从来没生效过。
# 现在拆成两级查：先查地级市码，查不到再回退到省会。

PROVINCE_CODE = {
    "11": "北京", "12": "天津", "13": "河北", "14": "山西", "15": "内蒙古",
    "21": "辽宁", "22": "吉林", "23": "黑龙江",
    "31": "上海", "32": "江苏", "33": "浙江", "34": "安徽", "35": "福建",
    "36": "江西", "37": "山东",
    "41": "河南", "42": "湖北", "43": "湖南", "44": "广东", "45": "广西", "46": "海南",
    "50": "重庆", "51": "四川", "52": "贵州", "53": "云南", "54": "西藏",
    "61": "陕西", "62": "甘肃", "63": "青海", "64": "宁夏", "65": "新疆",
    "71": "台湾", "81": "香港", "82": "澳门",
}

# 省份 → 省会（地级市码查不到时的回退值）
PROVINCE_CAPITAL = {
    "北京": "北京", "天津": "天津", "上海": "上海", "重庆": "重庆",
    "河北": "石家庄", "山西": "太原", "内蒙古": "呼和浩特",
    "辽宁": "沈阳", "吉林": "长春", "黑龙江": "哈尔滨",
    "江苏": "南京", "浙江": "杭州", "安徽": "合肥", "福建": "福州",
    "江西": "南昌", "山东": "济南", "河南": "郑州", "湖北": "武汉",
    "湖南": "长沙", "广东": "广州", "广西": "南宁", "海南": "海口",
    "四川": "成都", "贵州": "贵阳", "云南": "昆明", "西藏": "拉萨",
    "陕西": "西安", "甘肃": "兰州", "青海": "西宁", "宁夏": "银川",
    "新疆": "乌鲁木齐", "台湾": "台北", "香港": "香港", "澳门": "澳门",
}

# 地级市行政区划码前 4 位 → 城市名。
# 只列了直辖市、省会、计划单列市和常见出差城市；不在表里的自动回退到省会。
# 要加城市直接往这里补，格式是“省码+市序号”，查国标 GB/T 2260。
CITY_CODE = {
    "1101": "北京", "1201": "天津", "3101": "上海", "5001": "重庆",
    "1301": "石家庄", "1302": "唐山", "1303": "秦皇岛", "1304": "邯郸",
    "1401": "太原", "1501": "呼和浩特",
    "2101": "沈阳", "2102": "大连", "2201": "长春", "2301": "哈尔滨",
    "3201": "南京", "3202": "无锡", "3204": "常州", "3205": "苏州", "3206": "南通",
    "3301": "杭州", "3302": "宁波", "3304": "嘉兴", "3306": "绍兴", "3310": "台州",
    "3401": "合肥", "3501": "福州", "3502": "厦门", "3505": "泉州",
    "3601": "南昌", "3701": "济南", "3702": "青岛", "3703": "淄博", "3706": "烟台",
    "4101": "郑州", "4201": "武汉", "4301": "长沙", "4302": "株洲", "4306": "岳阳",
    "4401": "广州", "4403": "深圳", "4404": "珠海", "4406": "佛山", "4413": "惠州",
    "4419": "东莞", "4420": "中山",
    "4501": "南宁", "4502": "柳州", "4503": "桂林", "4601": "海口", "4602": "三亚",
    "5101": "成都", "5201": "贵阳", "5301": "昆明", "5401": "拉萨",
    "6101": "西安", "6201": "兰州", "6301": "西宁", "6401": "银川", "6501": "乌鲁木齐",
}

# 省份关键字 → 省会。用于公司名里直接带省份的情况（例：“湖南世容…”）
PROVINCE = dict(PROVINCE_CAPITAL)


def city_from_credit_code(code: str) -> str:
    """从统一社会信用代码取城市。code 至少要有前 8 位。

    先按地级市码查，查不到回退到省会。两级都查不到返回空串。
    """
    if not code or len(code) < 8:
        return ""
    region = code[2:8]          # 6 位行政区划码
    city = CITY_CODE.get(region[:4])
    if city:
        return city
    prov = PROVINCE_CODE.get(region[:2])
    return PROVINCE_CAPITAL.get(prov, "") if prov else ""


# ---------------- OFD 解析 ----------------
def parse_ofd_text(path: str) -> str:
    """OFD = ZIP+XML，提取所有 TextCode 文本拼起来"""
    with zipfile.ZipFile(path) as z:
        chunks = []
        for name in z.namelist():
            if not name.endswith(".xml"):
                continue
            try:
                data = z.read(name).decode("utf-8", errors="ignore")
            except Exception:
                continue
            for m in re.finditer(r"<(?:ofd:)?TextCode[^>]*>([^<]*)</(?:ofd:)?TextCode>", data):
                chunks.append(m.group(1))
    return " ".join(chunks)


def parse_pdf_text(path: str) -> str:
    """调用 poppler 的 pdftotext，保留版式"""
    r = subprocess.run(["pdftotext", "-layout", path, "-"],
                       capture_output=True, text=True)
    return r.stdout


# ---------------- 金额识别 ----------------
def extract_amount(text: str):
    # 优先小写金额行，否则取所有 *.xx 中最大值
    m = re.search(r"小写\s*[)）]?\s*[¥￥]?\s*([0-9]+\.[0-9]{2})", text)
    if m:
        return float(m.group(1))
    m = re.search(r"价税合计.*?[¥￥]\s*([0-9]+\.[0-9]{2})", text)
    if m:
        return float(m.group(1))
    cands = [float(x) for x in re.findall(r"([0-9]+\.[0-9]{2})", text)]
    return max(cands) if cands else None


# ---------------- 票种与费用科目识别 ----------------
def detect_type(text: str, fname: str) -> str:
    """票据业务类型（这张票买的是什么）。顺序有讲究，先具体后笼统。"""
    if "机票" in fname or "代订机票" in text or "航空" in text or "航班" in text:
        return "机票"
    if "酒店" in text or "住宿服务" in text or "客房" in text or "住宿费" in text:
        return "酒店"
    if "出租车" in text or "网约车" in text or "客运服务" in text or "网络预约" in text:
        return "出租/网约车"
    if "通行费" in text or "高速公路" in text or "路桥费" in text:
        return "通行费"
    if "停车" in text:
        return "停车费"
    if "餐饮" in text or "餐厅" in text or "餐费" in text or "食品" in text:
        return "餐饮"
    if "会议" in text or "会务" in text:
        return "会议"
    if "办公用品" in text or "文具" in text or "耗材" in text:
        return "办公用品"
    if "电信" in text or "通信服务" in text or "话费" in text:
        return "通讯"
    if "铁路电子客票" in text or "12306" in text or re.search(r"[GDCZTKLY]\d{2,4}", text):
        return "高铁"
    if "礼品" in text or "工艺品" in text:
        return "礼品"
    return "其他"


def detect_invoice_kind(text: str, fname: str) -> str:
    """票面形式（这是一张什么票）。与业务类型是两个维度：
    一张餐饮消费可能开成专票也可能开成普票，票面形式决定能不能抵扣。
    """
    if "增值税专用发票" in text:
        return "增值税专用发票"
    if "铁路电子客票" in text or "12306" in text:
        return "铁路电子客票"
    if "行程单" in text or "电子客票行程单" in text:
        return "航空运输电子客票行程单"
    if "通行费" in text and "发票" in text:
        return "通行费电子发票"
    if "定额发票" in text:
        return "定额发票"
    if "全电发票" in text or "数电票" in text:
        return "全电发票"
    if "增值税电子普通发票" in text or "电子发票" in text:
        return "增值税电子普通发票"
    if "增值税普通发票" in text or "普通发票" in text:
        return "增值税普通发票"
    return "其他票据"


def detect_category(typ: str) -> tuple:
    """费用科目（记到哪个会计科目）。

    返回 (科目名, 是否需要人来定)。

    餐饮、礼品这类归属**票面上判不出来**：出差期间自己吃饭是差旅费，
    请客户吃饭是业务招待费，差别在事由和参与人，不在票面。
    所以这类一律返回“待定”+ needs_decision=True，交给人决定，不许脚本猜。
    """
    if typ in AMBIGUOUS_TYPES:
        return "待定", True
    for cat, types in CATEGORY_BY_TYPE.items():
        if typ in types:
            return cat, False
    return "待定", True


def detect_goods_flags(text: str) -> dict:
    """从票面商品明细里找需要额外说明的东西。"""
    return {
        "has_alcohol": any(k in text for k in ALCOHOL_KEYWORDS),
        "has_tobacco": any(k in text for k in TOBACCO_KEYWORDS),
    }


def build_risk_flags(typ, invoice_kind, category, goods, amount, date) -> list:
    """只做标记，不做税务计算。扣除比例会变、各家财务制度不同，算错比不算更糟。"""
    flags = []
    if category == ENTERTAINMENT_CATEGORY and invoice_kind == "增值税专用发票":
        flags.append("专票用于招待")
    if goods["has_alcohol"] and typ != "餐饮":
        flags.append("单独酒类发票")
    if goods["has_tobacco"]:
        flags.append("含烟草")
    if not date:
        flags.append("缺日期")
    if amount is None:
        flags.append("缺金额")
    return flags


# ---------------- 日期识别 ----------------
def detect_date(text: str) -> str:
    # 优先开票日期；其次匹配的 YYYY-MM-DD
    text_norm = re.sub(r"[０-９]", lambda m: chr(ord(m.group(0)) - 0xFEE0), text)  # 全角数字 → 半角
    text_norm = text_norm.replace("年", "-").replace("月", "-").replace("日", "")
    for pat in [r"开票日期[:：]?\s*(\d{4}-\d{1,2}-\d{1,2})",
                r"(\d{4}-\d{1,2}-\d{1,2})"]:
        m = re.search(pat, text_norm)
        if m:
            y, mo, d = m.group(1).split("-")
            return f"{y}-{int(mo):02d}-{int(d):02d}"
    return ""


# ---------------- 高铁路线提取 ----------------
STATION_PAT = re.compile(r"([一-龥]{2,6}站|[一-龥]{2,6}北|[一-龥]{2,6}南|[一-龥]{2,6}东|[一-龥]{2,6}西)")
PINYIN_STATION = re.compile(r"([A-Z][a-z]+(?:nan|bei|dong|xi))\b")
TRAIN_NO_PAT = re.compile(r"\b([GDCZTKLY]\d{2,4})\b")


def extract_train_route(text: str):
    """返回 (车次, 出发站, 到达站)，单看 OFD/PDF 文本结构差异较大，尽量找两个中文车站名"""
    train_no = ""
    m = TRAIN_NO_PAT.search(text)
    if m:
        train_no = m.group(1)
    # 全角数字归一化（pdftotext 输出经常是全角）
    text_n = re.sub(r"[Ａ-Ｚａ-ｚ０-９]", lambda m: chr(ord(m.group(0)) - 0xFEE0), text)
    # 提取所有中文 + 方位词的车站候选
    stations = re.findall(r"([一-龥]{2,5}(?:北|南|东|西))站?", text_n)
    # 去重保序
    seen = set()
    uniq = []
    for s in stations:
        if s not in seen:
            uniq.append(s)
            seen.add(s)
    # 经验上前两个站就是起点+终点
    if len(uniq) >= 2:
        return train_no, uniq[0], uniq[1]
    return train_no, "", ""


# ---------------- 酒店卖方提取 ----------------
def extract_hotel_seller(text: str):
    """返回 (卖方公司名, 信用代码前 8 位)。前 8 位含完整行政区划码，可定位到地级市。"""
    seller_name = ""
    m = re.search(r"销\s*[售\s]*方[\s\S]{0,200}?名称\s*[:：]?\s*([^\n\r　]+?(?:酒店|宾馆|公寓|旅馆|饭店|度假|民宿)[^\n\r　]*)", text)
    if m:
        seller_name = m.group(1).strip().replace(" ", "")
    if not seller_name:
        # 退化：找文本里第一个含"酒店/宾馆"的串
        m = re.search(r"([一-龥]{2,15}(?:酒店|宾馆|公寓|旅馆|饭店|度假|民宿)(?:有限公司|集团|管理[^\s]{0,8})?)", text)
        if m:
            seller_name = m.group(1)
    # 找卖方一侧的信用代码：销售方块之后第二个 91/92 开头的
    # 返回前 8 位而不是前 4 位——第 3-8 位才是完整的 6 位行政区划码，
    # 只取 4 位就只剩省级精度，地级市信息白丢了。
    codes = re.findall(r"(9[1-3]\d{12,16}[A-Z0-9])", text)
    code_prefix = codes[1][:8] if len(codes) >= 2 else (codes[0][:8] if codes else "")
    return seller_name, code_prefix


# ---------------- 机票航线提取 ----------------
FLIGHT_ROUTE_PAT = re.compile(r"(\d{4}/\d{1,2}/\d{1,2})\s*([一-龥]{2,4})\s*[-→]\s*([一-龥]{2,4})")


def extract_flight_route(text: str):
    m = FLIGHT_ROUTE_PAT.search(text)
    if m:
        return m.group(1), m.group(2), m.group(3)
    return "", "", ""


# ---------------- 城市猜测 ----------------
def station_to_city(station: str) -> str:
    """高铁站名 → 主城市（去掉方位后缀）"""
    if not station:
        return ""
    return re.sub(r"(北|南|东|西|站)$", "", station)


def is_home(name: str) -> bool:
    return any(h in name for h in HOME_KEYS)


def guess_city_train(start: str, end: str) -> str:
    """两个站点里，挑非家乡且非过路站的；都非家乡就挑非过路那个"""
    cities = []
    for s in (start, end):
        c = station_to_city(s)
        if c and not is_home(s):
            cities.append((c, s in TRANSIT_HINTS or any(t in s for t in TRANSIT_HINTS)))
    if not cities:
        return ""
    non_transit = [c for c, t in cities if not t]
    if non_transit:
        return non_transit[0]
    return cities[0][0]


def guess_city_hotel(seller: str, credit_code: str) -> str:
    """酒店所在地：先按信用代码里的行政区划码查，查不到再从公司名里找省份关键字。"""
    city = city_from_credit_code(credit_code)
    if city:
        return city
    for prov, cap in PROVINCE.items():
        if prov in seller:
            return cap
    return ""


def guess_city_flight(start: str, end: str) -> str:
    for c in (start, end):
        if c and not is_home(c):
            return c
    return ""


# ---------------- 主逻辑 ----------------
def process_one(src_dir: str, fname: str):
    p = os.path.join(src_dir, fname)
    ext = fname.lower().rsplit(".", 1)[-1]
    if ext not in ("ofd", "pdf"):
        return None
    text = parse_ofd_text(p) if ext == "ofd" else parse_pdf_text(p)
    amount = extract_amount(text)
    typ = detect_type(text, fname)
    date = detect_date(text)
    invoice_kind = detect_invoice_kind(text, fname)
    category, needs_decision = detect_category(typ)
    goods = detect_goods_flags(text)
    flags = build_risk_flags(typ, invoice_kind, category, goods, amount, date)

    route_or_seller = ""
    city = ""

    if typ == "高铁":
        train_no, start, end = extract_train_route(text)
        route_or_seller = f"{train_no} {start}→{end}".strip()
        city = guess_city_train(start, end)
    elif typ == "机票":
        _, start, end = extract_flight_route(text)
        route_or_seller = f"{start}→{end}" if start and end else ""
        city = guess_city_flight(start, end)
    elif typ == "酒店":
        seller, credit_code = extract_hotel_seller(text)
        route_or_seller = seller
        city = guess_city_hotel(seller, credit_code)

    return {
        "file": fname,
        "ext": ext,
        "amount": amount,
        "type": typ,
        "invoice_kind": invoice_kind,
        "category_guess": category,
        "needs_decision": needs_decision,
        "date": date,
        "route_or_seller": route_or_seller,
        "city_guess": city or "未识别",
        "has_alcohol": goods["has_alcohol"],
        "has_tobacco": goods["has_tobacco"],
        "flags": flags,
        "text_preview": text[:300].replace("\n", " ").replace("\r", " "),
    }


def main(src_dir: str):
    if not os.path.isdir(src_dir):
        print(f"ERROR: 目录不存在: {src_dir}", file=sys.stderr)
        sys.exit(1)
    rows = []
    for f in sorted(os.listdir(src_dir)):
        if f.startswith(".") or f.endswith("_报销汇总.pdf"):
            continue
        row = process_one(src_dir, f)
        if row:
            rows.append(row)
    out_path = os.path.join(src_dir, ".manifest.json")
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(rows, f, ensure_ascii=False, indent=2)
    # 同时打印精简版到 stdout，方便直接阅读或交给 AI 复核
    print(json.dumps(rows, ensure_ascii=False, indent=2))
    print(f"\n[manifest 已写入] {out_path}", file=sys.stderr)
    print(f"[发票张数] {len(rows)}    [初步总金额] {sum(r['amount'] or 0 for r in rows):.2f}", file=sys.stderr)


def require_config():
    """常驻城市没填就停下。

    这一项留空不会报错，但会让行程目的地判定静默取到起点——
    “北京南→济南西”会被判成北京。错得不响，比直接失败更难发现，
    所以在这里挡住，不进入解析流程。
    """
    if not HOME_CITIES:
        print(
            "还没设置常驻城市，无法判断出差目的地。\n"
            "请打开 scripts/config.py，把 HOME_CITIES 填成你的常驻城市，例如：\n"
            '    HOME_CITIES = ["北京"]\n'
            "（可填多个；填完再重新运行）",
            file=sys.stderr,
        )
        sys.exit(2)


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("用法: extract.py <目录>", file=sys.stderr)
        sys.exit(1)
    require_config()
    main(sys.argv[1])
