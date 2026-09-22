# 接入指南：把「一键分发 + 登录态保活」接到你的 Playwright 工具

参考实现是「乔发（QiaoFa）一键自媒体分发」：Flask 后端 + 原生 SPA 前端 +
Playwright，登录态存于 `cookiesFile/<uuid>.json`（Playwright `storage_state`），
账号/任务存 SQLite。下面给出可直接套用的接线片段。

- 第一部分：**一键分发**接线（`scripts/distribute.py`）
- 第二部分：**登录态保活**接线（`scripts/keepalive.py`）
- 第三部分：避坑清单

---

# 第一部分 · 一键分发

## 1. 宿主契约（provider 必须实现的 3 个方法）

| 方法 | 说明 |
|---|---|
| `create_publish_records(batch_id, items)` | 批量写入发布记录，返回与 items 等长的 id 列表。`items[i]` 见下 |
| `update_publish_record(record_id, status=None, message=None, finished=False)` | 更新单条状态；`finished=True` 时写 `finished_at` |
| `due_pending_records(now_str)` | 返回到点的本地定时记录，每项含 `id / batch_id / payload`（payload 可为 JSON 字符串或 dict） |

`items[i]` 字段：`accountId, accountName, platformType, filePath(素材), fileName,
title, tags, coverPath, scheduleMode, scheduledAt, status, payload`。

SQLite DDL 参考：

```sql
CREATE TABLE IF NOT EXISTS qf_publish_record (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  batch_id TEXT NOT NULL,
  account_id INTEGER, account_name TEXT, platform_type INTEGER,
  file_path TEXT, file_name TEXT, title TEXT, tags TEXT,
  cover_path TEXT, schedule_mode TEXT, scheduled_at TEXT,
  status TEXT, message TEXT,
  payload TEXT,                       -- job.to_dict() 的 JSON，定时到点后据此重建任务
  created_at TEXT, finished_at TEXT
);
CREATE INDEX idx_qf_pr_batch ON qf_publish_record(batch_id);
```

## 2. 五个必需回调

```python
async def launch_browser(pw):
    """启动浏览器。分发/保活都必须隐藏窗口。"""
    return await launch_chromium_with_codecs(pw, headless=True)   # 被风控时回退 hide_until_ready

async def open_context(browser, storage_state):
    """用合并后的登录态开上下文。storage_state 可能是 dict 或文件路径。"""
    ctx = await new_publish_context(browser, storage_state=storage_state)
    ctx = await set_init_script(ctx)      # 反自动化检测补丁
    return ctx

def merge_states(cookie_files):
    """合并多账号 storage_state：cookies 去重合并，origins 合并。
    视频号的登录态放最后，origin 冲突时以其为准。"""
    ...

def publish_url(platform_type):
    return {1: XHS_CREATOR, 2: CHANNELS_POST, 3: DOUYIN_CREATOR,
            4: KS_CREATOR, 5: BILI_UPLOAD}[platform_type]

def make_publisher(job, publish_datetime, page, context, browser):
    """按平台返回宿主的上传器对象（只需实现 async upload(playwright)）。"""
    cls = {1: XiaoHongShuVideo, 3: DouYinVideo, 4: KSVideo, 5: BilibiliVideo}[job.platform_type]
    return cls(job, publish_datetime, page=page, context=context, browser=browser)
```

## 3. 实例化与启动

```python
# publisher.py（或等价模块）
from scripts.distribute import DistributionEngine

engine = DistributionEngine(
    provider=db,
    launch_browser=launch_browser,
    open_context=open_context,
    merge_states=merge_states,
    publish_url=publish_url,
    make_publisher=make_publisher,
    reveal_window=reveal_window,        # 可选：dry-run 时显示窗口
    cfg_get=db.cfg_get, cfg_set=db.cfg_set,
    # playwright_factory=async_playwright,   # 可选：接管驱动启动方式（测试可注入 fake）
)

# server.py 启动段
db.init_db()
engine.start_scheduler()      # 开启本地定时轮询（每 20s 检查一次到点任务）
```

## 4. 后端接口

