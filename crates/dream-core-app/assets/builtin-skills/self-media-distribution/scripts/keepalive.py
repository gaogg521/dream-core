# -*- coding: utf-8 -*-
"""
playwright-session-keepalive —— 通用「滑动会话」登录态保活引擎

适用场景
--------
多平台（抖音 / 快手 / B站 / 小红书 / 视频号 等）账号自动化工具，使用
Playwright 的 storage_state（cookie 文件）保存登录态。这些平台大多为
「滑动会话」：只要在 cookie 过期前访问一次后台，服务端就会把过期时间
往后推。若登录后从不回访，cookie 常会在一两天内到期，表现为「第二天要重登」。

本模块把乔发（QiaoFa 一键自媒体分发）的保活逻辑抽成**与具体项目解耦**
的通用引擎：宿主只需提供 3 个回调（列举账号 / 打开带登录态的浏览器上下文 /
给出后台内容页 URL），即可获得：
  - 单账号静默保活（手动「刷新登录态」用）
  - 整轮批量保活（后台定时线程用）
  - 登录态写回（把刷新后的 storage_state 重新落盘，实现续期）

注意：若某平台**强制固定过期且不可续**（非滑动会话），任何客户端手段都
无法延长，只能重新扫码——此时 keep_alive 会如实返回 valid=False。

依赖：playwright（宿主已装即可；未装且未提供 launch_fn 时仅在使用默认
启动器时报错，不影响模块导入与静态分析）。
"""
from __future__ import annotations

import asyncio
import threading
from contextlib import asynccontextmanager
from datetime import datetime
from pathlib import Path
from typing import Awaitable, Callable, Optional

try:
    from playwright.async_api import Browser, BrowserContext, Page, async_playwright
except Exception:  # noqa: BLE001  (playwright 未安装时允许模块被导入)
    async_playwright = None  # type: ignore


