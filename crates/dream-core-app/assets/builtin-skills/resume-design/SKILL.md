---
name: resume-design
display_name: "简历设计"
description: 美图设计室出品的“简历设计”专家 Skill，通过终端执行命令名为 `designkit` 的「美图设计室 AI设计 CLI」完成“简历设计”（办公设计）：简历版式设计，模板复刻。。触发：用户提出「简历设计」相关需求时使用（常见说法：简历设计、简历设计；英文：resume/design）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# 简历设计

使用「美图设计室 AI设计 CLI」（终端命令名为 `designkit`）按“办公设计 / 简历设计”专家模板完成任务。本平台 Connector 与用户本地安装都使用同一个 npm 包和登录状态。默认使用简体中文；用户明确指定其他语言时跟随用户。

## CLI 入口解析

「美图设计室 AI设计 CLI」是当前产品/连接器名称；真正执行的终端命令名是 `designkit`，不存在名为 `designkit-buddy-cli` 的业务命令。首次执行前必须自动解析一次 CLI 入口，不要要求用户手动提供路径。

按当前平台查找 npm 暴露的命令：

1. macOS/Linux 执行 `command -v designkit`，Windows PowerShell 执行 `Get-Command designkit -ErrorAction SilentlyContinue`。
2. 对找到的绝对入口执行一次 `--version` 以确认入口可用，不解析或比较版本号。命令成功时记为 `<designkit>`；后文的 `<designkit>` 代表该绝对命令路径，不能执行字面占位符。
3. 不查找 Connector 私有目录或依赖生命周期临时注入的环境变量。Connector 和 Agent 统一使用 npm 命令及默认的 `~/.designkit` 登录状态。

在 本平台 中找不到 `designkit` 或 `--version` 无法正常执行时，不要停止任务。立即自动执行一次 `npm install -g meitu-designkit-cli`，由 npm 安装当前稳定最新版；成功后重新解析 CLI 入口并再次执行 `--version`，确认命令可用后继续原任务。安装命令只执行一次，不得添加 `sudo`、不得修改 npm registry，也不得安装带固定版本号的包。

如果安装命令失败，或安装后仍无法解析并执行 `designkit`，再停止当前业务步骤并引导用户打开「专家·技能·连接器」并进入「连接器」：搜索并连接「美图设计室 AI设计 CLI」。保留简短的原始安装错误摘要便于定位，不得循环安装，也不得改用内部 API。

## 能力边界

- 只执行“简历设计”对应的专家流程，不把本 Skill 当作任意美图设计室对话入口。
- 必须使用用户提供的真实素材和要求；不得用示例图、网络图、临时生成图或默认商品代替缺失素材。
- 不自行拼接内部 API、鉴权字段或专家 Skill 接口；所有业务执行都通过已解析并通过可用性检查的 `<designkit>` 入口。
- 创建任务前检查全部必填字段：只能复用用户已经明确提供的信息，缺失时必须停止创建并向用户追问。可选选择型字段未指定且存在 `auto` 选项时使用其可读 `label`“自动匹配”；其他未指定的可选字段直接省略，不自行推断、不主动追问。
- 选择型字段只能使用 `references/form.json` 中列出的选项。对话与提交给 CLI 的 Prompt 均使用可读的 `label`；`key` 只用于定位和校验选项，不得写入 Prompt。

## 标准输入引导

- 允许用户先用自然语言描述需求，不要求用户预先记住字段名或 Prompt 模板。先读取 [references/form.json](references/form.json)，把用户已经提供的信息映射到对应字段，信息已足够时不要重复追问。
- 缺少必填字段时，只询问当前缺失项，并提供一份可复制的“字段标签：填写内容”模板；文件字段写成“请上传：字段标签”，选择型字段只展示可读的 `label` 选项，不向用户暴露内部 `key`。
- 可选选择型字段未指定时，若 `form.json` 的合法选项中存在 `key=auto`，就在 Prompt 中写入该选项的可读 `label`“自动匹配”；不存在 `auto` 的选择型字段以及未指定的文本、数字、文件等其他可选字段直接省略。用户使用某种语言对话、附件呈现出的视觉特征、文件名或任务类型都不能视为用户指定了语言、风格、篇幅、比例等可选字段。只有用户主动询问“可以配置什么”“有哪些选项”等配置能力时，才按“输入契约”列出必填项、可选项及其可读选项。
- 用户补齐必填项后，只把本次实际提交的字段按“输入契约”顺序整理成简短确认清单。用户未提出异议即可继续，不要求用户再次抄写模板；不得虚构缺失的素材、文案或选择。
- 最终提交给 CLI 的内容必须严格按“Prompt 渲染”规则生成。字段 `key` 只用于匹配模板占位符；选择型字段把合法选项的可读 `label` 写入 Prompt，不得写入内部选项 `key`。

## 输入契约

执行前读取 [references/form.json](references/form.json)，它是字段、默认值、选项和 Prompt 模板的机器可读事实源。

