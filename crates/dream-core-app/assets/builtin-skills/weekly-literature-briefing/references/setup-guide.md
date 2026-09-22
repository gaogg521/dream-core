# 每周文献简报 - 配置指南

## 概述

本技能需要一个 `config.json` 配置文件。首次使用时，请复制 `config.template.json` 并填写你的信息。

```bash
cp config.template.json config.json
```

---

## 配置项详解

### 1. profile（个人信息）

| 字段 | 说明 | 示例 |
|------|------|------|
| `name` | 你的名字 | `"张三"` |
| `title` | 职称/单位 | `"XX医院血管外科主治医师"` |

---

### 2. search（检索配置）

#### topics（检索主题）

这是核心配置，定义你要追踪的文献领域。每个主题包含：

| 字段 | 说明 | 示例 |
|------|------|------|
| `name` | 分类名称（显示在简报标题中） | `"静脉血栓栓塞症"` |
| `queries` | PubMed 检索表达式数组（取 AND 交集） | 见下方说明 |
| `journal_filter` | 期刊白名单（可选，留空则不过滤） | `["J Vasc Surg", "Blood"]` |

**PubMed 检索表达式编写要点：**

```json
{
  "queries": [
    "(\"venous thromboembolism\"[MeSH] OR \"VTE\"[tiab] OR \"deep vein thrombosis\"[tiab])",
    "(\"anticoagulation\"[MeSH] OR \"anticoagulant\"[tiab] OR \"DOAC\"[tiab])"
  ]
}
```

- `[MeSH]` = 医学主题词（更精确）
- `[tiab]` = 标题/摘要检索（更广泛）
- 多个 `queries` 之间取 **AND** 交集
- 使用 PubMed Advanced Search 预先测试你的查询表达式

#### 其他搜索参数

| 字段 | 说明 | 默认值 |
|------|------|--------|
| `date_range_days` | 检索最近几天的文献 | `7` |
| `max_results_per_topic` | 每个主题最多返回文献数（全局默认值） | `20` |
| `min_journal_rank` | 最低期刊分区（需 easyScholar） | `"Q2"` |

> **推送篇数说明：** `max_results_per_topic` 是全局设定。如果某个主题需要差异化配置，可在对应 topic 中添加 `"max_results": 10` 来覆盖全局值。检索时先从 PubMed 获取该数量的文献，再经分区/期刊筛选，最终推送篇数可能少于设定值。

---

### 3. email（邮件推送）

| 字段 | 说明 |
|------|------|
| `enabled` | 是否启用邮件推送 |
| `smtp.host` | SMTP 服务器地址 |
| `smtp.port` | 端口（SSL: 465, TLS: 587） |
| `smtp.user` | 发件邮箱地址 |
| `smtp.auth_code` | SMTP 授权码（**不是登录密码**） |
| `recipient` | 收件邮箱 |
| `sender_name` | 发件人显示名 |

**常见邮箱 SMTP 配置：**

| 邮箱 | host | port | 获取授权码 |
|------|------|------|-----------|
| 126邮箱 | `smtp.126.com` | `465` | 设置 → POP3/SMTP → 开启 → 获取授权码 |
| QQ邮箱 | `smtp.qq.com` | `465` | 设置 → 账户 → 开启SMTP → 获取授权码 |
| Gmail | `smtp.gmail.com` | `465` | Google Account → App Passwords |
| Outlook | `smtp.office365.com` | `587` | 设置 → 邮件 → 同步邮件 |

---

### 4. feishu（飞书推送，可选）

| 字段 | 说明 |
|------|------|
| `enabled` | 是否启用飞书推送 |
| `app_id` | 飞书应用 App ID |
| `app_secret` | 飞书应用 App Secret |
| `open_id` | 接收消息的用户 Open ID |

> 如不需要飞书推送，设置 `enabled: false` 即可。

---

### 5. easyScholar（期刊分区查询）

| 字段 | 说明 |
|------|------|
| `enabled` | 是否启用期刊分区查询 |
| `secret_key` | easyScholar API Key |

**获取 easyScholar API Key：**
1. 注册 [easyScholar](https://easyscholar.cc/)
2. 进入个人中心 → API 接口 → 获取 secret_key

> 不使用 easyScholar 时，简报不会标注期刊分区和影响因子。

---

### 6. zotero（Zotero 文献管理，可选）

| 字段 | 说明 |
|------|------|
| `enabled` | 是否自动保存到 Zotero |
| `user_id` | Zotero User ID |
| `api_key` | Zotero API Key |

**获取 Zotero API Key：**
1. 登录 [www.zotero.org/settings/keys](https://www.zotero.org/settings/keys)
2. 点击 "Create new private key"
3. 勾选 "Allow library access" 和 "Allow notes access"
4. 复制生成的 Key

---

### 7. output（输出配置）

| 字段 | 说明 | 默认值 |
|------|------|--------|
| `report_dir` | 报告保存目录 | `/sandbox/workspace/outputs/briefing` |
| `write_ima_note` | 是否写入 IMA 笔记 | `true` |
| `notebook_name` | IMA 笔记本名称（留空则不指定） | `""` |

---

## 快速开始

1. 复制模板：`cp config.template.json config.json`
2. 填写必填项：`profile`、`search.topics`、`email`
3. 运行：`python scripts/run_briefing.py config.json`
4. 检查邮件是否收到

## 故障排除

| 问题 | 解决方案 |
|------|---------|
| PubMed 搜索无结果 | 检查 queries 表达式，在 PubMed 网站上先测试 |
| 邮件发送失败 | 检查 SMTP 配置和授权码是否正确 |
| 期刊分区无显示 | 确认 easyScholar 已启用且 secret_key 正确 |
| 配置文件读取失败 | 确认 config.json 存在且 JSON 格式正确 |
