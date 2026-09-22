#!/usr/bin/env python3
"""
PubMed 文献检索模块
- 支持 E-utilities API 搜索
- 获取摘要和元数据
- 支持 easyScholar 期刊分区查询
"""

import json
import time
import urllib.request
import urllib.parse
import urllib.error
import xml.etree.ElementTree as ET
from datetime import datetime, timedelta


def load_config(config_path="config.json"):
    with open(config_path, "r", encoding="utf-8") as f:
        return json.load(f)


def search_pubmed(query, date_from, date_to, max_results=20, retstart=0):
    """搜索 PubMed，返回 PMID 列表"""
    base_url = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi"
    params = {
        "db": "pubmed",
        "term": f"{query} AND (\"{date_from}\"[Date - Publication] : \"{date_to}\"[Date - Publication])",
        "retmax": max_results,
        "retstart": retstart,
        "retmode": "json",
        "sort": "date"
    }
    url = f"{base_url}?{urllib.parse.urlencode(params)}"
    
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
        with urllib.request.urlopen(req, timeout=30) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            return data.get("esearchresult", {}).get("idlist", [])
    except Exception as e:
        print(f"  [WARN] PubMed search failed: {e}")
        return []


def fetch_article_details(pmids):
    """获取文献详情（标题、摘要、作者、期刊等）"""
    if not pmids:
        return []
    
    base_url = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi"
    params = {
        "db": "pubmed",
        "id": ",".join(pmids),
        "rettype": "xml",
        "retmode": "xml"
    }
    url = f"{base_url}?{urllib.parse.urlencode(params)}"
    
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
        with urllib.request.urlopen(req, timeout=60) as resp:
            xml_content = resp.read().decode("utf-8")
    except Exception as e:
        print(f"  [WARN] PubMed efetch failed: {e}")
        return []
    
    articles = []
    root = ET.fromstring(xml_content)
    
    for article in root.findall(".//PubmedArticle"):
        try:
            art = {}
            
            # PMID
            pmid_el = article.find(".//PMID")
            art["pmid"] = pmid_el.text if pmid_el is not None else ""
            
            # Title
            title_el = article.find(".//ArticleTitle")
            art["title"] = "".join(title_el.itertext()) if title_el is not None else ""
            
            # Journal
            journal_el = article.find(".//Journal/Title")
            art["journal"] = journal_el.text if journal_el is not None else ""
            
            # Journal abbreviation
            abbr_el = article.find(".//Journal/ISOAbbreviation")
            art["journal_abbr"] = abbr_el.text if abbr_el is not None else art["journal"]
            
            # Authors
            authors = []
            for author in article.findall(".//Author"):
                last = author.find("LastName")
                fore = author.find("ForeName")
                if last is not None and fore is not None:
                    authors.append(f"{last.text} {fore.text}")
                elif last is not None:
                    authors.append(last.text)
            art["authors"] = authors
            art["firstAuthor"] = authors[0] if authors else ""
            
            # Pub date
            pub_date = article.find(".//PubDate")
            if pub_date is not None:
                year = pub_date.find("Year")
                month = pub_date.find("Month")
                day = pub_date.find("Day")
                parts = []
                if year is not None: parts.append(year.text)
                if month is not None: parts.append(month.text)
                if day is not None: parts.append(day.text)
                art["pubDate"] = " ".join(parts)
            else:
                art["pubDate"] = ""
            
            # Abstract
            abstract_parts = []
            for abs_text in article.findall(".//AbstractText"):
                label = abs_text.get("Label", "")
                text = "".join(abs_text.itertext())
                if label:
                    abstract_parts.append(f"【{label}】{text}")
                else:
                    abstract_parts.append(text)
            art["abstract"] = "\n\n".join(abstract_parts)
            
            # DOI
            doi_el = article.find(".//ArticleId[@IdType='doi']")
            art["doi"] = doi_el.text if doi_el is not None else ""
            
            # Article type
            pub_type = article.find(".//PublicationType")
            art["articleType"] = pub_type.text if pub_type is not None else ""
            
            articles.append(art)
        except Exception as e:
            print(f"  [WARN] Failed to parse article: {e}")
            continue
    
    return articles