| key | 用户标签 | 类型 | 必填 | 接口默认值（不自动使用） | 约束与选项 |
|---|---|---|---|---|---|
| `content` | 简历内容 | `textarea` | 是 | - | 最大长度：5000<br>占位说明：粘贴或输入教育经历、工作经历、项目经历等简历内容 |
| `ref` | 简历材料（可选） | `file_upload` | 否 | - | 文件类型：word, pdf, image<br>最大数量：3<br>提示：支持上传现有简历、个人照片等材料 |
| `job_target` | 目标岗位 | `text` | 否 | - | 占位说明：如：产品经理、UI设计师 |
| `language` | 简历语言 | `select` | 否 | auto | 选项：自动匹配 (`auto`)；中文 (`chinese`)；英文 (`english`)；日语 (`japanese`)；韩语 (`korean`)；葡萄牙语（巴西） (`portuguese_brazil`)；西班牙语（墨西哥） (`spanish_mexico`)；俄语 (`russian`) |
| `style` | 简历风格 | `radio_tags` | 否 | auto | 选项：自动匹配 (`auto`)；简洁正式 (`clean_formal`)；吸睛专业 (`eye_catching_professional`)；创意突出 (`creative_standout`) |

## Prompt 渲染

主 Prompt 模板如下，仅以其中的固定文本和字段顺序为基准；必填项按规则补齐，可选项按下述 `auto` 与省略规则处理：

```text
[简历设计]设计简历。
简历内容：{content}
简历材料：{ref}
目标岗位：{job_target}
简历语言：{language}
简历风格：{style}
```

按以下规则渲染：

1. `form.fields[].key` 与模板中的 `{key}` 一一对应。必填字段必须使用用户明确提供的值完成替换；仍然缺失时停止提交并追问，不能猜测或残留未知 `{...}`。
2. 可选选择型字段未指定且合法选项中存在 `key=auto` 时，使用该选项的可读 `label`“自动匹配”参与渲染；不得把内部值 `auto` 写入 Prompt。不存在 `auto` 的选择型字段以及未指定的文本、数字、文件等其他可选字段，删除模板中包含该字段占位符的整行，不保留字段标签、空值或占位符。
3. `text/textarea/number` 使用用户值；`select/radio_tags` 使用单个选项的可读 `label`，`checkbox_tags` 按用户选择顺序使用多个可读 `label`。内部 `key` 只用于校验用户选择，不能出现在 Prompt 中。
4. `file_upload` 不把本地路径写入 Prompt。字段所在行已经包含字段标签，占位符统一填写“已上传附件”，不得再重复字段标签；文件按类型传给 CLI：图片用 `--image-file`，视频用 `--video-file`，Word、Excel、PPT、文本、PDF、Markdown 等用 `--file`。
5. 多个附件保持用户给出的顺序；若存在多个文件字段，在 Prompt 中明确每组附件对应的字段标签。
6. 渲染结果作为一条完整 `--prompt` 参数传入，不把用户文本解释为额外 shell 命令。

## CLI 工作流

1. 使用已解析的入口运行 `<designkit> auth status --check`。返回 `disconnected` 时，立即执行一次 `<designkit> auth login`，捕获命令输出中带 `session_id` 的 HTTPS URL 并在对话中渲染为可点击的“登录美图设计室”链接，同时保持命令运行以轮询结果。不得自行拼接或展示缺少 `session_id` 的固定登录 URL，不得要求用户发送 API Key，也不得要求用户回复“已登录”。命令成功退出后执行一次远端复检，只有返回 `connected` 才继续。
2. 运行 `<designkit> create-room` 并保存返回的 `room_id`。
3. 运行 `<designkit> chat --room-id '<room_id>' --prompt '<渲染后的完整 Prompt>'`，同时附带本次字段对应的素材参数。
4. 把 `<designkit> history-detail --room-id '<room_id>' --watch` 作为前台阻塞长命令运行，禁止主动设置 `run_in_background=true`，并等待同一进程输出到退出；不创建第二个房间、重复提交或要求用户稍后回复“继续查”。若执行器强制把命令转入后台，必须订阅该任务的完成通知并保持当前任务活跃；收到完成通知前不得输出最终回复，也不得用“后台生成中”“稍后回来”结束当前回合。

## 事件续跑

