#!/usr/bin/env python3
"""
每周文献简报 - 主运行脚本
整合 PubMed 检索、HTML 生成、邮件发送流程
"""

import json
import os
import sys
from datetime import datetime

# 将 scripts 目录加入 path
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from pubmed_search import run_search, load_config
from html_generator import generate_briefing_html
from email_sender import send_email


def run_briefing(config_path="config.json"):
    """
    执行完整的文献简报流程:
    1. 读取配置
    2. 检索 PubMed
    3. 生成 HTML 报告
    4. 保存报告文件
    5. 发送邮件
    6. 返回结果用于 IMA 笔记写入
    """
    print("=" * 50)
    print("📋 每周医学文献热点简报系统")
    print(f"🕐 开始时间: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    print("=" * 50)
    
    config = load_config(config_path)
    output_config = config.get("output", {})
    report_dir = output_config.get("report_dir", "/sandbox/workspace/outputs/briefing")
    
    # Step 1: 检索 PubMed
    print("\n📝 Step 1/4: 检索 PubMed...")
    articles = run_search(config_path)
    
    if not articles:
        print("\n⚠️ 本周无符合条件的文献")
        return {"status": "no_articles", "count": 0}
    
    # Step 2: 生成 HTML
    print(f"\n📝 Step 2/4: 生成 HTML 报告...")
    html = generate_briefing_html(articles, config_path)
    
    # Step 3: 保存文件
    print(f"\n📝 Step 3/4: 保存报告文件...")
    os.makedirs(report_dir, exist_ok=True)
    
    date_str = datetime.now().strftime("%Y-%m-%d")
    json_path = os.path.join(report_dir, f"briefing_{date_str}.json")
    html_path = os.path.join(report_dir, f"briefing_{date_str}.html")
    
    with open(json_path, "w", encoding="utf-8") as f:
        json.dump(articles, f, ensure_ascii=False, indent=2)
    
    with open(html_path, "w", encoding="utf-8") as f:
        f.write(html)
    
    print(f"  [OK] JSON: {json_path}")
    print(f"  [OK] HTML: {html_path}")
    
    # Step 4: 发送邮件
    print(f"\n📝 Step 4/4: 发送邮件...")
    date_range = config.get("search", {}).get("date_range_days", 7)
    subject = f"📋 每周文献热点简报 ({date_str})"
    
    email_result = send_email(subject, html, config_path, attachments=[json_path])
    
    # 汇总
    print(f"\n{'='*50}")
    print(f"✅ 简报生成完成！")
    print(f"📊 文献数量: {len(articles)}")
    print(f"📧 邮件状态: {'已发送' if email_result.get('success') else '未发送 (' + email_result.get('reason', '') + ')'}")
    print(f"{'='*50}")
    
    return {
        "status": "success",
        "count": len(articles),
        "articles": articles,
        "files": {"json": json_path, "html": html_path},
        "email": email_result
    }


if __name__ == "__main__":
    config_path = sys.argv[1] if len(sys.argv) > 1 else "config.json"
    result = run_briefing(config_path)
    print(f"\nResult: {json.dumps({k: v for k, v in result.items() if k != 'articles'}, ensure_ascii=False, indent=2)}")
