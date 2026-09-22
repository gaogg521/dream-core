#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""一键：markdown → wenyan 自定义主题渲染 → 直连微信 API → 公众号草稿 →（可选）核实

用法：
  python3 publish_wenyan.py article.md [--theme redwhite|green|blue|gray] [--title X]
      [--summary Y] [--author NAME] [--cover cover.png] [--dry-run] [--verify] [--no-footnote]

凭证（必须由用户提供，本脚本不内置）：
  export WECHAT_APP_ID=wx...
  export WECHAT_APP_SECRET=...
  且调用方 IP 需在公众号后台「安全中心」白名单内，否则 API 返回 40164。

说明：
  - 正文图片先经 media/uploadimg 换成微信链接（外链图在正文里不可靠）
  - 封面经 material/add_material 上传为永久素材拿 thumb_media_id
  - 最后 draft/add 建草稿；--verify 用 draft/get 核实图数与字数
  - API 调用全部用标准库 urllib，零第三方依赖
"""
import argparse
import json
import mimetypes
import os
import re
import subprocess
import sys
import tempfile
import urllib.parse
import urllib.request
import uuid
from pathlib import Path

API_BASE = "https://api.weixin.qq.com/cgi-bin"
VALID_THEMES = ["redwhite", "green", "blue", "gray"]


# ──────────────────────────────────────────────
#  HTTP helpers（标准库 multipart）
# ──────────────────────────────────────────────
class ApiError(SystemExit):
    def __init__(self, where: str, payload: dict):
        code = payload.get("errcode", "?")
        msg = payload.get("errmsg", "unknown")
        hint = ""
        if code == 40164:
            hint = "\n[提示] 40164 = 调用 IP 不在公众号白名单。请把本机出口 IP 加入公众号后台「设置与开发→安全中心→IP 白名单」后重试。"
        elif code == 40001:
            hint = "\n[提示] 40001 = 凭证无效。核对 WECHAT_APP_SECRET，或稍后重取 access_token。"
        elif code == 40007:
            hint = "\n[提示] 40007 = media_id 非法。封面必须先经 material/add_material 上传。"
        super().__init__(f"{where} 失败 errcode={code}: {msg}{hint}")


def _post_multipart(url: str, file_field: str, file_path: Path) -> dict:
    boundary = uuid.uuid4().hex
    mime = mimetypes.guess_type(file_path.name)[0] or "application/octet-stream"
    with open(file_path, "rb") as f:
        data = f.read()
    body = b"".join([
        f"--{boundary}\r\n".encode(),
        f'Content-Disposition: form-data; name="{file_field}"; filename="{file_path.name}"\r\n'.encode(),
        f"Content-Type: {mime}\r\n\r\n".encode(),
        data,
        f"\r\n--{boundary}--\r\n".encode(),
    ])
    req = urllib.request.Request(
        url, data=body, method="POST",
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}"},
    )
    with urllib.request.urlopen(req, timeout=120) as resp:
        payload = json.loads(resp.read().decode("utf-8"))
    if "errcode" in payload and payload["errcode"]:
        raise ApiError(f"POST {url.split('?')[0]}", payload)
    return payload


def _post_json(url: str, payload: dict) -> dict:
    req = urllib.request.Request(
        url, data=json.dumps(payload, ensure_ascii=False).encode("utf-8"), method="POST",
        headers={"Content-Type": "application/json; charset=utf-8"},
    )
    with urllib.request.urlopen(req, timeout=60) as resp:
        result = json.loads(resp.read().decode("utf-8"))
    if "errcode" in result and result["errcode"]:
        raise ApiError(f"POST {url.split('?')[0]}", result)
    return result


# ──────────────────────────────────────────────
#  WeChat API steps
# ──────────────────────────────────────────────
def get_access_token(appid: str, secret: str) -> str:
    qs = urllib.parse.urlencode({"grant_type": "client_credential", "appid": appid, "secret": secret})
    with urllib.request.urlopen(f"{API_BASE}/token?{qs}", timeout=30) as resp:
        payload = json.loads(resp.read().decode("utf-8"))
    if "access_token" not in payload:
        raise ApiError("获取 access_token", payload)
    return payload["access_token"]


def upload_content_image(token: str, img: Path) -> str:
    """正文图片 → 微信临时链接（uploadimg，图文消息内用）"""
    payload = _post_multipart(f"{API_BASE}/media/uploadimg?access_token={token}", "media", img)
    return payload["url"]


def upload_cover_material(token: str, cover: Path) -> str:
    """封面 → 永久图片素材（add_material），返回 media_id 供草稿 thumb 用"""
    payload = _post_multipart(f"{API_BASE}/material/add_material?type=image&access_token={token}", "media", cover)
    return payload["media_id"]


def add_draft(token: str, article: dict) -> str:
    payload = _post_json(f"{API_BASE}/draft/add?access_token={token}", {"articles": [article]})
    return payload["media_id"]


def get_draft(token: str, media_id: str) -> dict:
    return _post_json(f"{API_BASE}/draft/get?access_token={token}", {"media_id": media_id})


# ──────────────────────────────────────────────
#  Pipeline
# ──────────────────────────────────────────────
def parse_frontmatter(md: Path):
    """粗解析 frontmatter：title/cover/author/description"""
    text = md.read_text(encoding="utf-8")
    meta = {}
    m = re.match(r"^---\s*\n(.*?)\n---", text, re.DOTALL)
    if m:
        for line in m.group(1).splitlines():
            if ":" in line:
                k, v = line.split(":", 1)
                meta[k.strip().lower()] = v.strip().strip('"').strip("'")
    return meta


def rewrite_body_images(token: str, html: str, base_dir: Path) -> tuple[str, int]:
    """把正文里的本地图片经 uploadimg 换成微信链接，返回 (新html, 上传数)"""
    count = 0

    def repl(m: re.Match) -> str:
        nonlocal count
        src = m.group(1)
        if src.startswith(("http://", "https://")):
            return m.group(0)
        local = (base_dir / src).resolve()
        if not local.is_file():
            raise SystemExit(f"ERROR: 正文图片不存在: {src}")
        url = upload_content_image(token, local)
        count += 1
        return f'src="{url}"'

    html = re.sub(r'src="([^"]+\.(?:png|jpe?g|webp|gif))"', repl, html, flags=re.IGNORECASE)
    return html, count


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("md", type=Path)
    ap.add_argument("--theme", default="redwhite", choices=VALID_THEMES)
    ap.add_argument("--title")
    ap.add_argument("--summary")
    ap.add_argument("--author", default="")
    ap.add_argument("--cover")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--verify", action="store_true")
    ap.add_argument("--no-footnote", action="store_true")
    args = ap.parse_args()

    md = args.md.resolve()
    meta = parse_frontmatter(md)

    title = args.title or meta.get("title") or md.stem
    cover = Path(args.cover or meta.get("cover") or "").resolve()
    if not cover.is_file():
        raise SystemExit(f"ERROR: cover 不存在: {cover}")
    author = args.author or meta.get("author") or ""
    summary = args.summary or meta.get("description") or meta.get("summary") or ""

    # 0) 发布需要凭证；dry-run 不需要
    if not args.dry_run:
        appid = os.environ.get("WECHAT_APP_ID", "").strip()
        secret = os.environ.get("WECHAT_APP_SECRET", "").strip()
        if not (appid and secret):
            raise SystemExit(
                "ERROR: 未设置 WECHAT_APP_ID / WECHAT_APP_SECRET 环境变量。\n"
                "  本脚本不内置任何凭证；请提供你自己的公众号 AppID/AppSecret，\n"
                "  或改用 --dry-run 只渲染 HTML 手动粘贴。"
            )
        print("  获取 access_token ...")
        token = get_access_token(appid, secret)

    # 1) wenyan 渲染
    cmd = ["wenyan", "render", "-f", str(md), "-t", args.theme, "-h", "github"]
    if args.no_footnote:
        cmd.append("--no-footnote")
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=180)
    if proc.returncode != 0:
        raise SystemExit(f"wenyan render 失败: {proc.stderr[:500]}")
    html = proc.stdout
    print(f"  渲染完成：theme={args.theme}, {len(html)} 字符")

    if args.dry_run:
        with tempfile.NamedTemporaryFile("w", suffix=".html", delete=False, encoding="utf-8") as f:
            f.write(html)
            print(f"  dry-run 输出: {f.name}（未发布）")
        return

    # 2) 正文图片上传换取微信链接
    html, n_imgs = rewrite_body_images(token, html, md.parent)
    print(f"  正文图片已上传 {n_imgs} 张")

    # 3) 封面上传为永久素材
    thumb_media_id = upload_cover_material(token, cover)
    print(f"  封面已上传: thumb_media_id={thumb_media_id}")

    # 4) 建草稿
    media_id = add_draft(token, {
        "title": title,
        "author": author,
        "digest": summary,
        "content": html,
        "thumb_media_id": thumb_media_id,
        "need_open_comment": 0,
        "only_fans_can_comment": 0,
    })
    print(f"media_id={media_id}")

    # 5) 可选：核实
    if args.verify:
        d = get_draft(token, media_id)
        item = (d.get("news_item") or [{}])[0]
        content = item.get("content", "")
        img_count = len(re.findall(r"<img\b", content))
        cn_chars = len(re.findall(r"[\u4e00-\u9fff]", re.sub(r"<[^>]+>", "", content)))
        print(f"核实 ✓ {item.get('title', title)} | 图: {img_count} | 中文字: {cn_chars}")


if __name__ == "__main__":
    main()
