"""自媒体一键分发引擎（与项目解耦）

这是「自媒体一键分发系统」技能的核心可复用模块，从乔发（QiaoFa）真实分发
逻辑抽取而来，去掉了对具体项目（uploader.* / myUtils / qiaofa.db）的依赖，
只保留**通用编排层**；各平台的「表单填写 / 点击发布」由宿主通过回调注入。

设计要点（与乔发现网一致）：
  - 一次批量任务 batch = 多个 job（素材 × 账号 × 平台）
  - 单浏览器、单上下文（多账号 cookie 合并进一个 storage_state）、
    每 job 一个标签页，顺序执行，互不干扰
  - 每个 job 由宿主的 make_publisher() 构建上传器实例，引擎注入
    external_page / external_context / external_browser 后调用 app.upload(pw)
  - 通过内存事件总线（EventBus）实时推送进度，前端用 SSE 订阅即可
  - 支持「立即 / 平台侧定时 / 本地队列定时」三种模式、取消、按账号失败跳过、
    dry-run 预演

宿主只需提供 6 个回调即可接入任意 Playwright 多账号分发工具：
  provider          账号与发布记录的读写（list_accounts / create_publish_records /
                    update_publish_record / due_pending_records / cfg_get / cfg_set）
  launch_browser    启动浏览器（应用 codec 补丁、隐藏窗口）
  open_context      用合并后的 storage_state 建上下文（应用 init_script + 隐藏）
  merge_states      把多个账号 cookie 文件合并成一个 storage_state
  publish_url       平台类型 -> 发布页 URL
  make_publisher    (job, publish_datetime, page, context, browser) -> 上传器实例
（可选）reveal_window  dry-run 预演结束后把窗口拉回可视区供人工核对

详见 references/integration.md。
"""

from __future__ import annotations

import asyncio
import json
import threading
import time
import traceback
import uuid
from datetime import datetime
from typing import Any, Callable, Dict, List, Optional


# ---------------------------------------------------------------- 平台常量
PLATFORMS: Dict[int, Dict[str, Any]] = {
    1: {"key": "xiaohongshu", "name": "小红书", "color": "#ff2442", "icon": "小"},
    2: {"key": "tencent", "name": "视频号", "color": "#07c160", "icon": "视"},
    3: {"key": "douyin", "name": "抖音", "color": "#161823", "icon": "抖"},
    4: {"key": "kuaishou", "name": "快手", "color": "#ff6600", "icon": "快"},
    5: {"key": "bilibili", "name": "B站", "color": "#fb7299", "icon": "B"},
}

# 发布记录状态（与 qiaofa.db 保持一致）
ST_PENDING = 0
ST_RUNNING = 1
ST_SUCCESS = 2
ST_FAILED = 3
ST_CANCELLED = 4

STATUS_TEXT = {
    ST_PENDING: "等待中",
    ST_RUNNING: "发布中",
    ST_SUCCESS: "已发布",
    ST_FAILED: "失败",
    ST_CANCELLED: "已取消",
}


# ---------------------------------------------------------------- 工具函数
def normalize_publish_tags(tags: Any, max_count: Optional[int] = None) -> List[str]:
    """规范化话题标签：字符串按逗号/空格拆分，补 # 前缀，可选截断。"""
    if tags is None:
        return []
    if isinstance(tags, str):
        tags = [t for t in tags.replace(",", " ").split() if t.strip()]
    out = []
    for t in tags or []:
        t = str(t).strip()
        if not t:
            continue
        out.append(t if t.startswith("#") else "#" + t)
    if max_count:
        out = out[: max(0, int(max_count))]
    return out


def _load_payload(raw: Any) -> Dict[str, Any]:
    """从发布记录里还原任务 payload。

    兼容两种宿主存储方式：JSON 字符串（乔发/SQLite 的普遍做法）与已反序列化的 dict。
    解析失败一律返回 {}，由调用方判定为「任务数据损坏」。
    """
    if isinstance(raw, dict):
        return raw
    if not raw:
        return {}
    try:
        data = json.loads(raw)
    except (ValueError, TypeError):
        return {}
    return data if isinstance(data, dict) else {}


def _resolve_publish_datetime(job: "PublishJob"):
    """返回上传器用的发布时间。

    约定（必须与上传器内部一致）：
      - 立即发布：返回 int 0
      - 平台侧定时：返回 datetime
    不能返回 None，否则上传器的 `if self.publish_date != 0:` 会进定时分支后
    调 .month / .day 时炸 AttributeError。
    """
    if job.schedule_mode == "platform" and job.scheduled_at:
        s = str(job.scheduled_at).strip()
        for fmt in ("%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S"):
            try:
                return datetime.strptime(s, fmt)
            except ValueError:
                continue
        # 无法解析 -> 自动降级为立即发布
        return 0
    return 0


