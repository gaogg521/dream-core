"""无浏览器单元测试：验证分发引擎的编排逻辑（任务展开 / 时间解析 / 标签归一 /
事件总线 / 按账号失败跳过 / 取消 / dry-run 揭示 / 本地定时重建）。

用 fake host 注入 6 个回调，不依赖 playwright、不依赖具体项目。
"""

import asyncio
import json
import os
import sys
import threading
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from scripts.distribute import (
    DistributionEngine, EventBus, PublishJob, build_jobs,
    normalize_publish_tags, _resolve_publish_datetime,
    ST_PENDING, ST_RUNNING, ST_SUCCESS, ST_FAILED, ST_CANCELLED,
)


class FakePlaywright:
    """占位对象：仅用于让 run_jobs 走「注入 playwright」分支，无需真实浏览器。"""
    pass


class FakePlaywrightCM:
    """异步上下文管理器：模拟 async_playwright()，使定时/后台线程路径也可测。"""
    async def __aenter__(self):
        return FakePlaywright()

    async def __aexit__(self, *a):
        return False


# ---------------------------------------------------------------- fake host
class FakeProvider:
    def __init__(self):
        self.accounts = [
            {"id": 1, "type": 3, "filePath": "douyin_a.json", "userName": "抖A"},
            {"id": 2, "type": 4, "filePath": "ks_b.json", "userName": "快B"},
        ]
        self.records = {}
        self.next_id = 1
        self.due_rows = []

    def list_accounts(self, platform_type=None):
        return self.accounts

    def create_publish_records(self, batch_id, items):
        ids = []
        for it in items:
            rid = self.next_id
            self.next_id += 1
            self.records[rid] = dict(it, id=rid, status=ST_PENDING, message=None,
                                     started_at=None, finished_at=None)
            ids.append(rid)
        return ids

    def update_publish_record(self, record_id, status=None, message=None,
                              started=False, finished=False):
        r = self.records[record_id]
        if status is not None:
            r["status"] = status
        if message is not None:
            r["message"] = str(message)[:1000]
        if started:
            r["started_at"] = "now"
        if finished:
            r["finished_at"] = "now"

    def due_pending_records(self, now_str):
        return self.due_rows

    def cfg_get(self, k, d=None):
        return d

    def cfg_set(self, k, v):
        pass


class FakeApp:
    """模拟平台上传器：upload 可成功、可抛错（按 job 控制）。"""
    def __init__(self, job, publish_datetime, page, context, browser,
                 force_fail=False):
        self.job = job
        self.publish_datetime = publish_datetime
        self.external_page = page
        self.external_context = context
        self.external_browser = browser
        self.uploaded = False
        self.force_fail = force_fail

    async def upload(self, pw):
        # 模拟上传：指定账号会失败
        if self.force_fail:
            raise RuntimeError("模拟上传失败")
        self.uploaded = True


class FakePage:
    async def goto(self, *a, **k):
        pass
    async def wait_for_timeout(self, *a, **k):
        pass
    async def close(self):
        pass


class FakeContext:
    def __init__(self):
        self.pages = []
    async def new_page(self):
        p = FakePage()
        self.pages.append(p)
        return p
    async def close(self):
        pass


class FakeBrowser:
    async def close(self):
        pass


def make_host(plan=None, fail_account_id=None, reveal_calls=None):
    provider = FakeProvider()
    revealed = reveal_calls if reveal_calls is not None else []
    fail_id = fail_account_id

    async def launch_browser(pw):
        return FakeBrowser()

    async def open_context(browser, storage_state):
        return FakeContext()

    def merge_states(cookie_files):
        return {"cookies": [], "origins": []}

    def publish_url(ptype):
        return {1: "xhs", 2: "tencent", 3: "douyin", 4: "ks", 5: "bili"}.get(ptype)

    def make_publisher(job, publish_datetime, page, context, browser):
        force = fail_id is not None and job.account_id == fail_id
        return FakeApp(job, publish_datetime, page, context, browser,
                       force_fail=force)

    def reveal_window(page):
        revealed.append(page)

    return provider, dict(
        launch_browser=launch_browser,
        open_context=open_context,
        merge_states=merge_states,
        publish_url=publish_url,
        make_publisher=make_publisher,
        reveal_window=reveal_window,
        playwright_factory=FakePlaywrightCM,
    )


