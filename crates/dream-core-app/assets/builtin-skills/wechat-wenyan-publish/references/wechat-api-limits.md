# 微信 API 硬约束（发布前必读）

这些限制是**微信 API 本身**的，超限直接报错。

## 字段限制

| 字段 | 上限 | 备注 |
|------|------|------|
| title | **64 字节** | 中文 ≈ 3B/字 → 实际约 ≤21 个中文字 |
| summary | **120 字节** | 中文 ≈ 3B/字 → 实际约 ≤40 字；超限用 python 数：`len(s.encode('utf-8'))` |
| author | **8 字节** | 两个汉字=6B（OK）；不要带间隔符后缀（如"作者·"=9B 且超限） |
| 正文图 | PNG/JPG/WebP/GIF | **SVG 会被微信拒绝**，一律转 PNG |
| 正文图片数 | 建议 2-3 张 | 单图文章在卡片流里画面单薄 |
| 封面 | 900×383 建议 | 经 material/add_material 上传为永久素材 |

## 字节数快速判定

```bash
python3 -c "print(len('标题'.encode('utf-8')))"   # 中文≈3B/字
```

## 检查顺序

1. `preflight.py`（title/summary/author 字节 + cover 存在性 + 图片引用与文件一致 + 图数）
2. 全部通过再 `publish_wenyan.py --dry-run`
3. 发布后 `--verify` 核实（图数/中文字数）

## 发布链路

```
markdown → wenyan render(-t theme)
→ 正文图片经 media/uploadimg 换成微信链接
→ 封面经 material/add_material 上传为永久素材
→ draft/add 建草稿（media_id）
→ --verify 时 draft/get 核实
```

## 常见 API 错误码

| errcode | 含义 | 处理 |
|---------|------|------|
| 40164 | 调用 IP 不在白名单 | 把出口 IP 加入公众号后台「安全中心 → IP 白名单」 |
| 40001 | 凭证无效 | 核对 AppSecret；access_token 有效期 7200 秒，过期重取 |
| 40007 | media_id 非法 | 封面必须先经 material/add_material 上传 |
| 45009 | 接口调用频率超限 | 稍后重试；draft API 有日配额 |