def spread_times(count: int, per_day: int = 1, start_days: int = 0,
                 hours: Optional[List[int]] = None) -> List[Optional[str]]:
    """给多条素材生成错峰发布时间（'YYYY-MM-DD HH:MM' 列表，None 表示立即）。

    这是一个自包含的简化实现：从 start_days 天后的每天 per_day 条均匀铺开。
    宿主若已有更精细的错峰算法（如整合包 utils.files_times），可在 make_publisher
    之外自行处理，本函数仅作兜底。
    """
    if count <= 0:
        return []
    per_day = max(1, int(per_day or 1))
    start_days = int(start_days or 0)
    hours = hours or [9, 12, 18, 21]
    out: List[Optional[str]] = []
    day = 0
    slot = 0
    while len(out) < count:
        for h in hours:
            if len(out) >= count:
                break
            dt = datetime.now().replace(hour=0, minute=0, second=0, microsecond=0)
            dt = dt.replace(day=dt.day + start_days + day, hour=h % 24)
            out.append(dt.strftime("%Y-%m-%d %H:%M"))
            if len(out) >= count:
                break
        day += 1
        if day > 365:
            # 极端情况保底，避免死循环
            while len(out) < count:
                out.append(None)
            break
    return out


# ---------------------------------------------------------------- 任务模型
class PublishJob:
    """单个发布任务（素材 × 账号 × 平台）。

    设计成可直接序列化为 payload 存入发布记录，本地定时到点后再从 JSON 还原。
    """

    __slots__ = (
        "platform_type", "account_id", "account_name", "cookie_file",
        "file_path", "file_name", "title", "tags", "desc", "cover",
        "category", "bili_type", "bili_partition", "schedule_mode",
        "scheduled_at", "dry_run", "spread", "record_id",
    )

    def __init__(self, platform_type: int, account_id=None, account_name=None,
                 cookie_file=None, file_path=None, file_name=None, title="",
                 tags=None, desc="", cover=None, category=None, bili_type=None,
                 bili_partition=None, schedule_mode="now", scheduled_at=None,
                 dry_run=False, spread=None, record_id=None):
        self.platform_type = int(platform_type)
        self.account_id = account_id
        self.account_name = account_name
        self.cookie_file = cookie_file
        self.file_path = file_path
        self.file_name = file_name
        self.title = (title or "").strip()
        self.tags = tags or []
        self.desc = desc or ""
        self.cover = cover
        self.category = category
        self.bili_type = bili_type
        self.bili_partition = bili_partition
        self.schedule_mode = schedule_mode or "now"
        self.scheduled_at = scheduled_at
        self.dry_run = bool(dry_run)
        self.spread = spread or {}
        self.record_id = record_id

    def to_dict(self) -> Dict[str, Any]:
        return {k: getattr(self, k) for k in self.__slots__}

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "PublishJob":
        job = cls(
            platform_type=d.get("platform_type"),
            account_id=d.get("account_id"),
            account_name=d.get("account_name"),
            cookie_file=d.get("cookie_file") or d.get("filePath"),
            file_path=d.get("file_path") or d.get("filePath"),
            file_name=d.get("file_name") or d.get("fileName"),
            title=d.get("title", ""),
            tags=d.get("tags") or [],
            desc=d.get("desc", ""),
            cover=d.get("cover"),
            category=d.get("category"),
            bili_type=d.get("bili_type"),
            bili_partition=d.get("bili_partition"),
            schedule_mode=d.get("schedule_mode", "now"),
            scheduled_at=d.get("scheduled_at"),
            dry_run=bool(d.get("dry_run", False)),
            spread=d.get("spread") or {},
            record_id=d.get("record_id"),
        )
        return job

    @property
    def label(self) -> str:
        meta = PLATFORMS.get(self.platform_type, {})
        name = meta.get("name", f"平台{self.platform_type}")
        return f"{name} / {self.account_name or '账号'} / {self.file_name or ''}"