# ---------------------------------------------------------------- 断言辅助
_fail = 0
def check(cond, msg):
    global _fail
    if cond:
        print("  PASS", msg)
    else:
        _fail += 1
        print("  FAIL", msg)


# ---------------------------------------------------------------- 1) build_jobs
def test_build_jobs():
    print("[1] build_jobs 展开 + 内容合并 + 标签归一")
    plan = {
        "materials": [{"filePath": "v1.mp4", "fileName": "v1"},
                      {"filePath": "v2.mp4", "fileName": "v2"}],
        "targets": [{"accountId": 1, "platformType": 3, "accountName": "抖A",
                     "filePath": "douyin_a.json"}],
        "content": {"default": {"title": "默认标题", "tags": ["#通用", "重复"]},
                    "byPlatform": {"3": {"title": "抖音标题", "tags": ["#抖"]}}},
        "schedule": {"mode": "now"},
        "covers": {"default": "c.jpg"},
        "options": {"dryRun": False},
    }
    jobs = build_jobs(plan)
    check(len(jobs) == 2, "2 素材 × 1 账号 = 2 个 job")
    j0 = jobs[0]
    check(j0.title == "抖音标题", "平台专属标题覆盖默认")
    check(j0.tags == ["#抖"], "平台专属 tags 覆盖默认（非合并）: " + str(j0.tags))
    check(j0.cookie_file == "douyin_a.json", "cookie 文件透传")
    check(j0.cover == "c.jpg", "封面透传")


# ---------------------------------------------------------------- 2) 时间解析
def test_datetime():
    print("[2] _resolve_publish_datetime")
    j = PublishJob(3, scheduled_at="2026-09-05 10:00", schedule_mode="platform")
    dt = _resolve_publish_datetime(j)
    check(isinstance(dt, type(dt)) and dt.year == 2026, "平台侧定时解析为 datetime")
    j2 = PublishJob(3, schedule_mode="now")
    check(_resolve_publish_datetime(j2) == 0, "立即发布返回 int 0（非 None）")
    j3 = PublishJob(3, scheduled_at="坏时间", schedule_mode="platform")
    check(_resolve_publish_datetime(j3) == 0, "无法解析时降级为 0")


# ---------------------------------------------------------------- 3) 标签归一
def test_tags():
    print("[3] normalize_publish_tags")
    check(normalize_publish_tags("a, b c") == ["#a", "#b", "#c"], "字符串拆分+补#")
    check(normalize_publish_tags(["x", "y"], max_count=1) == ["#x"], "max_count 截断")
    check(normalize_publish_tags(None) == [], "None -> []")


# ---------------------------------------------------------------- 4) 事件总线
def test_bus():
    print("[4] EventBus")
    bus = EventBus()
    q = bus.subscribe("B1")
    bus.emit("B1", {"level": "info", "msg": "hi"})
    ev = q.get(timeout=1)
    check(ev["msg"] == "hi" and "ts" in ev, "订阅者收到事件且带 ts")
    check(len(bus.history("B1")) == 1, "历史记录 1 条")

    # 结束哨兵：close 后订阅者收到 None，可干净退出；历史保留
    bus.close("B1")
    check(q.get(timeout=1) is None, "close 后收到结束哨兵 None")
    check(len(bus.history("B1")) == 1, "close 后历史仍保留")


# ---------------------------------------------------------------- 5) 成功编排
def test_run_success():
    print("[5] run_jobs 全部成功")
    provider, cb = make_host()
    engine = DistributionEngine(provider, **cb)
    plan = {
        "materials": [{"filePath": "v1.mp4", "fileName": "v1"}],
        "targets": [{"accountId": 1, "platformType": 3, "accountName": "抖A", "filePath": "a.json"},
                    {"accountId": 2, "platformType": 4, "accountName": "快B", "filePath": "b.json"}],
        "content": {"default": {"title": "t", "tags": []}},
        "schedule": {"mode": "now"},
    }
    jobs = build_jobs(plan)
    rid_map = provider.create_publish_records("B1", [
        {"accountId": j.account_id, "accountName": j.account_name,
         "platformType": j.platform_type, "filePath": j.file_path,
         "fileName": j.file_name, "title": j.title, "tags": j.tags,
         "coverPath": j.cover, "scheduleMode": "now", "scheduledAt": None,
         "status": ST_PENDING, "payload": j.to_dict()} for j in jobs])
    for j, rid in zip(jobs, rid_map):
        j.record_id = rid
    ok, fail = asyncio.run(engine.run_jobs("B1", jobs, threading.Event(), playwright=FakePlaywright()))
    check(ok == 2 and fail == 0, f"成功 2 失败 0 (ok={ok}, fail={fail})")
    check(all(provider.records[r]["status"] == ST_SUCCESS for r in rid_map),
          "两条记录状态均为成功")


