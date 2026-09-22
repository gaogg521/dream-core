# 发布前逐项清单与常见坑

## 写稿清单

- [ ] 关键事实已联网核实，观点类表述不写成硬事实
- [ ] 全角标点、无「本文看点」、章节标题 4-5 字
- [ ] 文末落款按账号惯用风格统一（无惯用则用简洁中性落款）
- [ ] 工具类文章附可复制安装命令
- [ ] 系列文章结尾按用户要求引导互动

## 元数据清单（frontmatter）

- [ ] title ≤64B（中文≈3B/字）
- [ ] description ≤120B（会渲染成文首导语引用块，写一句有钩子的）
- [ ] author ≤8B，不带间隔符后缀
- [ ] cover=绝对路径且文件存在
- [ ] 正文图 `![](assets/xx.png)` ≥2 张，全部 PNG/JPG，无 SVG
- [ ] `preflight.py` 全绿

## 发布清单

- [ ] 凭证就绪：`WECHAT_APP_ID` / `WECHAT_APP_SECRET` 已导出，调用 IP 在白名单
- [ ] `publish_wenyan.py --dry-run` 通过（title/summary 字节、图数）
- [ ] 正式发布拿到 media_id
- [ ] `--verify` 核实：图数、中文字数

## 常见坑速查

| 症状 | 原因 | 修法 |
|------|------|------|
| title exceeds WeChat UTF-8 byte limit | 标题 >64B | 压到 ≤21 个中文字 |
| 发布后正文裂图 | wenyan 输出相对路径 `assets/xx.png` | 用 publish_wenyan.py（自动上传图片换微信链接），别直接喂 wenyan 原始输出 |
| 40007 media_id 非法 | 封面没走 add_material | 用 publish_wenyan.py 完整链路，不要手动拼 media_id |
| 40164 IP 不在白名单 | 调用环境 IP 未备案 | 把出口 IP 加入公众号后台白名单 |
| 40001 invalid credential | secret 错或 token 过期 | 核对凭证后重跑（token 每次运行自动重取） |
| cairosvg: CAIRO_STATUS_WRITE_ERROR | 输出目录不存在 | 先 mkdir assets |
| cairosvg: ModuleNotFoundError | pip 环境不对 | `pip install cairosvg Pillow` 后重跑 |
| 主题不生效 | 未 `wenyan theme --add`，或改 CSS 没重新 add | `wenyan theme --add --name <名> --path <css>` 后重渲染 |
