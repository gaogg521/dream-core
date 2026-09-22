# 研究方法 / Research Methods（中英对照 / Bilingual）

## 1. 信源分级 / Source Grading

| 级别 / Tier | 中文范围 | English scope | 使用规则 / Usage rule |
|---|---|---|---|
| 一级 Tier 1 | 上市公司财报与招股书、监管备案与政策原文、一级媒体报道、同行评审论文、官方统计数据 | Public filings, regulatory documents, tier-one media, peer-reviewed papers, official statistics | 可直接支撑强结论 |
| 二级 Tier 2 | 行业媒体、知名分析机构报告、头部从业者公开分享、行业协会数据 | Trade press, established analyst reports, public talks by recognized practitioners, industry associations | 需两个以上交叉验证 |
| 三级 Tier 3 | 自媒体、论坛帖子、社群讨论、未经证实的传闻 | Influencer posts, forums, community discussion, unverified claims | 只做线索，不做结论 |

中文规则：强结论 = 至少三个独立信源，其中至少一个一级。只有三级信源的判断必须标注"线索，待验证"。

EN rule: A strong claim needs three or more independent sources including at least one Tier 1. Tier-3-only findings are labeled as leads pending verification.

## 2. 市场分层 / Market Sizing (TAM / SAM / SOM)

| 层级 | 中文定义 | English | 常用测算方式 |
|---|---|---|---|
| TAM | 全部潜在市场，假设拿到 100% 份额 | Total addressable market | 自上而下：行业报告总量 × 相关占比 |
| SAM | 在现有能力与地域范围内可服务的市场 | Serviceable available market | TAM × 可覆盖的地域与细分占比 |
| SOM | 考虑竞争后现实可得的份额 | Serviceable obtainable market | 自下而上：目标客户数 × 客单价 × 可获份额 |

中文规则：

- 两种口径都要算，差距超过 3 倍时说明假设有问题，必须在报告中并列呈现并解释差异
- 一律给区间（如 80-120 亿），不给孤值
- 必须写明测算假设清单，便于后续复核

EN rule:

- Compute both top-down and bottom-up. If they differ by more than 3x, surface both and explain the gap.
- Always give a range, never a point value.
- List the assumptions so the math can be rechecked later.

## 3. 趋势生命周期 / Trend Lifecycle

| 阶段 / Stage | 中文特征 | Signals | 产品含义 / Implication |
|---|---|---|---|
| 萌芽 Emerging | 少数先行者在做，主流没反应，技术不成熟 | 专利与开源项目活跃，商业案例稀少 | 适合小规模技术验证，不做大规模投入 |
| 上升 Growth | 资本与玩家涌入，媒体开始报道，客户开始询问 | 融资事件密集，头部厂商发布同类产品 | 进入窗口期，需要速度 |
| 成熟 Mature | 格局基本确定，价格战出现，差异化变难 | 并购增多，毛利率下滑 | 只能靠细分或成本优势进入 |
| 衰退 Declining | 替代技术出现，需求下滑 | 头部厂商停止投入，客户迁移 | 不建议进入，考虑退出 |

中文判断方法：看三个指标交叉——资本流向（融资笔数与金额的趋势）、供给密度（头部厂商同类产品发布节奏）、需求信号（搜索量与销售线索的变化）。

EN method: Cross three indicators - capital flow (funding count and size trend), supply density (launch cadence of tier-one vendors), and demand signal (search volume and inbound lead trend).

## 4. 趋势 vs 噪声判定 / Trend or Noise

中文：同时满足以下三条才算趋势，否则记为噪声。

1. 三个以上独立信源提及
2. 跨越至少两个季度仍在持续
3. 至少有一个可观测的量化指标在变化（搜索量、融资额、出货量、招聘数）

EN: All three must hold, otherwise log it as noise: three or more independent sources; persistence across at least two quarters; at least one observable quantitative indicator moving (search volume, funding, shipments, job postings).

信号强度分级 / Signal strength:

| 强度 | 中文判定 | English |
|---|---|---|
| 强 | 三条全满足，且量化指标连续两个季度单向变化 | All three met with two quarters of monotonic movement |
| 中 | 满足两条，量化指标有变化但波动 | Two met, indicator moves but fluctuates |
| 弱 | 只满足一条，仅有个别提及 | One met, isolated mentions only |

## 5. 技术成熟度 / Technology Maturity

中文：用 TRL（技术就绪水平）简化为四档判断。

| 档位 | 中文描述 | English | 商业化含义 |
|---|---|---|---|
| 实验室 Lab | 论文或原型阶段 | Research or prototype | 3 年以上才可能商用 |
| 可用 Viable | 有开源或内测产品，但稳定性不足 | Working but unstable | 1-3 年，可做技术预研 |
| 可商用 Production | 有成熟商业产品与付费客户 | Commercial products with paying customers | 现在可评估引入 |
|  commoditized | 已成为基础设施，价格快速下降 | Commoditized infrastructure | 直接采购，不要自研 |

EN: Simplified four-level TRL: Lab (papers or prototypes, 3+ years out), Viable (working but unstable, 1-3 years), Production (paying customers commercialized, evaluate now), Commoditized (infrastructure, buy rather than build).

## 6. 竞品格局分析 / Competitive Landscape

中文：必须覆盖四类，缺一类结论就有偏差。

| 类型 / Type | 中文定义 | English | 关注点 |
|---|---|---|---|
| 直接对手 | 同样产品、同样人群 | Direct competitors | 功能对比、定价、份额 |
| 间接对手 | 不同形态但解决同一问题 | Indirect competitors | 替代威胁、切换成本 |
| 新进入者 | 初创与跨界玩家 | Emerging players | 融资情况、差异化打法 |
| 替代方案 | 自建、人工、Excel、不解决 | Substitutes and DIY | 用户为什么不用任何产品 |

中文输出：竞品格局图（二维坐标轴：横轴为价格或复杂度，纵轴为能力或覆盖度）+ 空白地带标注。

EN output: A two-axis landscape map (price or complexity on one axis, capability or coverage on the other) with white space called out.

## 7. 进入时机判断 / Entry Timing

中文：用四个问题做判断，任一答案为否则应暂缓。

1. 技术是否可商用（不在实验室阶段）
2. 需求是否已被验证（有付费意愿，不只是口头兴趣）
3. 是否还有差异化空间（空白地带是否足够大）
4. 我们是否有独特资产（数据、渠道、技术、成本）

四个"是"→ 建议进入；三个"是"→ 建议小规模试点；两个及以下 → 建议观察，并明确列出需要看到什么信号才重新评估。

EN: Four questions. Any "no" means wait: is the technology production-ready; is demand validated with willingness to pay; is there differentiation space; do we hold a unique asset (data, channel, technology, cost). Four yes means enter; three means pilot; two or fewer means watch, with explicit signals that would trigger re-evaluation.

## 8. 引注格式 / Citation Format

中文：

```
[1] 【标题】（【机构】，【发布日期】，一级信源）https://example.com
```

英文：

```
[1] Title (Publisher, publication date, Tier 1) https://example.com
```

中文规则：正文中的每条关键事实后跟方括号编号；报告末尾给出完整信源清单；检索日期统一标注在信源清单顶部。

EN rule: Place a bracketed number after every key fact in the body, provide the full list at the end, and stamp the search date at the top of the source list.
