# AI短剧剧本（ai-short-drama-script）· 本平台 技能包 V2

> 全网第一质量的 AI 短剧剧本输出引擎。v2.0.0 · 知识截止 2026-09 · author: ajie

## 定位

一句话需求 → **剧集圣经 + 人物小传 + 分集大纲 + 逐集可拍摄剧本 + AI 分镜表 + 平台投稿包** 一键成型；对标 2026 头部爆款结构，并联网校准平台政策（分账/审核/投稿），过十二维评分 ≥90 硬门禁。

## 包结构（13 文件）

```
ai-short-drama-script/
├── SKILL.md                              # 七阶段流水线 + 五模式路由 + 硬约束
├── README.md
├── references/
│   ├── script-formulas.md                # 题材公式×Hook五式×悬念八类×完播口径×政策驱动选题
│   ├── script-format-standard.md         # 双体系格式（投稿真人版 vs AI视频版）+过稿5标准+投稿12项
│   ├── platform-policy-2026.md           # 2026 平台分账/审核/投稿规则 + 完播口径 + 行业供给侧
│   ├── character-dialogue-craft.md       # 人物黄金三角+台词三字诀+画面写法+标志性动作
│   ├── ai-video-pipeline.md              # 角色一致性7方案+七列分镜表+五段式提示词+参数速查
│   ├── episode-template.md               # 体例速查（含投稿体系正文模板）
│   ├── red-line-checklist.md             # 合规红线 + 2026 AI 标识/溯源/备案新规
│   └── changelog.md
└── scripts/
    ├── script_score.py                   # 十二维评分门禁（--file/--text/--optimize/--self-test）
    └── monthly_evolution.py              # 月度进化引擎（--check/--tasks/--bump）
```

## 使用示例

- 「写一部 80 集都市逆袭短剧，投抖音，出大纲和前三集 + 分镜表」（A+D）
- 「精写第 10 集大反转」（B）
- 「帮我诊断优化这个剧本」→ 十二维诊断报告 + 改写版（C）
- 「按红果要求打包投稿」→ 小传+大纲+前5集+原创声明（E）
- 「检查资源库时效 / 更新到最新」→ 月度进化

## 本地安装
复制 `ai-short-drama-script/` 到 `~\.dream\skills\`，重启客户端或 `/reload-skills`。

## 上传开放平台
open.workbuddy.cn → 技能 → 上传 `ai-short-drama-script.zip` → 解析确认（表单另需手动上传 512×512 图标）→ 提交审核（约 7 个工作日）→ 发布。

## 自检
```
python scripts/script_score.py --self-test              # 高100/中27/低5，三档区分
python scripts/script_score.py --file x.md --optimize    # 评分 + 优化提示词
python scripts/monthly_evolution.py --check --tasks
```
