# 各平台「登录已失效」判定标记

保活前先用这些特征判断后台页面是否仍处于登录态。**命中任一**即视为已退出登录，
返回 `valid=False`（需重新扫码）。这些标记与 `myUtils/auth.py` 的
`check_cookie` 登录特征保持一致。

## 标记类型

| 类型 | 字段 | 含义 |
|------|------|------|
| 文本命中 | `logged_out_text` | 页面出现该文本即视为未登录（如「手机号登录」「扫码登录」） |
| 选择器命中 | `logged_out_locators` | 用 Playwright 选择器定位到即视为未登录（按钮/表单/iframe/qrcode） |
| URL 命中 | `logged_out_url` | 页面 URL 含其一即视为未登录（多为 passport 登录域名） |

## 平台标记表

```python
PLATFORM_LOGIN_MARKERS = {
    1: {  # 小红书
        "name": "xiaohongshu",
        "logged_out_text": ["手机号登录", "扫码登录", "验证码登录"],
    },
    2: {  # 视频号
        "name": "weixin_channels",
        "logged_out_locators": [
            "text=扫码登录", "text=请使用微信扫码", "text=微信扫一扫",
            "text=登录后可使用", 'iframe[src*="login"]',
        ],
    },
    3: {  # 抖音
        "name": "douyin",
        "logged_out_text": ["手机号登录", "扫码登录"],
    },
    4: {  # 快手
        "name": "kuaishou",
        "logged_out_locators": [
            'a:has-text("立即登录")', 'button:has-text("登录")', "text=扫码登录",
            "text=快手扫码登录", "text=请扫码登录", 'img[alt="qrcode"]',
        ],
    },
    5: {  # B站
        "name": "bilibili",
        "logged_out_url": ["passport.bilibili.com"],
        "logged_out_locators": [
            'button:has-text("立即登录")', 'button:has-text("扫码登录")',
            'form[action*="login"]', ".login-form", 'input[type="password"]', ".qr-login",
        ],
    },
}
```

## 判定逻辑（is_logged_in）

```python
async def is_logged_in(page, ptype):
    m = PLATFORM_LOGIN_MARKERS.get(ptype, {})
    # 任意 logged_out_text 命中 -> 未登录
    # 任意 logged_out_locators 命中 -> 未登录
    # 任意 logged_out_url 命中 -> 未登录
    # 都不命中 -> 仍在线（True）
    # 检测自身抛异常 -> 保守返回 True（仍在线）
```

## 扩展新平台

```python
from scripts.keepalive import register_platform

# 头条 / 西瓜 / 知乎 / 微博 等：按实际后台登录页补标记
register_platform(6, name="toutiao",
                 logged_out_text=["登录", "立即登录"],
                 logged_out_locators=['a:has-text("登录")', 'img[alt="qrcode"]'])

register_platform(7, name="zhihu",
                 logged_out_url=["zhihu.com/signin"],
                 logged_out_locators=['button:has-text("登录")'])
```

## 维护提示

- 平台改版登录页文案时，优先更新 `logged_out_text` / `logged_out_locators`，
  不必动引擎代码。
- 若某平台改成「强制固定过期」，其 `is_logged_in` 永远返回 `valid=False`——
  这是正确行为，说明该平台不可续，只能重新扫码，请勿误删标记。
- 检测异常（超时、元素消失）一律保守判为「仍在线」，避免误报失效导致用户
  反复重登。