def build_jobs(plan: Dict[str, Any]) -> List[PublishJob]:
    """把前端提交的发布计划展开为 job 列表。

    plan = {
      "materials": [{"filePath","fileName"} ...],
      "targets":   [{"accountId","platformType","accountName","filePath"(cookie)} ...],
      "content":   {"byPlatform": {"3": {"title","tags","desc",...}}, "default": {...}},
      "schedule":  {"mode":"now|platform|local", "time":"YYYY-MM-DD HH:MM", "spread": {...}},
      "covers":    {"3": "cover.jpg", "default": "cover.jpg"},
      "options":   {"dryRun":bool, "skipAccountCheck":bool}
    }
    """
    materials = plan.get("materials") or []
    targets = plan.get("targets") or []
    content = plan.get("content") or {}
    schedule = plan.get("schedule") or {}
    covers = plan.get("covers") or {}
    options = plan.get("options") or {}

    by_platform = content.get("byPlatform") or {}
    default_content = content.get("default") or {}

    mode = schedule.get("mode", "now")
    sched_time = (schedule.get("time") or "").strip() or None
    spread = schedule.get("spread") or {}

    jobs: List[PublishJob] = []
    for target in targets:
        ptype = int(target.get("platformType"))
        c = dict(default_content)
        c.update(by_platform.get(str(ptype)) or {})
        for mat in materials:
            jobs.append(PublishJob(
                platform_type=ptype,
                account_id=target.get("accountId"),
                account_name=target.get("accountName"),
                cookie_file=target.get("filePath"),
                file_path=mat.get("filePath"),
                file_name=mat.get("fileName"),
                title=(c.get("title") or "").strip(),
                tags=normalize_publish_tags(c.get("tags")),
                desc=c.get("desc") or "",
                cover=covers.get(str(ptype)) or covers.get("default"),
                category=c.get("category"),
                bili_type=c.get("biliType"),
                bili_partition=c.get("biliPartition"),
                schedule_mode=mode,
                scheduled_at=sched_time,
                dry_run=bool(options.get("dryRun")),
                spread=spread,
            ))
    return jobs


# ---------------------------------------------------------------- 事件总线
class EventBus:
    """按 batch_id 分发进度事件，支持多个 SSE 订阅者。"""

    def __init__(self):
        self._subs: Dict[str, List[Any]] = {}
        self._history: Dict[str, List[Dict[str, Any]]] = {}
        self._lock = threading.RLock()

    def subscribe(self, batch_id: str) -> Any:
        from queue import Queue
        q = Queue()
        with self._lock:
            self._subs.setdefault(batch_id, []).append(q)
            for ev in self._history.get(batch_id, []):
                q.put(ev)
        return q

    def unsubscribe(self, batch_id: str, q: Any) -> None:
        with self._lock:
            lst = self._subs.get(batch_id) or []
            if q in lst:
                lst.remove(q)
            if not lst:
                self._subs.pop(batch_id, None)

    def emit(self, batch_id: str, event: Dict[str, Any]) -> None:
        event = dict(event)
        event.setdefault("ts", datetime.now().strftime("%H:%M:%S"))
        with self._lock:
            hist = self._history.setdefault(batch_id, [])
            hist.append(event)
            if len(hist) > 500:
                del hist[:-500]
            for q in list(self._subs.get(batch_id, [])):
                q.put(event)

    def history(self, batch_id: str) -> List[Dict[str, Any]]:
        with self._lock:
            return list(self._history.get(batch_id, []))

    def clear(self, batch_id: str) -> None:
        with self._lock:
            self._history.pop(batch_id, None)

    def close(self, batch_id: str) -> None:
        """向该批次所有订阅者放入结束哨兵 None，让 SSE / 消费循环能干净退出。

        历史记录保留（便于回放），仅通知订阅者流已结束。
        """
        with self._lock:
            for q in list(self._subs.get(batch_id, [])):
                q.put(None)
            self._subs.pop(batch_id, None)


class BasePublisher:
    """宿主上传器应实现的上传器契约（可选继承，仅作文档）。

    宿主的上传器类（如 DouYinVideo / KSVideo / BilibiliVideo …）只需提供：
      - __init__(self, job, publish_datetime, cookie_file, ...) 构造时拿到任务与发布时间
      - async upload(self, playwright)                          执行「上传视频→填表→提交」
    引擎会在调用前注入 external_page / external_context / external_browser，
    上传器优先复用这些外部对象（而非自建浏览器），从而多 job 共享一个上下文。
    """

    def __init__(self, job: PublishJob, publish_datetime, page=None,
                 context=None, browser=None):
        self.job = job
        self.publish_datetime = publish_datetime
        self.external_page = page
        self.external_context = context
        self.external_browser = browser

    async def upload(self, playwright) -> None:  # pragma: no cover - 由宿主实现
        raise NotImplementedError("宿主需实现 upload(playwright)")