def get_journal_rank(easyScholar_key, journal_name):
    """通过 easyScholar API 获取期刊 JCR 分区和影响因子"""
    if not easyScholar_key or not journal_name:
        return {}
    
    base_url = "https://easyscholar.cc/open/getPublicationRank"
    params = {
        "secretKey": easyScholar_key,
        "publicationName": journal_name
    }
    url = f"{base_url}?{urllib.parse.urlencode(params)}"
    
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
        with urllib.request.urlopen(req, timeout=15) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            if data.get("code") == 200 and data.get("data"):
                official = data["data"].get("officialRank", {}).get("all", {})
                if official:
                    return {
                        "jcr": official.get("sci", ""),
                        "if_2023": official.get("sciif", ""),
                        "casUp": official.get("sciUp", ""),
                        "casDown": official.get("sciBase", "")
                    }
    except Exception as e:
        print(f"  [WARN] easyScholar lookup failed for {journal_name}: {e}")
    
    return {}


def run_search(config_path="config.json"):
    """执行完整的文献检索流程"""
    config = load_config(config_path)
    search_config = config.get("search", {})
    easyScholar_key = config.get("easyScholar", {}).get("secret_key", "")
    
    date_to = datetime.now()
    date_from = date_to - timedelta(days=search_config.get("date_range_days", 7))
    date_from_str = date_from.strftime("%Y/%m/%d")
    date_to_str = date_to.strftime("%Y/%m/%d")
    
    print(f"📅 检索时间范围: {date_from_str} - {date_to_str}")
    
    all_articles = []
    
    for topic in search_config.get("topics", []):
        topic_name = topic["name"]
        queries = topic.get("queries", [])
        max_results = topic.get("max_results", search_config.get("max_results_per_topic", 20))
        
        # 组合查询条件
        combined_query = " AND ".join(queries) if queries else ""
        if not combined_query:
            print(f"  [SKIP] {topic_name}: 无有效查询条件")
            continue
        
        print(f"\n🔍 检索: {topic_name}")
        print(f"  查询: {combined_query[:100]}...")
        
        # 搜索 PubMed
        pmids = search_pubmed(combined_query, date_from_str, date_to_str, max_results)
        print(f"  找到 {len(pmids)} 篇文献")
        
        if not pmids:
            continue
        
        # 获取详情
        articles = fetch_article_details(pmids)
        
        # 标记分类
        for art in articles:
            art["category"] = topic_name
        
        # 期刊分区查询
        if easyScholar_key and config.get("easyScholar", {}).get("enabled", False):
            print(f"  查询期刊分区...")
            for art in articles:
                art["journalRank"] = get_journal_rank(easyScholar_key, art.get("journal_abbr", "") or art.get("journal", ""))
                time.sleep(0.3)  # 限速
        
        # 期刊过滤
        journal_filter = topic.get("journal_filter", [])
        min_rank = search_config.get("min_journal_rank", "")
        
        if journal_filter:
            before_count = len(articles)
            articles = [a for a in articles if a.get("journal_abbr", "") in journal_filter or a.get("journal", "") in journal_filter]
            print(f"  期刊过滤: {before_count} → {len(articles)} 篇")
        
        if min_rank and easyScholar_key:
            rank_order = {"Q1": 1, "Q2": 2, "Q3": 3, "Q4": 4}
            min_val = rank_order.get(min_rank, 99)
            before_count = len(articles)
            filtered = []
            for a in articles:
                jcr = a.get("journalRank", {}).get("jcr", "")
                if not jcr:
                    # easyScholar 查不到分区时保留文献（标记为"未查询到分区"）
                    a["_no_rank"] = True
                    filtered.append(a)
                elif rank_order.get(jcr, 99) <= min_val:
                    filtered.append(a)
            articles = filtered
            no_rank_count = sum(1 for a in articles if a.get("_no_rank"))
            print(f"  分区过滤 (>={min_rank}): {before_count} → {len(articles)} 篇" + 
                  (f" (其中 {no_rank_count} 篇未查询到分区，已保留)" if no_rank_count else ""))
        
        all_articles.extend(articles)
    
    print(f"\n{'='*40}")
    print(f"📊 检索完成: 共 {len(all_articles)} 篇文献")
    
    return all_articles


if __name__ == "__main__":
    articles = run_search()
    print(json.dumps(articles, ensure_ascii=False, indent=2))