```python
@app.route("/api/publish/submit", methods=["POST"])
def api_publish_submit():
    plan = request.get_json(silent=True) or {}
    try:
        res = engine.submit(plan)
    except ValueError as e:
        return err(str(e))
    return ok(res, "已加入定时队列" if res.get("queued") else "发布任务已启动")

@app.route("/api/publish/cancel", methods=["POST"])
def api_publish_cancel():
    batch_id = (request.get_json(silent=True) or {}).get("batchId", "")
    return ok({"cancelled": engine.cancel(batch_id)})

@app.route("/api/publish/stream/<batch_id>")
def api_publish_stream(batch_id):
    """SSE 实时进度。

    subscribe() 会自动补发历史事件，之后增量推送；批次结束时引擎调用
    bus.close() 放入哨兵 None，消费循环据此干净退出。
    """
    def gen():
        q = engine.bus.subscribe(batch_id)
        try:
            while True:
                e = q.get()                 # 阻塞直到有事件或哨兵
                if e is None:               # 结束哨兵
                    break
                yield f"data: {json.dumps(e, ensure_ascii=False)}\n\n"
        finally:
            engine.bus.unsubscribe(batch_id, q)
    return Response(gen(), mimetype="text/event-stream",
                    headers={"Cache-Control": "no-cache",
                             "X-Accel-Buffering": "no"})
```

> 提示：`bus.history(batch_id)` 可在订阅前单独拉取，用于「刷新页面后补显示日志」。
> 事件结构：`{"level": "info|warn|error|done", "msg": "...", "ts": "HH:MM:SS"}`，
> `done` 事件额外带 `summary: {ok, fail, total}`。

## 5. 前端

```js
// 提交
QF.api.post('/api/publish/submit', plan).then(function (d) {
  var r = d.data || d;
  listenProgress(r.batchId);
});

// SSE 进度
function listenProgress(batchId) {
  var es = new EventSource('/api/publish/stream/' + batchId);
  es.onmessage = function (ev) {
    var e = JSON.parse(ev.data);
    appendLog(e);                         // info/warn/error 分级着色
    if (e.level === 'done') { es.close(); loadRecords(); }
  };
  es.onerror = function () { es.close(); };
}

// 取消
QF.api.post('/api/publish/cancel', { batchId: batchId });
```

## 6. 内置行为一览

| 行为 | 说明 |
|---|---|
| 单浏览器多标签 | 一次 `launch_browser`，合并所有账号登录态共用一个 context，每个 job 开一个 tab |
| 视频号优先 | 合并 cookie 时视频号放最后，避免 origin 冲突（与乔发一致） |
| 同账号失败跳过 | 某账号的第一条失败后，该账号剩余 job 直接标记失败，不继续浪费时间 |
| 取消 | `cancel()` 设事件，引擎在**当前 job 结束后**停止，剩余标记「已取消」 |
| 平台侧定时 | `mode=platform` + `time` → 解析成 `datetime` 交给上传器，由平台定时发布 |
| 本地定时 | `mode=local` + `time` → 入队返回 `queued=True`，由调度器到点重建 job 再发 |
| dry-run | 走完流程但不真提交，并调用 `reveal_window` 把隐藏窗口显示出来供人工核对 |
| 进度事件 | `EventBus` 发 `info/warn/error/done`，`history()` 可回放，`subscribe()` 订阅增量 |

---

# 第二部分 · 登录态保活

## 1. 实例化（服务启动时）

```python
from scripts.keepalive import SessionKeepAlive

ka = SessionKeepAlive(
    provider=db,                      # 提供 list_accounts() -> [{id,name,type,storage_state_path}]
    open_context=open_ctx,            # async (browser, path) -> (context, page)
    content_url_fn=content_page_url,  # (ptype) -> 后台内容页 URL
    default_interval_hours=12,
    cfg_get=db.cfg_get, cfg_set=db.cfg_set,
)
```

启动段：

```python
db.init_db()
ka.start()        # 后台定时保活（按 keepalive_hours 配置，0 = 关闭）
```

## 2. 同步即续期（最高性价比）

在「同步 / 抓取」成功后、关 context 之前，把刷新后的登录态写回 cookie 文件：

```python
from scripts.keepalive import refresh_cookie
await refresh_cookie(context, str(cookie_path))
```

这是「滑动会话续期」的核心：访问后台时服务端已顺延 cookie，重写文件即落盘。

## 3. 手动刷新接口

```python
@app.route("/api/accounts/<int:account_id>/refresh", methods=["POST"])
def api_account_refresh(account_id):
    acc = db.get_account(account_id)
    if not acc:
        return err("账号不存在", 404)
    if not acc.get("filePath"):
        return err("该账号没有登录文件")
    try:
        res = run_async(ka.keep_alive(acc))     # asyncio.run 包一层
    except Exception as e:
        return err(f"刷新失败：{str(e)[:200]}")
    db.update_account(account_id, status=1 if res.get("valid") else 0)
    return ok(res, res.get("msg"))
```

## 4. 自动保活接口（开关 + 频率）