# ---------------------------------------------------------------- 引擎
class DistributionEngine:
    """通用一键分发引擎。

    宿主通过构造参数注入 6 个回调（见模块文档字符串），其余编排逻辑全部内置。
    """

    def __init__(self, provider, launch_browser: Callable, open_context: Callable,
                 merge_states: Callable, publish_url: Callable, make_publisher: Callable,
                 reveal_window: Optional[Callable] = None,
                 cfg_get: Optional[Callable] = None, cfg_set: Optional[Callable] = None,
                 bus: Optional[EventBus] = None,
                 playwright_factory: Optional[Callable[[], Any]] = None):
        """构造引擎。

        playwright_factory：可选。返回「异步上下文管理器」的可调用对象，
            `async with factory() as pw:` 得到的 pw 会传给 launch_browser 与各上传器。
            默认 None —— 此时内部 import playwright.async_api.async_playwright。
            注入该工厂可方便测试（传 fake）或让宿主接管驱动进程的启动方式。
        """
        self.provider = provider
        self.launch_browser = launch_browser
        self.open_context = open_context
        self.merge_states = merge_states
        self.publish_url = publish_url
        self.make_publisher = make_publisher
        self.reveal_window = reveal_window
        self.cfg_get = cfg_get or (lambda k, d=None: d)
        self.cfg_set = cfg_set or (lambda k, v: None)
        self.bus = bus or EventBus()
        self.playwright_factory = playwright_factory

        self._running: Dict[str, Dict[str, Any]] = {}
        self._running_lock = threading.RLock()
        self._scheduler_started = False

    # ---- 查询 / 取消 ----------------------------------------------------
    def running_batches(self) -> Dict[str, Any]:
        with self._running_lock:
            return {
                bid: {k: v for k, v in info.items() if k != "cancel"}
                for bid, info in self._running.items()
            }

    def cancel(self, batch_id: str) -> bool:
        with self._running_lock:
            info = self._running.get(batch_id)
        if not info:
            return False
        info["cancel"].set()
        self.bus.emit(batch_id, {"level": "warn",
                                 "msg": "已收到取消指令，将在当前任务结束后停止"})
        return True

    # ---- 提交批次 --------------------------------------------------------
    def submit(self, plan: Dict[str, Any]) -> Dict[str, Any]:
        """提交一个发布批次。返回 {batchId, recordIds, queued, total}。"""
        jobs = build_jobs(plan)
        if not jobs:
            raise ValueError("没有可执行的发布任务，请检查素材与账号选择")

        batch_id = f"B{datetime.now().strftime('%y%m%d%H%M%S')}{uuid.uuid4().hex[:4]}"
        mode = (plan.get("schedule") or {}).get("mode", "now")

        items = []
        for job in jobs:
            items.append({
                "accountId": job.account_id,
                "accountName": job.account_name,
                "platformType": job.platform_type,
                "filePath": job.file_path,
                "fileName": job.file_name,
                "title": job.title,
                "tags": job.tags,
                "coverPath": job.cover,
                "scheduleMode": mode,
                "scheduledAt": job.scheduled_at,
                "status": ST_PENDING,
                "payload": job.to_dict(),
            })
        record_ids = self.provider.create_publish_records(batch_id, items)
        for job, rid in zip(jobs, record_ids):
            job.record_id = rid

        # 本地队列定时：先入库排队，到点由调度器执行
        if mode == "local" and (plan.get("schedule") or {}).get("time"):
            self.bus.emit(batch_id, {"level": "info",
                                     "msg": f"已加入定时队列，将在 {plan['schedule']['time']} 自动发布"})
            return {"batchId": batch_id, "recordIds": record_ids,
                    "queued": True, "total": len(jobs)}

        cancel_event = threading.Event()
        with self._running_lock:
            self._running[batch_id] = {
                "batchId": batch_id,
                "total": len(jobs),
                "startedAt": datetime.now().strftime("%Y-%m-%d %H:%M:%S"),
                "cancel": cancel_event,
            }
        t = threading.Thread(target=self._batch_worker,
                             args=(batch_id, jobs, cancel_event), daemon=True)
        t.start()
        return {"batchId": batch_id, "recordIds": record_ids,
                "queued": False, "total": len(jobs)}

    # ---- 执行核心（可被测试注入 playwright） --------------------------
    async def run_jobs(self, batch_id: str, jobs: List[PublishJob],
                       cancel_event, playwright=None) -> tuple:
        """单浏览器多标签，顺序执行全部 job。返回 (ok_count, fail_count)。

        playwright 为 None 时，启动真实 Playwright；单元测试可传入 fake 对象
        （只要后续宿主回调 launch_browser/open_context 也用对应的 fake 实现）。
        """
        if playwright is None:
            factory = self.playwright_factory
            if factory is None:  # 延迟导入：无浏览器环境也能 import 本模块
                from playwright.async_api import async_playwright as factory
            async with factory() as pw:
                return await self._execute(batch_id, jobs, cancel_event, pw)
        return await self._execute(batch_id, jobs, cancel_event, playwright)

    async def _execute(self, batch_id, jobs, cancel_event, pw) -> tuple:
        ok_count = fail_count = 0
        self.bus.emit(batch_id, {"level": "info", "msg": "正在启动浏览器..."})
        try:
            browser = await self.launch_browser(pw)
        except Exception as e:  # noqa: BLE001
            self.bus.emit(batch_id, {"level": "error", "msg": f"浏览器启动失败：{e}"})
            for job in jobs:
                self.provider.update_publish_record(
                    job.record_id, status=ST_FAILED,
                    message=f"浏览器启动失败：{e}", finished=True)
            return 0, len(jobs)

        context = None
        try:
            # 视频号的 storage_state 放最后，origin 冲突时以其为准（与乔发一致）
            cookie_files = [
                j.cookie_file
                for j in sorted(jobs, key=lambda x: 1 if x.platform_type == 2 else 0)
            ]
            try:
                storage_state = self.merge_states(cookie_files)
            except Exception as e:  # noqa: BLE001
                storage_state = None
                self.bus.emit(batch_id, {"level": "warn",
                                         "msg": f"合并登录态失败，将尝试单账号登录态：{str(e)[:80]}"})

            context = await self.open_context(browser, storage_state)
            self.bus.emit(batch_id, {"level": "info",
                                     "msg": f"登录态已装载，共 {len(jobs)} 个发布任务"})

            failed_key = set()
            first_page = None

            for idx, job in enumerate(jobs, 1):
                rid = job.record_id
                label = job.label

                if cancel_event.is_set():
                    self.provider.update_publish_record(
                        rid, status=ST_CANCELLED, message="批次已取消", finished=True)
                    self.bus.emit(batch_id, {"level": "warn", "recordId": rid,
                                            "index": idx, "total": len(jobs),
                                            "status": "cancelled",
                                            "msg": f"[{idx}/{len(jobs)}] 已取消 {label}"})
                    continue

                key = (job.platform_type, job.account_id)
                if key in failed_key:
                    fail_count += 1
                    self.provider.update_publish_record(
                        rid, status=ST_FAILED,
                        message="同账号前序任务失败，已跳过", finished=True)
                    self.bus.emit(batch_id, {"level": "warn", "recordId": rid,
                                            "index": idx, "total": len(jobs),
                                            "status": "failed",
                                            "msg": f"[{idx}/{len(jobs)}] 跳过 {label}（同账号前序任务失败）"})
                    continue

                self.provider.update_publish_record(
                    rid, status=ST_RUNNING, started=True, message="发布中")
                self.bus.emit(batch_id, {"level": "info", "recordId": rid,
                                        "index": idx, "total": len(jobs),
                                        "status": "running",
                                        "msg": f"[{idx}/{len(jobs)}] 开始处理 {label}"})

                page = None
                try:
                    page = await context.new_page()
                    if first_page is None:
                        first_page = page
                    url = self.publish_url(job.platform_type)
                    if url:
                        try:
                            await page.goto(url, wait_until="commit", timeout=30000)
                            await page.wait_for_timeout(300)
                        except Exception as e:  # noqa: BLE001
                            self.bus.emit(batch_id, {"level": "warn", "recordId": rid,
                                                    "msg": f"预加载页面较慢：{str(e)[:80]}"})

                    publish_dt = _resolve_publish_datetime(job)
                    app = self.make_publisher(job, publish_dt, page, context, browser)
                    app.external_page = page
                    app.external_context = context
                    app.external_browser = browser

                    await app.upload(pw)

                    ok_count += 1
                    self.provider.update_publish_record(
                        rid, status=ST_SUCCESS,
                        message="预演完成" if job.dry_run else "发布成功",
                        finished=True)
                    self.bus.emit(batch_id, {"level": "success", "recordId": rid,
                                            "index": idx, "total": len(jobs),
                                            "status": "success",
                                            "msg": f"[{idx}/{len(jobs)}] 完成 {label}"})
                except Exception as e:  # noqa: BLE001
                    fail_count += 1
                    err = str(e)[:400] or e.__class__.__name__
                    failed_key.add(key)
                    self.provider.update_publish_record(
                        rid, status=ST_FAILED, message=err, finished=True)
                    self.bus.emit(batch_id, {"level": "error", "recordId": rid,
                                            "index": idx, "total": len(jobs),
                                            "status": "failed",
                                            "msg": f"[{idx}/{len(jobs)}] 失败 {label}：{err}"})
                    print(f"[分发] job failed: {label}\n{traceback.format_exc()}")

            # dry-run 预演：若宿主提供 reveal_window，把窗口拉回可视区人工核对
            if any(j.dry_run for j in jobs) and first_page and self.reveal_window:
                try:
                    await self.reveal_window(first_page)
                    self.bus.emit(batch_id, {"level": "info",
                                            "msg": "预演模式：浏览器已置顶，请人工核对后关闭窗口"})
                except Exception:  # noqa: BLE001
                    pass
        finally:
            if context:
                try:
                    await context.close()
                except Exception:  # noqa: BLE001
                    pass
            try:
                await browser.close()
            except Exception:  # noqa: BLE001
                pass

        return ok_count, fail_count

    def _batch_worker(self, batch_id, jobs, cancel_event) -> None:
        started = time.time()
        try:
            ok, fail = asyncio.run(self.run_jobs(batch_id, jobs, cancel_event))
        except Exception as e:  # noqa: BLE001
            ok, fail = 0, len(jobs)
            print(f"[分发] batch crashed: {traceback.format_exc()}")
            for job in jobs:
                self.provider.update_publish_record(
                    job.record_id, status=ST_FAILED,
                    message=f"批次异常：{e}", finished=True)
            self.bus.emit(batch_id, {"level": "error",
                                     "msg": f"批次异常终止：{e}"})
        finally:
            with self._running_lock:
                self._running.pop(batch_id, None)

        self.bus.emit(batch_id, {
            "level": "done",
            "status": "done",
            "msg": f"批次结束：成功 {ok} 个，失败 {fail} 个，用时 {int(time.time()-started)} 秒",
            "summary": {"ok": ok, "fail": fail, "total": len(jobs)},
        })
        # 通知所有 SSE/订阅者：流已结束，可以断开
        self.bus.close(batch_id)

    # ---- 本地定时调度 ----------------------------------------------------
    def start_scheduler(self) -> None:
        if self._scheduler_started:
            return
        self._scheduler_started = True
        threading.Thread(target=self._scheduler_loop, daemon=True).start()

    def _scheduler_tick(self) -> None:
        """执行一次定时轮询（可单独测试，不阻塞）。"""
        try:
            now = datetime.now().strftime("%Y-%m-%d %H:%M")
            due = self.provider.due_pending_records(now)
            if due:
                groups: Dict[str, List[Dict[str, Any]]] = {}
                for r in due:
                    groups.setdefault(r["batch_id"], []).append(r)
                for batch_id, rows in groups.items():
                    with self._running_lock:
                        if batch_id in self._running:
                            continue
                    jobs: List[PublishJob] = []
                    for r in rows:
                        payload = _load_payload(r.get("payload"))
                        if not payload:
                            self.provider.update_publish_record(
                                r["id"], status=ST_FAILED,
                                message="任务数据损坏", finished=True)
                            continue
                        job = PublishJob.from_dict(payload)
                        job.record_id = r["id"]
                        job.schedule_mode = "now"
                        job.scheduled_at = None
                        jobs.append(job)
                    if not jobs:
                        continue
                    cancel_event = threading.Event()
                    with self._running_lock:
                        self._running[batch_id] = {
                            "batchId": batch_id,
                            "total": len(jobs),
                            "startedAt": datetime.now().strftime("%Y-%m-%d %H:%M:%S"),
                            "cancel": cancel_event,
                        }
                    self.bus.emit(batch_id, {"level": "info",
                                             "msg": "定时时间已到，开始执行发布"})
                    threading.Thread(target=self._batch_worker,
                                     args=(batch_id, jobs, cancel_event),
                                     daemon=True).start()
        except Exception as e:  # noqa: BLE001
            print(f"[分发] scheduler error: {e}")

    def _scheduler_loop(self) -> None:
        while True:
            self._scheduler_tick()
            time.sleep(20)
