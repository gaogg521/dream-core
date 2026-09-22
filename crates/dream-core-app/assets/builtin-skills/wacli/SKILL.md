---
name: wacli
display_name: "WhatsApp"
description: Send WhatsApp messages to other people or search/sync WhatsApp history via the wacli CLI (。触发：用户提出「wacli」相关需求时使用（常见说法：wacli、wacli；英文：wacli）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# wacli

Use `wacli` only when the user explicitly asks you to message someone on WhatsApp or to sync/search WhatsApp history.

Safety
- Require explicit recipient + message text.
- Confirm recipient + message before sending.

Auth + sync
- `wacli auth` (QR login + initial sync)
- `wacli sync --follow` (continuous sync)
- `wacli doctor`

Find chats + messages
- `wacli chats list --limit 20 --query "name or number"`
- `wacli messages search "query" --limit 20 --chat <jid>`

Send
- Text: `wacli send text --to "+14155551212" --message "Hello!"`
- File: `wacli send file --to "+14155551212" --file /path/agenda.pdf --caption "Agenda"`

Notes
- Store dir: `~/.wacli` (override with `--store`).
- Use `--json` for machine-readable output.
