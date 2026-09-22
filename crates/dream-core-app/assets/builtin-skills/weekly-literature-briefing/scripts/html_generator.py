#!/usr/bin/env python3
"""
HTML 邮件生成模块
- 将文献数据渲染为结构化 HTML 邮件
- 支持中英对照、期刊分区标注
"""

import json
from datetime import datetime, timedelta


def generate_briefing_html(articles, config_path="config.json"):
    """生成文献简报 HTML"""
    with open(config_path, "r", encoding="utf-8") as f:
        config = json.load(f)
    
    profile = config.get("profile", {})
    sender_name = profile.get("name", "文献简报助手")
    
    today = datetime.now()
    week_ago = today - timedelta(days=config.get("search", {}).get("date_range_days", 7))
    fmt = lambda d: f"{d.year}年{d.month}月{d.day}日"
    
    # 按分类分组
    categories = {}
    for art in articles:
        cat = art.get("category", "未分类")
        if cat not in categories:
            categories[cat] = []
        categories[cat].append(art)
    
    # 生成摘要表格
    summary_rows = ""
    for cat, cat_articles in categories.items():
        highlights = "、".join(set(
            a.get("titleZh", a.get("title", ""))[:15] + "..." 
            for a in cat_articles[:3]
        )) if cat_articles else "本周无高质量期刊发表"
        summary_rows += f"""
        <tr>
          <td><strong>{cat}</strong></td>
          <td>{len(cat_articles)}篇</td>
          <td>{highlights}</td>
        </tr>"""
    
    # 生成各分类文章
    sections_html = ""
    for cat, cat_articles in categories.items():
        articles_html = ""
        for i, a in enumerate(cat_articles):
            jcr = a.get("journalRank", {})
            jcr_badge = f'<span class="journal-badge">JCR {jcr.get("jcr", "")}</span>' if jcr.get("jcr") else ""
            cas_badge = f'<span class="journal-badge {"q1-badge" if jcr.get("jcr")=="Q1" else "q2-badge"}">{jcr.get("casUp", "")}</span>' if jcr.get("casUp") else ""
            if_badge = f'<span class="journal-badge if-badge">IF={jcr.get("if_2023", "N/A")}</span>' if jcr.get("if_2023") else ""
            
            abstract_en = ""
            if a.get("abstract"):
                text = a["abstract"][:800]
                abstract_en = f'<div class="abstract-en"><strong>Abstract:</strong><br>{text}{"..." if len(a["abstract"])>800 else ""}</div>'
            
            abstract_zh = ""
            if a.get("abstractZh"):
                text = a["abstractZh"][:500]
                abstract_zh = f'<div class="abstract-zh"><strong>摘要:</strong><br>{text}{"..." if len(a["abstractZh"])>500 else ""}</div>'
            
            articles_html += f"""
          <div class="article">
            <h3 class="title-zh">{i+1}. {a.get("titleZh", a.get("title", ""))}</h3>
            <p class="title-en">{a.get("title", "")}</p>
            <p>{jcr_badge}{cas_badge}{if_badge}</p>
            <table class="info-table">
              <tr><td><strong>发表时间</strong></td><td>{a.get("pubDate", "N/A")}</td></tr>
              <tr><td><strong>文献类型</strong></td><td>{a.get("articleType", "")}</td></tr>
              <tr><td><strong>期刊</strong></td><td>{a.get("journal", "")}</td></tr>
              <tr><td><strong>第一作者</strong></td><td>{a.get("firstAuthor", "")}</td></tr>
              <tr><td><strong>DOI</strong></td><td>{a.get("doi", "N/A")}</td></tr>
              <tr><td><strong>PubMed</strong></td><td><a href="https://pubmed.ncbi.nlm.nih.gov/{a.get("pmid", "")}/" class="pubmed-link">PMID: {a.get("pmid", "")}</a></td></tr>
            </table>
            {abstract_en}
            {abstract_zh}
          </div>"""
        
        sections_html += f"""
      <h2><span class="emoji">📌</span> {cat}</h2>
      {articles_html}"""
    
    html = f"""<!DOCTYPE html>
<html>
<head>
  <meta charset="UTF-8">
  <style>
    body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Microsoft YaHei', sans-serif; line-height: 1.6; color: #333; max-width: 800px; margin: 0 auto; padding: 20px; background: #f5f5f5; }}
    h1 {{ color: #2c3e50; border-bottom: 3px solid #3498db; padding-bottom: 10px; }}
    h2 {{ color: #2980b9; border-left: 4px solid #3498db; padding-left: 10px; margin-top: 30px; background: white; padding: 15px; border-radius: 8px; }}
    h3 {{ color: #34495e; margin-bottom: 5px; }}
    .meta {{ background: #ecf0f1; padding: 15px; border-radius: 8px; margin-bottom: 20px; }}
    .article {{ background: #fff; border: 1px solid #ddd; border-radius: 8px; padding: 20px; margin-bottom: 20px; box-shadow: 0 2px 4px rgba(0,0,0,0.1); }}
    .article:hover {{ box-shadow: 0 4px 8px rgba(0,0,0,0.15); }}
    .journal-badge {{ background: #3498db; color: white; padding: 3px 8px; border-radius: 4px; font-size: 12px; margin-right: 5px; display: inline-block; margin-bottom: 5px; }}
    .q1-badge {{ background: #27ae60; }}
    .q2-badge {{ background: #f39c12; }}
    .if-badge {{ background: #9b59b6; }}
    .abstract-en {{ background: #fff3cd; padding: 12px; border-radius: 8px; font-size: 13px; margin-bottom: 10px; border-left: 3px solid #ffc107; }}
    .abstract-zh {{ background: #d4edda; padding: 12px; border-radius: 8px; font-size: 14px; border-left: 3px solid #28a745; }}
    .pubmed-link {{ color: #3498db; text-decoration: none; font-weight: bold; }}
    .pubmed-link:hover {{ text-decoration: underline; }}
    .summary-table {{ width: 100%; border-collapse: collapse; margin: 20px 0; background: white; }}
    .summary-table th, .summary-table td {{ border: 1px solid #ddd; padding: 10px; text-align: left; }}
    .summary-table th {{ background: #3498db; color: white; }}
    .summary-table tr:nth-child(even) {{ background: #f8f9fa; }}
    .info-table {{ width: 100%; border-collapse: collapse; margin: 10px 0; }}
    .info-table td {{ padding: 6px 10px; border-bottom: 1px solid #eee; }}
    .info-table td:first-child {{ width: 100px; color: #666; font-weight: bold; background: #f8f9fa; }}
    .footer {{ text-align: center; color: #7f8c8d; margin-top: 30px; padding-top: 20px; border-top: 1px solid #ddd; font-size: 12px; }}
    .emoji {{ font-size: 1.2em; }}
    .title-zh {{ color: #2980b9; font-size: 1.1em; }}
    .title-en {{ color: #7f8c8d; font-style: italic; font-size: 0.95em; }}
  </style>
</head>
<body>

<h1>📋 {fmt(week_ago)} - {fmt(today)} 研究热点简报</h1>

<div class="meta">
  <p><strong>生成时间:</strong> {today.strftime("%Y年%m月%d日 %H:%M")}</p>
  <p><strong>文献来源:</strong> PubMed</p>
  <p><strong>期刊筛选:</strong> JCR Q1/Q2</p>
</div>

<h2><span class="emoji">📊</span> 本周摘要</h2>
<table class="summary-table">
  <tr>
    <th>板块</th>
    <th>文献数</th>
    <th>重点内容</th>
  </tr>
  {summary_rows}
</table>

{sections_html}

<div class="footer">
  <p>🤖 {sender_name} 自动生成 | 数据来源: PubMed NCBI</p>
</div>

</body>
</html>"""
    
    return html


if __name__ == "__main__":
    import sys
    if len(sys.argv) > 1:
        articles = json.loads(open(sys.argv[1], "r", encoding="utf-8").read())
    else:
        articles = []
    
    html = generate_briefing_html(articles)
    print(html)
