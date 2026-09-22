# 趋势研究 / Trend Researcher

## 一句话简介 / Summary

中文：做市场趋势、竞品格局与技术扫描研究，输出带信源分级、置信度与失效条件的决策简报，并落到"所以我们该做什么"。

EN: Market trend, competitive landscape, and technology scouting research delivered as decision briefs with source grading, confidence levels, and clear so-what actions.

## 能力清单 / Capabilities

中文：

- 市场分层：TAM / SAM / SOM 双口径测算，一律给区间不给孤值
- 趋势判断：生命周期阶段（萌芽、上升、成熟、衰退）+ 信号强度分级
- 趋势 vs 噪声判定：三信源 + 跨两季度 + 量化指标变化
- 竞品格局：直接对手、间接对手、新进入者、替代方案四类全覆盖
- 技术扫描：技术成熟度四档判断与商业化时间预判
- 进入时机判断：四问法给出进入 / 试点 / 观察建议
- 信源分级与引注：一级、二级、三级信源，正文编号 + 文末清单

EN: Market sizing with dual methods, trend lifecycle and signal strength, trend-versus-noise test, four-type competitive landscape, technology maturity, entry timing, and graded citations.

## 使用示例 / Example Prompts

中文：

- 帮我调研一下企业级 AI 客服这个赛道
- 竞品 A 和 B 的差别在哪？做个对比分析
- 这个趋势值不值得跟进？给我判断依据
- 有哪些新技术可能影响我们这个行业
- 现在进入这个市场还来得及吗

EN:

- Research the enterprise AI support market
- Compare competitor A and B, side by side
- Is this trend worth following? Show me the evidence
- What emerging technologies could affect our industry?

## 目录结构 / Structure

```text
product-trend-researcher/
├── SKILL.md                      # 技能定义（必须）
├── README.md                     # 本说明
└── references/
    ├── methods.md                # 信源分级、市场分层、生命周期、进入时机（中英对照）
    └── templates.md              # 五节简报模板 + 信源清单 + 一页速览（中英对照）
```

## 前置条件 / Prerequisites

中文：本技能依赖环境的联网检索能力。若当前环境无法联网，技能会明确告知用户"以下基于已有知识，置信度降级为低"，不会伪装成已检索。涉及付费数据库（Statista、CB Insights、PitchBook 等）或需登录可见的内容，技能会说明获取途径，不绕过付费墙。

EN: Depends on the environment's web search. If unavailable, the skill states that the answer is based on prior knowledge with low confidence and never implies a live search occurred. It does not bypass paywalls or logins.

## 安全说明 / Security Notes

中文：本技能只读公开信息，不执行脚本、不写入文件、不提交任何表单、不抓取需登录的内容。所有数据标注检索日期，超过 90 天提示复核。

EN: Read-only over public information. Runs no scripts, writes no files, submits no forms, and does not scrape authenticated content. Every figure is date-stamped with a 90-day recheck notice.

## 版本 / Version

1.0.0 ｜ 作者 Author: zxh ｜ 许可 License: MIT
