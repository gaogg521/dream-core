---
name: self-media-distribution
display_name: "自媒体一键分发系统"
description: 可以实现多自媒体平台的多账号内容一键分发系统，支持抖音、视频号、快手等主流平台。触发：用户提出「自媒体一键分发系统」相关需求时使用（常见说法：自媒体一键分发系统、自媒体一键分发系统；英文：self/media/distribution）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# 自媒体一键分发系统

多平台（抖音 / 视频号 / 快手 / B站 / 小红书…）**多账号内容一键分发**的通用引擎，
外加配套的**登录态保活**能力。两个引擎都与具体项目解耦，通过回调注入宿主能力。

## 何时使用

- 要做一个「一次上传，多平台多账号发布」的分发工具（或给现有工具补全编排层）。
- 分发过程需要：任务展开、批量调度、实时进度、可中途取消、单账号失败跳过、
  定时发布（平台侧定时 / 本地定时）、预演（dry-run）等能力。
- 分发工具用 Playwright `storage_state` 存登录态，但**第二天就失效要重新扫码**，
  需要自动/手动续期与后台定时保活。
- 要为新平台扩展「发布适配器」或「登录失效判定标记」。

## 架构分工（重要）

本技能封装的是**编排层**，不含任何平台选择器和表单填写代码：

| 层 | 归属 | 原因 |
|---|---|---|
| 任务展开、批量调度、进度事件、取消、失败跳过、定时、dry-run | **本技能** `scripts/distribute.py` | 与平台无关，通用且稳定 |
| 登录态续期、保活调度、登录失效判定 | **本技能** `scripts/keepalive.py` | 与平台无关 |
| 打开发布页、填标题/话题、上传视频、点发布等 DOM 操作 | **宿主** `make_publisher` 回调 | 选择器天天变，必须留在宿主里维护 |

**不要**把平台选择器写进本技能——那是维护噩梦。宿主只需实现
`async upload(playwright)` 一个方法。

## 资源

- `scripts/distribute.py` — 通用一键分发引擎（`DistributionEngine` + `build_jobs`）。
- `scripts/keepalive.py` — 通用登录态保活引擎（`SessionKeepAlive`）。
- `scripts/selftest.py` — 分发引擎的无浏览器自测（`python scripts/selftest.py`，
  10 组场景 / 20+ 断言；改完引擎跑一遍，不用开浏览器）。
- `references/integration.md` — Flask 后端 + 原生前端的完整接线
  （发布接口、SSE 进度、保活接口、设置页、账号卡按钮、乔发现状对照）。
- `references/login-markers.md` — 各平台「登录已失效」判定标记表，含扩展方法。

## 快速开始 · 分发

宿主提供 5 个必需回调（+ 2 个可选），其余编排全部内置：

```python
from scripts.distribute import DistributionEngine, build_jobs

engine = DistributionEngine(
    provider=db,                    # 需实现 create_publish_records / update_publish_record / due_pending_records
    launch_browser=launch_browser,  # async (pw) -> browser
    open_context=open_context,      # async (browser, storage_state) -> context
    merge_states=merge_states,      # (cookie_files) -> 合并后的 storage_state
    publish_url=publish_url,        # (platform_type) -> 发布页 URL
    make_publisher=make_publisher,  # (job, publish_datetime, page, context, browser) -> 上传器对象
    reveal_window=reveal_window,    # 可选：dry-run 时把隐藏窗口显示出来
    cfg_get=db.cfg_get, cfg_set=db.cfg_set,   # 可选
)

plan = {
    "materials": [{"filePath": "v1.mp4", "fileName": "v1"}],
    "targets": [
        {"accountId": 1, "platformType": 3, "accountName": "抖A", "filePath": "a.json"},
        {"accountId": 2, "platformType": 4, "accountName": "快B", "filePath": "b.json"},
    ],
    "content": {"default": {"title": "默认标题", "tags": ["#通用"]},
                "byPlatform": {"3": {"title": "抖音标题", "tags": ["#抖"]}}},
    "schedule": {"mode": "now"},     # now | platform（平台侧定时）| local（本地定时）
    "covers": {"default": "cover.jpg"},
    "options": {"dryRun": False},
}

result = engine.submit(plan)         # -> {"batchId", "queued", "total"}
engine.start_scheduler()             # 可选：开启本地定时轮询
```

`make_publisher` 返回的上传器只需一个方法，引擎会先注入外部浏览器对象：

```python
class DouYinVideo:
    def __init__(self, job, publish_datetime, page=None, context=None, browser=None):
        self.job, self.publish_datetime = job, publish_datetime
        self.external_page, self.external_context, self.external_browser = page, context, browser

    async def upload(self, playwright):
        # 优先复用 self.external_page / external_context，别自己再开浏览器
        ...
```

引擎内置行为（均可在 `references/integration.md` 查到细节）：

- **一次启动浏览器、合并多账号登录态共用一个 context**，每个 job 开一个 tab。
- **同账号前序失败 → 跳过该账号后续 job**（避免连环失败刷屏）。
- **取消**：`cancel_event.set()` 后在当前 job 结束后停止，剩余标记「已取消」。
- **进度事件**：`EventBus` 发布 `info/warn/error/done`，可接 SSE 推给前端。
- **定时**：`platform` 模式把发布时间交给平台；`local` 模式入队，由
  `start_scheduler()` 轮询到点后再发（重启后仍可从 payload 重建任务）。
- **dry-run**：走完流程但不真提交，并 `reveal_window` 把窗口显示给用户核对。

## 快速开始 · 保活

```python
from scripts.keepalive import SessionKeepAlive

ka = SessionKeepAlive(
    provider=db,                      # 提供 list_accounts() -> [{id,name,type,storage_state_path}]
    open_context=open_ctx,            # async (browser, path) -> (context, page)
    content_url_fn=content_page_url,  # (ptype) -> 后台内容页 URL
    default_interval_hours=12,
    cfg_get=db.cfg_get, cfg_set=db.cfg_set,
)

ka.start()                            # 服务启动时调用，开启后台定时保活
# 手动刷新：await ka.keep_alive(acc)
# 状态查询：ka.status();   改频率：ka.set_interval(6)  # 6/12/24 小时
```

根因：平台 cookie 过期时间由服务端决定，且多为「滑动会话」——只要在过期前访问
一次后台就会顺延。旧逻辑登录后从不回访，于是很快到期。

## 关键约束

- Playwright 的 `context.storage_state(path=...)` 是续期核心：把刷新后的登录态
  重新落盘到 cookie 文件。
- 保活 / 分发窗口默认**隐藏**（headless 优先，必要时 `hide_until_ready` 丢到屏幕外
  `-32000,-32000`），过程中不弹任何可见窗口；只有 dry-run 才显式显示。
- 若平台**强制固定过期且不可续**，任何客户端手段都无效——保活会如实返回
  `valid=False`，只能重新扫码。

## 新增平台

```python
from scripts.keepalive import register_platform
register_platform(6, name="toutiao",
                  logged_out_text=["登录", "立即登录"],
                  logged_out_locators=['a:has-text("登录")'])
```

分发侧新增平台只需在宿主的 `make_publisher` 里注册一个上传器类，并在
`publish_url` 里补一条平台类型 → 发布页 URL 的映射。
