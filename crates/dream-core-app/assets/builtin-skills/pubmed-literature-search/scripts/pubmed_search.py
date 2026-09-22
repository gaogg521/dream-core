#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
pubmed_search.py — PubMed 关键词文献检索（自包含，零第三方依赖）

用法:
  python pubmed_search.py --query "crispr AND cancer" \
      --mindate 2021/09/06 --maxdate 2026/09/06 \
      [--retmax 100] [--sort relevance] [--out articles.json] [--api-key XXX]

说明:
  - --query 须为 PubMed 语法（AI 负责把客户关键词 + 限制条件 组装成本参数）
  - --mindate / --maxdate 为客户必填时间窗，格式 YYYY/MM/DD 或 YYYY
  - 输出 JSON：每篇含 pmid/year/title/journal/ptype/authors/corr/aff/aff_src/doi
    zh(标题中译)/aff_zh(单位中译)/ifv(影响因子) 由 AI 后续补齐，不在本脚本职责内
  - 通讯作者口径：默认取末位作者（仅一位作者时取该作者），aff_src 标记来源
"""
import argparse
import json
import os
import re
from datetime import datetime
import sys
import time
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET

EUTILS = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils"
SLEEP = 0.4  # 无 API key 限速 3 req/s


def http_get(url, api_key=None, retries=3):
    if api_key:
        sep = "&" if "?" in url else "?"
        url = url + sep + "api_key=" + urllib.parse.quote(api_key)
    last_err = None
    for i in range(retries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "pubmed-search-assistant/1.0"})
            with urllib.request.urlopen(req, timeout=60) as r:
                return r.read().decode("utf-8", errors="replace")
        except Exception as e:  # noqa: BLE001
            last_err = e
            time.sleep(1.5 * (i + 1))
    raise RuntimeError("请求失败(已重试%d次): %s ; 最后错误: %s" % (retries, url[:120], last_err))


def journal_canon(full_title):
    """期刊全称 -> JCR 查表规范名（大写；去副题；去地名括号；去冠词 The；句点转空格）。
    PubMed 的 Title 常带副题后缀（'Journal of experimental & clinical cancer
    research : CR'）、' (London, England)' 后缀与分节句点（如
    'Nature reviews. Drug discovery'），直接 upper 无法命中 JCR 全称键。"""
    s = full_title.strip().upper()
    # 副题: ' : XXX' 或 ' = XXX'（PubMed 本族语期刊 ' = ' 后接原语言名）；分号副题 '; a journal of ...'
    for sep in (" : ", " = ", ";"):
        if sep in s:
            s = s.split(sep)[0].strip()
    while True:
        s2 = re.sub(r"\s*\([^)]*\)\s*$", "", s).strip()
        if s2 == s or not s2:
            break
        s = s2
    if s.startswith("THE "):
        s = s[4:]
    # 副题被裁后可能留下不闭合括号: 'Journal of immunology (Baltimore, Md. : 1950)'
    s = re.sub(r"\s*\([^()]*$", "", s).strip()
    s = s.replace(".", " ")
    return re.sub(r"\s+", " ", s).strip()


def _norm(s):
    """IF 表回退查找用规范化: 大写、&→AND、去掉所有非字母数字字符。
    解决 JCR 键与 PubMed 全称的写法差异: 逗号(,)、撇号(')、连字符(-)、AND vs &。"""
    t = s.upper().replace("&", " AND ")
    return re.sub(r"[^A-Z0-9]", "", t)


def load_ifmap(path):
    """加载 IF 表, 兼容两种格式:
    - 旧: {name: {field: value, ...}}
    - 新(体积优化, 解压后 <3MB 平台限制): {"_fields": [...], name: [v1, ...]}
      数组按 _fields 顺序还原为字典, 下游代码无感知。
    """
    with open(path, "r", encoding="utf-8-sig") as f:
        raw = json.load(f)
    if not raw:
        return {}
    sample = next(iter(raw.values()))
    if isinstance(sample, dict):
        return raw
    fields = raw.get("_fields") or []
    return {k: dict(zip(fields, v)) for k, v in raw.items()
            if k != "_fields" and isinstance(v, list)}


def if_lookup(ifmap, canon):
    """精确命中优先, 回退到规范化命中; 都失败返回 None(未收录, 不得编造)。"""
    rec = ifmap.get(canon)
    if rec is not None:
        return rec
    idx = if_lookup._idx
    if idx is None:
        idx = if_lookup._idx = {}
        for k, v in ifmap.items():
            idx.setdefault(_norm(k), v)
    return idx.get(_norm(canon))


if_lookup._idx = None


def esearch(query, mindate, maxdate, retmax, sort, api_key):
    params = {
        "db": "pubmed",
        "term": query,
        "retmode": "json",
        "retmax": str(retmax),
        "sort": sort,
        "mindate": mindate,
        "maxdate": maxdate,
        "datetype": "pdat",
    }
    url = EUTILS + "/esearch.fcgi?" + urllib.parse.urlencode(params)
    data = json.loads(http_get(url, api_key))
    res = data.get("esearchresult", {})
    if res.get("error"):
        raise RuntimeError("esearch 错误: %s" % res["error"])
    ids = res.get("idlist", [])
    total = int(res.get("count", "0"))
    return ids, total


def _text(el):
    if el is None:
        return ""
    return "".join(el.itertext()).strip()


def parse_article(art):
    """art: <PubmedArticle> Element -> dict"""
    cit = art.find("./MedlineCitation")
    article = cit.find("./Article")
    pmid = _text(cit.find("./PMID"))

    journal_el = article.find("./Journal")
    journal = _text(journal_el.find("./ISOAbbreviation")) or _text(journal_el.find("./Title"))
    # journal_full = 期刊全称, 专供 IF 查表(JCR 键为全称大写); journal = ISO 缩写仅供显示
    journal_full = _text(journal_el.find("./Title")) or journal
    jissue = journal_el.find("./JournalIssue/PubDate")
    year = _text(journal_el.find("./JournalIssue/PubDate/Year")) or _text(journal_el.find("./JournalIssue/PubDate/MedlineDate"))
    if not year:
        year = _text(article.find("./ArticleDate/Year"))

    title = _text(article.find("./ArticleTitle"))

    ptypes = [_text(t) for t in article.findall("./PublicationTypeList/PublicationType")]
    joined = ";".join(ptypes)
    if "Review" in joined:
        ptype = "综述"
    elif ("Erratum" in joined or "Retracted Publication" in joined
          or "Retraction of Publication" in joined or "Retraction Notice" in joined):
        ptype = "其他"  # 更正/撤稿声明，非研究论文
    elif "Journal Article" in joined:
        ptype = "研究"
    else:
        ptype = "其他"

    authors, corr, aff, aff_src = [], "", "", "none"
    author_els = []
    alist = article.find("./AuthorList")
    if alist is not None:
        author_els = alist.findall("./Author")
        for a in author_els:
            coll = _text(a.find("./CollectiveName"))
            if coll:
                authors.append(coll)
            else:
                last = _text(a.find("./LastName"))
                fore = _text(a.find("./ForeName"))
                authors.append((fore + " " + last).strip() or last)
    # 通讯作者口径: 末位作者
    if author_els:
        pick = author_els[-1]
        aff_src = "last"
        if len(author_els) == 1:
            aff_src = "only"
        affs = [_text(x) for x in pick.findall("./AffiliationInfo/Affiliation")]
        if affs:
            aff = "; ".join(affs)
            corr = authors[-1] if authors else ""
        # 末位无单位时回退首位, 并标记来源
        if not aff and author_els[0] is not pick:
            affs0 = [_text(x) for x in author_els[0].findall("./AffiliationInfo/Affiliation")]
            if affs0:
                aff = "; ".join(affs0)
                corr = authors[0] if authors else ""
                aff_src = "first"
        if not aff:
            aff_src = "none"

    doi = ""
    for aid in art.findall("./PubmedData/ArticleIdList/ArticleId"):
        if aid.get("IdType") == "doi":
            doi = _text(aid)
            break

    return {
        "pmid": pmid,
        "year": year,
        "title": title,
        "journal": journal,
        "journal_full": journal_full,
        "journal_canon": journal_canon(journal_full),
        "ptype": ptype,
        "authors": authors,
        "corr": corr,
        "aff": aff,
        "aff_src": aff_src,  # last=末位通讯口径 / only=唯一作者 / first=末位缺单位回退首位 / none=未取到
        "doi": doi,
    }


def efetch(ids, api_key):
    out = []
    B = 200
    for i in range(0, len(ids), B):
        batch = ids[i:i + B]
        url = EUTILS + "/efetch.fcgi?" + urllib.parse.urlencode({
            "db": "pubmed", "id": ",".join(batch), "retmode": "xml",
        })
        xml_text = http_get(url, api_key)
        root = ET.fromstring(xml_text)
        for art in root.findall("./PubmedArticle"):
            try:
                out.append(parse_article(art))
            except Exception as e:  # noqa: BLE001
                print("⚠ 解析失败(PMID未知): %s" % e, file=sys.stderr)
        if i + B < len(ids):
            time.sleep(SLEEP)
    return out


def main():
    ap = argparse.ArgumentParser(description="PubMed 关键词文献检索")
    ap.add_argument("--query", required=True, help="PubMed 语法检索式")
    ap.add_argument("--mindate", required=True, help="起始日期 YYYY/MM/DD 或 YYYY")
    ap.add_argument("--maxdate", required=True, help="截止日期 YYYY/MM/DD 或 YYYY")
    ap.add_argument("--retmax", type=int, default=100, help="最大返回篇数(默认100, 上限500)")
    ap.add_argument("--sort", default="relevance", choices=["relevance", "pub_date"], help="排序")
    ap.add_argument("--out", default="", help="输出 JSON 路径(缺省打印到 stdout)")
    ap.add_argument("--api-key", default="", help="NCBI API key(可选, 提升限速)")
    ap.add_argument("--if-gt", type=float, default=0.0,
                    help="影响因子阈值筛选(可选)。给出后脚本内完成 IF 查表并输出召回审计报告;"
                         "阈值只筛掉「已确定低于阈值」的文章；IF 表未收录期刊的文章一律保留(标 IF未收录), 严禁删除")
    args = ap.parse_args()

    # 日期格式硬校验: 非法日期会被 NCBI 静默吞掉返回 0 篇, 必须在本地拦截
    date_re = re.compile(r"^(\d{4})(/(\d{1,2})(/(\d{1,2}))?)?$")
    for name, val in (("--mindate", args.mindate), ("--maxdate", args.maxdate)):
        m = date_re.match(val.strip())
        if not m:
            print("❌ %s 格式非法: %r (须为 YYYY 或 YYYY/MM/DD)" % (name, val), file=sys.stderr)
            sys.exit(2)
        if m.group(2):  # 带月/日时做语义校验, 拦截 2025/13/99 这类不存在日期
            try:
                datetime.strptime(val.strip().replace("/", "-"), "%Y-%m-%d" if m.group(4) else "%Y-%m")
            except ValueError:
                print("❌ %s 日期不存在: %r" % (name, val), file=sys.stderr)
                sys.exit(2)

    retmax = min(args.retmax, 500)
    ids, total = esearch(args.query, args.mindate, args.maxdate, retmax, args.sort, args.api_key or None)
    if not ids:
        print(json.dumps({"query": args.query, "total_in_window": total, "fetched": 0, "articles": []},
                         ensure_ascii=False, indent=2))
        sys.exit(0)
    if not ids:
        print(json.dumps({"query": args.query, "total": total, "fetched": 0, "articles": []},
                         ensure_ascii=False, indent=2))
        sys.exit(0)
    articles = efetch(ids, args.api_key or None)
    result = {
        "query": args.query,
        "mindate": args.mindate,
        "maxdate": args.maxdate,
        "total_in_window": total,
        "fetched": len(articles),
        "articles": articles,
    }

    # ---------- IF 阈值筛选 + 召回审计（禁止静默丢弃） ----------
    if args.if_gt > 0:
        if_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "journal_if_map.json")
        ifmap = load_ifmap(if_path)
        selected, below, unknown_if = [], [], []
        for a in articles:
            rec = if_lookup(ifmap, a["journal_canon"])
            ifv = rec.get("if") if rec else None
            if rec is None:
                # 全面性优先: 查不到 IF 只不标注, 绝不下架文章
                a["ifv"], a["ifq"] = None, None
                unknown_if.append(a)
                continue
            a["ifv"], a["ifq"] = ifv, rec.get("jcr_quartile")
            if ifv is not None and ifv > args.if_gt:
                selected.append(a)
            else:
                below.append(a)
        audit = {
            "if_threshold": args.if_gt,
            "total_in_window": total,
            "fetched": len(articles),
            "if_map_hit": len(articles) - len(unknown_if),
            "if_map_miss": len(unknown_if),
            "selected": len(selected),
            "below_threshold": len(below),
            "unknown_if_journals": sorted({a["journal_canon"] for a in unknown_if}),
        }
        result["audit"] = audit
        result["selected"] = selected
        result["unknown_if"] = unknown_if  # 必须一并交付, 报告内标「IF未收录」
        print("── 召回审计（必须核对，禁止静默丢弃）──", file=sys.stderr)
        print("   窗口内命中 %s ｜ 取回 %s ｜ IF 表命中 %s ｜ IF>%s 入选 %s ｜ 已确定低于阈值 %s ｜ IF 表未收录 %s（保留, 不标 IF）"
              % (total, len(articles), audit["if_map_hit"], args.if_gt, len(selected),
                 len(below), len(unknown_if)), file=sys.stderr)
        if unknown_if:
            print("   ⚠ 以下期刊未收录于 IF 表：其 %d 篇文章须保留在报告中并标「IF未收录」，不得删除：" % len(unknown_if), file=sys.stderr)
            for j in audit["unknown_if_journals"][:20]:
                print("     -", j, file=sys.stderr)
        if total > len(articles):
            print("   ⚠ 取回数少于窗口内命中数，请提高 --retmax 后重跑", file=sys.stderr)

    js = json.dumps(result, ensure_ascii=False, indent=2)
    if args.out:
        with open(args.out, "w", encoding="utf-8") as f:
            f.write(js)
        print("✓ 命中 %s 篇(窗口内共 %s), 已写入 %s" % (len(articles), total, args.out), file=sys.stderr)
    else:
        print(js)


if __name__ == "__main__":
    main()
