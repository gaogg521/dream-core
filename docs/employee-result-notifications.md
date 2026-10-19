# Digital employee result notifications

Members configure their own recipient under **Schedule & notify** on an employee card. Notifications apply to manual and scheduled runs independently of whether a schedule is enabled. Interactive employee chats do not send duplicate bot notifications.

- `GET/PUT /api/one/employee/agents/{agent_id}/notification` reads/writes `{ "target": { "plugin_id": "…", "chat_id": "…" } }`; `target: null` disables delivery.
- Employee access is checked before reading or changing settings. Settings belong to the requesting user, including for a shared company employee.
- Recipients must belong to that user's existing enabled, connected bot sessions. WeChat, WeCom, Lark, DingTalk and Telegram support result files. Other channel types are excluded from employee notification selection.
- Successful runs send a short summary and `result.md` containing the final answer. Up to five additional files explicitly linked in the final answer are eligible: each must be inside the conversation workspace, nonempty and at most 20 MiB; their combined size is limited to 40 MiB. Paths outside the workspace and remote links are excluded.
- Failed runs send the recorded reason without attachments. Delivery failure does not rewrite the employee task outcome.
- `one_employee_deliveries` records `sending`, `sent` or `failed` per run. The unique run ID prevents duplicate sends. Ambiguous failures are not automatically retried, because the platform may already have delivered some messages or files.
- Delivery is bounded to 120 seconds. Production logs include the run ID and success/failure boundary, without task contents, attachment contents or credentials.
- WeChat replies reuse the context token received from that chat. The current channel stores these tokens in memory, so a backend restart may require a new incoming message before the platform accepts another delivery. A connected indicator alone does not prove outgoing delivery; nonzero platform return codes mark delivery as failed.

The employee domain depends on a notifier trait. The application composition adapter binds it to the existing channel manager; credentials and recipient discovery use the same infrastructure as the user's connected bots. SQLite and MySQL use append-only migration 011.

Protocol references: [WeCom official SDK](https://github.com/WecomTeam/aibot-node-sdk/blob/main/src/client.ts), [WeChat official connector](https://github.com/Tencent/openclaw-weixin), [Telegram Bot API](https://core.telegram.org/bots/api#senddocument), [Lark upload API](https://open.feishu.cn/document/server-docs/im-v1/file/create), [DingTalk upload API](https://developer.alibaba.com/docs/api.htm?apiId=37051).

Automated checks cover durable receipts, owner isolation, recipient rejection, success reports, failure reasons, workspace file boundaries, uploaded bytes and platform acknowledgement errors. Actual platform delivery must be reported separately from fixture-based protocol checks.