# ---------------------------------------------------------------- 6) 按账号失败跳过
def test_fail_skip():
    print("[6] 同账号前序失败 -> 跳过后续")
    provider, cb = make_host(fail_account_id=1)
    engine = DistributionEngine(provider, **cb)
    plan = {
        "materials": [{"filePath": "v1.mp4", "fileName": "v1"},
                      {"filePath": "v2.mp4", "fileName": "v2"}],
        "targets": [{"accountId": 1, "platformType": 3, "accountName": "抖A", "filePath": "a.json"}],
        "content": {"default": {"title": "t", "tags": []}},
        "schedule": {"mode": "now"},
    }
    jobs = build_jobs(plan)
    rid_map = provider.create_publish_records("B1", [
        {"accountId": j.account_id, "accountName": j.account_name,
         "platformType": j.platform_type, "filePath": j.file_path,
         "fileName": j.file_name, "title": j.title, "tags": j.tags,
         "coverPath": j.cover, "scheduleMode": "now", "scheduledAt": None,
         "status": ST_PENDING, "payload": j.to_dict()} for j in jobs])
    for j, rid in zip(jobs, rid_map):
        j.record_id = rid
    ok, fail = asyncio.run(engine.run_jobs("B1", jobs, threading.Event(), playwright=FakePlaywright()))
    # 第一个失败 -> 第二个同账号被跳过（不计为 ok，但 fail 计 1，跳过不计 fail）
    check(ok == 0, f"ok=0 (实际 {ok})")
    check(provider.records[rid_map[0]]["status"] == ST_FAILED, "第1条=失败")
    check(provider.records[rid_map[1]]["status"] == ST_FAILED, "第2条=失败(跳过)")


# ---------------------------------------------------------------- 7) 取消
def test_cancel():
    print("[7] 取消：当前任务后停止")
    provider, cb = make_host()
    engine = DistributionEngine(provider, **cb)
    plan = {
        "materials": [{"filePath": "v1.mp4", "fileName": "v1"}],
        "targets": [{"accountId": 1, "platformType": 3, "accountName": "抖A", "filePath": "a.json"},
                    {"accountId": 2, "platformType": 4, "accountName": "快B", "filePath": "b.json"},
                    {"accountId": 3, "platformType": 5, "accountName": "B站C", "filePath": "c.json"}],
        "content": {"default": {"title": "t", "tags": []}},
        "schedule": {"mode": "now"},
    }
    jobs = build_jobs(plan)
    rid_map = provider.create_publish_records("B1", [
        {"accountId": j.account_id, "accountName": j.account_name,
         "platformType": j.platform_type, "filePath": j.file_path,
         "fileName": j.file_name, "title": j.title, "tags": j.tags,
         "coverPath": j.cover, "scheduleMode": "now", "scheduledAt": None,
         "status": ST_PENDING, "payload": j.to_dict()} for j in jobs])
    for j, rid in zip(jobs, rid_map):
        j.record_id = rid

    cancel = threading.Event()
    # 在第一个 job 完成后立即取消
    orig_make = cb["make_publisher"]
    state = {"n": 0}
    def _make(job, pdt, page, ctx, br):
        state["n"] += 1
        if state["n"] == 1:
            cancel.set()  # 第1个完成后触发取消
        return orig_make(job, pdt, page, ctx, br)
    engine.make_publisher = _make

    ok, fail = asyncio.run(engine.run_jobs("B1", jobs, cancel, playwright=FakePlaywright()))
    statuses = [provider.records[r]["status"] for r in rid_map]
    check(statuses[0] == ST_SUCCESS, "第1条成功")
    check(statuses[1] == ST_CANCELLED, "第2条取消")
    check(statuses[2] == ST_CANCELLED, "第3条取消")


