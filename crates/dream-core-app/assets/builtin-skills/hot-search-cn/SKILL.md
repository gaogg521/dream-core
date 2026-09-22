---
name: hot-search-cn
display_name: "全网热搜聚合"
description: 全平台热搜聚合：微博、知乎、百度、B站、抖音、36氪、贴吧、头条热榜，一键获取全网热点趋势，支持单平台与跨平台对比。触发词：热搜、热点、今天有什么热点、全网热榜。触发：用户提出「全网热搜聚合」相关需求时使用（常见说法：全网热搜聚合、全网热搜聚合；英文：hot/search/cn）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---

# 全平台热搜聚合

## 定位
聚合国内主流平台热搜榜单，快速掌握全网热点趋势，支持单平台查询和跨平台聚合对比。

## 脚本
- `scripts/hot_search.py` — 多平台热榜采集器（纯标准库urllib，零依赖）

## 使用方式
```bash
python scripts/hot_search.py <command> [options]
```

## 可用命令
- `all` — 聚合所有平台热搜
- `weibo` — 微博热搜
- `zhihu` — 知乎热榜
- `baidu` — 百度热搜
- `bilibili` — B站热门
- `douyin` — 抖音热点
- `toutiao` — 今日头条热榜
- `36kr` — 36氪热榜
- `tieba` — 贴吧热议
- `compare` — 跨平台热点对比（同一话题出现在几个平台）

## 数据来源
- 各平台公开热搜API/网页
- 无需API Key

## 输出格式
JSON，每条含 title/hot/platform/rank 字段。
