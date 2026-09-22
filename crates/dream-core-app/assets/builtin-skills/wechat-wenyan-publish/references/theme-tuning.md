# 主题调优指南（tuning-guide）

wenyan 自定义主题 = 一个 `#wenyan` 选择器的 CSS 文件。渲染时被 wenyan 解析（csstree）并转成**内联样式**打在对应元素上，与内置主题同机制，微信端稳定。

## 色板映射（每套主题只改 4 个色值）

| CSS 色值 | 作用 | redwhite | green | blue | gray |
|----------|------|----------|-------|------|------|
| `ACCENT`（标题/下划线文字/行内代码字/链接） | 主强调色 | #DC2626 | #059669 | #2563EB | #52525B |
| `UNDERLINE`（strong 下划线 / 引用左竖条 / 分割线） | 淡色标记 | #FECACA | #A7F3D0 | #BFDBFE | #E4E4E7 |
| `TINT`（引用底 / 表头底） | 极浅底色 | #FFF7F7 | #ECFDF5 | #EFF6FF | #F4F4F5 |
| `CODETINT`（行内代码底） | 代码浅底 | #FEF2F2 | #F0FDF4 | #F5F8FF | #F4F4F5 |

新增配色：复制任一主题，替换上面 4 个色值，`wenyan theme --add --name <新名> --path <css>` 即可。

## 关键 CSS 片段（不要删）

```css
/* 正文观感：15px / 1.8 / 深灰，无字间距/两端对齐/缩进 */
#wenyan p { font-size: 15px; line-height: 1.8; color: #374151; margin: 1em 0; }

/* 标题稳定：一行居中，主题色加粗，无装饰 */
#wenyan h2 { text-align: center; font-size: 1.25em; font-weight: 800; color: ACCENT; margin: 1.6em 0 1em; }

/* 重点标记：markdown **关键词** → 淡色下划线（红白 7d 手法） */
#wenyan strong { color: #1C1917; font-weight: 600; border-bottom: 2px solid UNDERLINE; }

/* 引用块：淡色左竖条 + 极浅底 */
#wenyan blockquote { border-left: 3px solid UNDERLINE; background: TINT; color: #374151; padding: 0.6em 1em; }
```

## 微信渲染注意

- 不要把 `font-size` / `border-bottom` 打在 `<strong>` 上混多个字号——微信会自动"纠正"。本主题把字号统一放 `p`，下划线用 `strong` 的 `border-bottom`，互不干扰。
- 每段一个字号（wenyan 每 markdown 段落一个 `<p>`），行内代码靠 `p code` 降字号，符合微信规则。
- 正文图必须 PNG/JPG（微信不接受 SVG）；正文图片链接由 `publish_wenyan.py` 自动经 uploadimg 上传换取微信链接。

## 常见问题

- **主题不生效**：确认已 `wenyan theme --add`（写入 `~/.config/wenyan-md/themes/`）；渲染用 `-t <名>`；改 CSS 后必须重新 add 覆盖。
- **图片发布后裂图**：wenyan 输出是相对路径 `assets/xx.png`，直接喂给微信 API 会找不到文件 → 必须用 `publish_wenyan.py`（它自动上传图片换取微信链接），不要直接喂 wenyan 的原始输出。
- **标题超长报错**：`title exceeds WeChat UTF-8 byte limit` → 标题压到 ≤21 个中文字；摘要 ≤40 字。