# ---------------------------------------------------------------- 8) dry-run 揭示
def test_dryrun_reveal():
    print("[8] dry-run 触发 reveal_window")
    provider, cb = make_host()
    revealed = []
    engine = DistributionEngine(provider, reveal_window=cb["reveal_window"], **{
        k: v for k, v in cb.items() if k != "reveal_window"})
    # 重定向 reveal 记录
    engine.reveal_window = lambda page: revealed.append(page)
    plan = {
        "materials": [{"filePath": "v1.mp4", "fileName": "v1"}],
        "targets": [{"accountId": 1, "platformType": 3, "accountName": "抖A", "filePath": "a.json"}],
        "content": {"default": {"title": "t", "tags": []}},
        "schedule": {"mode": "now"},
        "options": {"dryRun": True},
    }
    jobs = build_jobs(plan)
    rid_map = provider.create_publish_records("B1", [
        {"accountId": j.account_id, "accountName": j.account_name,
         "platformType": j.platform_type, "filePath": j.file_path,
         "fileName": j.file_name, "title": j.title, "tags": j.tags,
         "coverPath": j.cover, "scheduleMode": "now", "scheduledAt": None,
         "status": ST_PENDING, "payload": j.to_dict()} for j in jobs])
    for j, rid in zip(jobs, rid_map):
        j.record_id = rid
    asyncio.run(engine.run_jobs("B1", jobs, threading.Event(), playwright=FakePlaywright()))
    check(len(revealed) == 1, "dry-run 后 reveal_window 被调用 1 次")


# ---------------------------------------------------------------- 9) submit 排队
def test_submit_local():
    print("[9] submit 本地定时 -> 入队返回 queued")
    provider, cb = make_host()
    engine = DistributionEngine(provider, **cb)
    plan = {
        "materials": [{"filePath": "v1.mp4", "fileName": "v1"}],
        "targets": [{"accountId": 1, "platformType": 3, "accountName": "抖A", "filePath": "a.json"}],
        "content": {"default": {"title": "t", "tags": []}},
        "schedule": {"mode": "local", "time": "2026-09-10 09:00"},
    }
    res = engine.submit(plan)
    check(res["queued"] is True, "local 模式返回 queued=True")
    check(len(res["recordIds"]) == 1, "生成 1 条发布记录")


# ---------------------------------------------------------------- 10) 定时重建
def test_scheduler_rebuild():
    print("[10] 本地定时到点重建 job 并发布")
    provider, cb = make_host()
    engine = DistributionEngine(provider, **cb)
    # 预置一条本地定时记录（payload 是已完成的 job）
    job = PublishJob(3, account_id=1, account_name="抖A", cookie_file="a.json",
                     file_path="v1.mp4", file_name="v1", title="t",
                     schedule_mode="local", scheduled_at="2026-09-10 09:00")
    rid = provider.create_publish_records("BZ", [{
        "accountId": 1, "accountName": "抖A", "platformType": 3,
        "filePath": "v1.mp4", "fileName": "v1", "title": "t", "tags": [],
        "coverPath": None, "scheduleMode": "local", "scheduledAt": "2026-09-10 09:00",
        "status": ST_PENDING, "payload": job.to_dict()}])[0]
    # 模拟“到点”（真实宿主以 JSON 字符串存 payload）
    provider.due_rows = [{"batch_id": "BZ", "id": rid,
                          "payload": json.dumps(job.to_dict())}]
    # 只执行一次轮询（_scheduler_tick 可单独测试，不会死循环）
    engine._scheduler_tick()
    # 等待后台批量线程完成
    for _ in range(50):
        if provider.records[rid]["status"] in (ST_SUCCESS, ST_FAILED):
            break
        time.sleep(0.05)
    check(provider.records[rid]["status"] == ST_SUCCESS, "到点后记录变为成功")


if __name__ == "__main__":
    try:
        test_build_jobs()
        test_datetime()
        test_tags()
        test_bus()
        test_run_success()
        test_fail_skip()
        test_cancel()
        test_dryrun_reveal()
        test_submit_local()
        test_scheduler_rebuild()
    except Exception:
        _fail += 1
        traceback.print_exc()
    print("\n==== 失败项:", _fail, "====")
    sys.exit(1 if _fail else 0)