# ================================================================ 平台登录标记
# 每个平台类型（int）映射到一组「登录已失效」特征。只要打开后台内容页后命中
# 任一特征，即判定为已退出登录，需要重新扫码。可运行时用 register_platform 扩展。
PLATFORM_LOGIN_MARKERS: dict = {
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


def register_platform(ptype: int, **markers) -> None:
    """增加或覆盖某平台的登录失效特征。

    markers 可含：
      - name: str
      - logged_out_text: list[str]        命中即视为未登录（按文本）
      - logged_out_locators: list[str]    命中即视为未登录（按 Playwright 选择器）
      - logged_out_url: list[str]         页面 URL 含其一即视为未登录
    """
    PLATFORM_LOGIN_MARKERS.setdefault(ptype, {"name": str(ptype)})
    PLATFORM_LOGIN_MARKERS[ptype].update(markers)


# ================================================================ 核心工具
async def refresh_cookie(context: "BrowserContext", storage_state_path: str) -> bool:
    """把当前（已刷新过的）登录态重新导出到登录文件，实现续期。

    在同步 / 抓取成功后调用，可把服务端顺延过的 cookie 落盘。
    """
    try:
        Path(storage_state_path).parent.mkdir(parents=True, exist_ok=True)
        await context.storage_state(path=storage_state_path)
        return True
    except Exception as e:  # noqa: BLE001
        print(f"[keepalive] 重新导出登录态失败 {storage_state_path}: {e}")
        return False


async def is_logged_in(page: "Page", ptype: int) -> bool:
    """按平台标记判定打开的后台页面是否仍处于登录态。"""
    markers = PLATFORM_LOGIN_MARKERS.get(ptype, {})
    try:
        for txt in markers.get("logged_out_text", []):
            if await page.get_by_text(txt).count():
                return False
        for loc in markers.get("logged_out_locators", []):
            if await page.locator(loc).count():
                return False
        for sub in markers.get("logged_out_url", []):
            if sub in page.url:
                return False
    except Exception:  # noqa: BLE001  (检测本身异常时，保守认为仍在线)
        return True
    return True


@asynccontextmanager
async def _browser_session(launch_fn):
    """统一浏览器生命周期：未提供 launch_fn 时用内置 chromium 启动器。"""
    pw = None
    if launch_fn is None:
        if async_playwright is None:
            raise RuntimeError("未安装 playwright，且未提供 launch_fn")
        pw = async_playwright()
        await pw.__aenter__()
        browser = await pw.chromium.launch(headless=True)
    else:
        browser = await launch_fn(headless=True)
    try:
        yield browser
    finally:
        try:
            await browser.close()
        except Exception:  # noqa: BLE001
            pass
        if pw is not None:
            try:
                await pw.__aexit__(None, None, None)
            except Exception:  # noqa: BLE001
                pass


async def keep_alive_account(
    acc: dict,
    open_context: Callable,
    content_url_fn: Callable[[int], str],
    launch_fn=None,
) -> dict:
    """保活单个账号：打开后台访问一次，仍在线则刷新并写回登录文件。

    参数
    ----
    acc : dict，含至少 {id, name, type, storage_state_path}
    open_context(browser, storage_state_path) -> (context, page)
        宿主负责应用 codecs 补丁 / init_script / 隐藏窗口等。
    content_url_fn(ptype) -> str
        返回该平台后台「内容管理 / 作品列表」页 URL。
    launch_fn(headless=True) -> Browser  （可选）
        宿主自定义浏览器启动；省略则用默认 chromium。

    返回
    ----
    {valid, refreshed, msg, account}
    """
    path = acc["storage_state_path"]
    if not Path(path).exists():
        return {"valid": False, "refreshed": False,
                "msg": "登录文件缺失，请重新扫码", "account": acc.get("name")}
    ptype = acc["type"]
    async with _browser_session(launch_fn) as browser:
        context, page = await open_context(browser, path)
        url = content_url_fn(ptype)
        try:
            await page.goto(url, wait_until="domcontentloaded", timeout=45000)
        except Exception:  # noqa: BLE001
            pass
        try:
            await page.wait_for_load_state("networkidle", timeout=8000)
        except Exception:  # noqa: BLE001
            pass
        await page.wait_for_timeout(2500)
        logged_in = await is_logged_in(page, ptype)
        if logged_in:
            refreshed = await refresh_cookie(context, path)
            msg = "已刷新登录态" if refreshed else "访问成功但写回失败"
        else:
            refreshed = False
            msg = "登录已过期，请重新扫码"
        return {"valid": logged_in, "refreshed": refreshed,
                "msg": msg, "account": acc.get("name")}


# ================================================================ 保活调度器
class SessionKeepAlive:
    """后台定时保活调度器（守护线程）。与宿主项目完全解耦。

    宿主接线示例（Flask 项目）：
        ka = SessionKeepAlive(
            provider=MyDb,                       # 提供 list_accounts()
            open_context=open_ctx,               # 见 keep_alive_account
            content_url_fn=content_page_url,
            default_interval_hours=12,
            cfg_get=db.cfg_get, cfg_set=db.cfg_set,
        )
        ka.start()          # 服务启动时调用
        # 手动刷新：asyncio.run(ka.keep_alive(acc))
        # 状态：ka.status()  设置频率：ka.set_interval(hours)
    """

    def __init__(self, provider, open_context, content_url_fn,
                 launch_fn=None, default_interval_hours: float = 12,
                 cfg_get: Optional[Callable] = None,
                 cfg_set: Optional[Callable] = None,
                 tag: str = "keepalive"):
        self.provider = provider
        self.open_context = open_context
        self.content_url_fn = content_url_fn
        self.launch_fn = launch_fn
        self.default_interval = default_interval_hours
        self.cfg_get = cfg_get
        self.cfg_set = cfg_set
        self.tag = tag
        self._stop = threading.Event()
        self._thread: Optional[threading.Thread] = None

    # ---- 配置（可持久化到宿主 DB） ----
    def get_interval(self) -> float:
        if self.cfg_get:
            v = self.cfg_get("keepalive_hours")
            if v is not None:
                return float(v)
        return self.default_interval

    def set_interval(self, hours: float) -> None:
        if self.cfg_set:
            self.cfg_set("keepalive_hours", hours)
        self.stop()
        if hours and hours > 0:
            self.start()

    # ---- 单次保活 ----
    def keep_alive(self, acc: dict) -> "Awaitable[dict]":
        return keep_alive_account(acc, self.open_context,
                                  self.content_url_fn, self.launch_fn)

    def run_pass(self) -> dict:
        """对所有账号跑一轮保活（后台线程定时调用）。"""
        ts = datetime.now().strftime("%Y-%m-%d %H:%M")
        if self.cfg_set:
            self.cfg_set("keepalive_last_pass", {"ts": ts})
        summary = {"ok": 0, "fail": 0}
        for acc in self.provider.list_accounts():
            if self._stop.is_set():
                break
            if not acc.get("storage_state_path"):
                continue
            try:
                res = asyncio.run(self.keep_alive(acc))
            except Exception as e:  # noqa: BLE001
                res = {"valid": False, "refreshed": False,
                       "msg": str(e)[:120], "account": acc.get("name")}
            self.record_run(acc["id"], res)
            if res.get("valid"):
                summary["ok"] += 1
            else:
                summary["fail"] += 1
            tag = "OK " if res.get("valid") else "FAIL"
            print(f"[{self.tag}] {tag} {acc.get('name')}：{res.get('msg')}")
        return summary

    def record_run(self, account_id, res: dict) -> None:
        if not self.cfg_set:
            return
        key = "keepalive_runs"
        data = self.cfg_get(key, {}) or {}
        data[str(account_id)] = {
            "ts": datetime.now().strftime("%Y-%m-%d %H:%M"),
            "valid": bool(res.get("valid")),
            "refreshed": bool(res.get("refreshed")),
            "msg": res.get("msg", ""),
        }
        self.cfg_set(key, data)

    # ---- 状态查询 ----
    def status(self) -> dict:
        h = self.get_interval()
        return {
            "enabled": h > 0,
            "intervalHours": h,
            "lastPass": self.cfg_get("keepalive_last_pass") if self.cfg_set else None,
            "runs": self.cfg_get("keepalive_runs", {}) if self.cfg_set else {},
        }

    # ---- 后台守护线程 ----
    def _loop(self) -> None:
        while not self._stop.is_set():
            interval = self.get_interval()
            if interval <= 0:
                self._stop.wait(3600)  # 关闭态：每小时复查一次配置
                continue
            if self._stop.wait(interval * 3600):
                break
            try:
                self.run_pass()
            except Exception as e:  # noqa: BLE001
                print(f"[{self.tag}] 保活轮次异常: {e}")

    def start(self) -> None:
        """启动后台保活线程（0 间隔视为关闭）。"""
        if self._thread and self._thread.is_alive():
            return
        self._stop.clear()
        self._thread = threading.Thread(target=self._loop, daemon=True)
        self._thread.start()
        print(f"[{self.tag}] 已启动（间隔 {self.get_interval()} 小时）")

    def stop(self) -> None:
        """停止后台保活线程。"""
        self._stop.set()