```python
@app.route("/api/keepalive/status")
def api_keepalive_status():
    h = float(db.cfg_get("keepalive_hours", 12) or 0)
    return ok({"enabled": h > 0, "intervalHours": h,
               "lastPass": db.cfg_get("keepalive_last_pass"),
               "runs": db.cfg_get("keepalive_runs", {})})

@app.route("/api/keepalive/set", methods=["POST"])
def api_keepalive_set():
    body = request.get_json(silent=True) or {}
    try:
        hours = int(body.get("hours", 12))
    except (TypeError, ValueError):
        return err("参数错误")
    hours = 0 if hours < 0 else (1 if 0 < hours < 1 else hours)
    db.cfg_set("keepalive_hours", hours)
    ka.stop()
    if hours > 0:
        ka.start()
    return ok({"intervalHours": hours, "enabled": hours > 0},
              "自动保活已" + ("开启" if hours > 0 else "关闭"))
```

## 5. 前端：账号卡「刷新登录态」按钮

```js
// accounts.js 操作按钮区，紧跟「重新登录」之后
+ '<button class="btn xs" data-act="refresh">刷新登录态</button>'

// onAction 分支
if (act === 'refresh') {
  btn.disabled = true; btn.textContent = '刷新中';
  QF.api.post('/api/accounts/' + acc.id + '/refresh', {})
    .then(function (d) {
      var r = d.data || d;
      r.valid ? QF.ok(acc.displayName + '：' + (r.msg || '已刷新'))
              : QF.warn(acc.displayName + '：' + (r.msg || '登录失效'));
      load(root);
    })
    .catch(function (e) { btn.disabled = false; btn.textContent = '刷新登录态'; QF.err(e.message); });
  return;
}
```

## 6. 前端：设置页「登录态自动保活」卡片

```js
QF.api.get('/api/keepalive/status').catch(function () {
  return { enabled: false, intervalHours: 12, runs: {} };
});

function keepaliveForm(ka) {
  ka = ka || {};
  var on = ka.enabled, hours = Number(ka.intervalHours) || 12;
  var opts = [6, 12, 24].map(function (h) {
    return '<option value="' + h + '"' + (h === hours ? ' selected' : '') + '>每 ' + h + ' 小时</option>';
  }).join('');
  var last = ka.lastPass && ka.lastPass.ts ? ('上次保活轮次：' + ka.lastPass.ts) : '';
  return '<label style="display:inline-flex;gap:6px;align-items:center">'
    + '<input type="checkbox" id="kaOn" ' + (on ? 'checked' : '') + '> 启用自动保活</label>'
    + '<div class="field"><label>保活频率</label><select class="inp" id="kaHours">' + opts + '</select></div>'
    + (last ? '<div class="small muted">' + last + '</div>' : '')
    + '<button class="btn primary" id="kaSave">保存</button><span id="kaMsg"></span>';
}

function bindKeepalive(root) {
  QF.$('#kaSave', root).onclick = function () {
    var h = QF.$('#kaOn', root).checked ? Number(QF.$('#kaHours', root).value) : 0;
    QF.api.post('/api/keepalive/set', { hours: h })
      .then(function () { QF.ok('保活设置已保存'); })
      .catch(function (e) { QF.$('#kaMsg', root).textContent = e.message; });
  };
}
```

---

# 第三部分 · 避坑清单

- `asyncio.run(...)` 只能在**无正在运行的事件循环**时调用（同步 Flask 视图内 OK；
  异步视图需改用 `loop.run_until_complete`）。
- 引擎的每个批次都在独立守护线程里 `asyncio.run`，互不干扰；但**同批次内部是
  顺序执行**的（共用一个 context），不要试图并行发布同一账号。
- `payload` 建议存 `job.to_dict()` 的 JSON 字符串；引擎的 `_load_payload`
  同时兼容 dict，但存字符串更利于跨进程/重启恢复。
- `refresh_cookie` 失败多为文件被占用或路径无权限，仅打印日志，不影响主流程。
- 保活结果写入 `keepalive_runs`（按 account_id），可在设置页展示，方便核对
  「某账号是否仍在线」。
- 窗口隐藏是硬要求：分发/保活都走 `headless=True`；被风控时回退
  `hide_until_ready`（丢到屏幕外 `-32000,-32000`）。只有 dry-run 才显式显示窗口。
- 单元测试注入 `playwright_factory`（返回异步上下文管理器）即可完全脱离浏览器：
  `async with factory() as pw` 拿到 fake 对象，配合 fake 的
  `launch_browser / open_context / make_publisher` 就能跑通整条编排链路。