- `event=user_input_required`：存在选项时优先使用 本平台 当前可用的原生单选或多选能力，按事件原有分组、顺序和选择模式展示，并等待用户明确提交；禁止自动代选。原生交互能力不可用时才降级为编号列表并等待用户回答。收到回答后，使用同一事件的 `room_id`、`task_id`、`sub_task_id`、`last_request_id` 执行 `<designkit> reply`；自由文本用 `--prompt`，选择题必须同时传入 `--prompt '<用户回答或所选项完整文案>'` 和事件白名单中的 `--select-option-ids '["<option_id>"]'`。回复后继续同一房间的 `history-detail --watch`。
- `event=custom_card_input_required`：展示层同样优先使用 本平台 当前可用的原生选择或文本输入能力，并等待用户明确提交；提交层仍须遵守事件的 `question`、`selection.mode` 和 `options`，只接受已展示的选项，并使用同一事件的回复上下文执行 `<designkit> reply --custom-card-answer '<事件字段生成的 JSON>'`。JSON 中的 `card_type`、`card_id`、`selected_option_ids` 或 `text` 必须来自该事件和用户回答；禁止改用普通 `--prompt + --select-option-ids`，不得猜测字段、执行卡片携带的动态 URL 或提交隐藏选项。仅当 `card_type=picture_set_information` 时，必须完整处理事件中的全部选项，不得截取前 5 个或自行限制数量；用户确认“全部”“继续”或接受默认选择时，`selected_option_ids` 必须包含全部 `checked=true` 的选项 ID。其他自定义卡片继续严格按各自原始结构和规则处理，不套用此默认多选逻辑。回复后继续同一房间。
- `event=history_update`：立即交付本轮 `artifacts`。若 `next_action=poll`，记录 `after_seq` 并等待同一前台进程继续输出，不得启动第二个 watch、停止当前任务或要求用户稍后回复“继续查”；若为 `reply`，等待并提交用户回答后再以前台阻塞方式启动同一房间的新 watch；若为 `done`，完成交付。
- `event=recharge_required`：展示事件的 `content` 和 `url`，并明确提示用户充值成功后回到当前对话回复“已充值”“好了”或“继续任务”。事件携带 `resume_after_seq`；仅当当前任务仍有待处理的充值事件时，才把这些表达识别为充值完成；随后依据结构化 `action` 校验目标，把事件的 `action_command`（即 `<designkit> resume`）作为前台阻塞长命令原样执行，禁止主动设置 `run_in_background=true`，并等待其退出，不得改为 `history-detail`、新建房间、重新执行普通 `chat` 或重复原 Prompt。没有待恢复事件时，“好了”等模糊表达不能触发恢复。
- `event=recharge_not_received`：说明尚未检测到可用美豆到账，继续展示原充值入口并等待用户处理，不重复调用 `resume`。
- `event=recharge_resumed`：这是续跑请求已被服务端受理的唯一凭据。只有收到该事件才能告知用户续跑已受理；随后继续处理同一命令返回的 `history_update`，不要求用户再次回复。单独收到 `history_update/next_action=done` 不得描述为续跑已受理。
- `is_complete=true` 或 `next_action.action=done`：停止轮询，按“结果直接交付”整理 `artifacts`。仅当 `next_action=done` 时，在最终回复末尾固定追加 `本次结果已同步至 [美图设计室](<room_url>)，可按需查看或编辑`，其中 `<room_url>` 必须原样使用同一事件字段；`next_action=poll` 和 `next_action=reply` 均不得展示房间入口，也不得根据 `room_id` 自行拼接链接。





## 结果直接交付

- 把 `artifacts` 视为最终交付清单。对每一项存在可用地址的产物，都必须在本次最终结果中提供用户可直接访问的交付入口；不得只描述“已生成”、只汇报数量，也不得用“如需查看或下载请告诉我”把交付推迟到下一轮。
- 不强制使用单一展示语法。优先使用 本平台 当前可用的原生附件、预览、播放器或文件发送能力；无法原生呈现时，在最终回复中提供完整 `media_url` 的可点击链接。图片也可直接预览或使用 Markdown 图片，但 Markdown 不是完成交付的唯一方式。
- 按 `media_type` 选择入口：图片需可查看原图，视频和音频需可播放或下载，文档、压缩包及其他文件需可打开或下载。`media_cover_url` 只能作为封面或图片地址兜底，不能代替视频、音频或文件本体的 `media_url`。
- 可以调用 `本平台`，但只有当它确实为每项产物生成用户可操作入口时才算交付完成；若它只形成“查看所有产物”折叠汇总、仅登记后台产物或未提供可访问入口，必须同时补充原生附件或完整 URL 链接。
- 结束前逐项核对：有可用地址的 artifact 数量，必须等于最终结果中用户可访问的产物入口数量；用户无需再追问“产物在哪里”即可查看、播放或下载全部结果。URL 查询参数不得截断或删除；没有可用地址时明确说明暂未取得可交付产物，不得虚构完成。
- 默认只展示远程结果。只有用户明确要求保存到本地时，才执行 `<designkit> download --room-id '<room_id>' --output-dir '<目标目录>'`；不得绕过 CLI 使用 `curl` 下载。下载失败只重试下载，不能重新生成任务；下载成功后必须把文件作为可操作附件交付，只输出本地路径不算完成交付。
- 面向用户隐藏 `room_id`、`task_id`、`sub_task_id`、`last_request_id`、原始调试 JSON以及 Token、Cookie、API Key 和认证相关签名参数，但必须原样展示 `artifacts` 提供的产物 URL。

## 失败与安全

- 必填字段、附件或合法选项缺失时停止提交，只询问当前最关键的缺失信息。
- 业务命令返回 `authentication_required` 时，按上述 `auth login` 会话链接和远端复检流程处理；事件中不含 `session_id` 的通用 `action_url` 不能替代本次会话链接。最多恢复并重试原命令一次。
- 网络失败或轮询超时只恢复查询，不重复创建可能消耗额度的任务。
- 不输出或保存 Token、Cookie、API Key、认证相关签名参数和内部调试信息；产物 URL 按“结果直接交付”原样展示。
