---
name: screen-automation
display_name: "屏幕自动化"
description: 增强 Agent 的屏幕理解和控制能力，利用本地屏幕视觉技术提高界面识别与定位效率；基于屏幕自动化小助手在 Windows 或 macOS 上确认目标窗口并安全完成当前屏幕任务。触发：用户提出「屏幕自动化」相关需求时使用（常见说法：屏幕自动化、屏幕自动化；英文：screen/automation）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# 屏幕自动化

使用“屏幕自动化小助手”完成当前屏幕任务，并为流程工程提供唯一的 CLI、能力、安全和流程标准。
需要把任务沉淀为可复用流程、维护已有流程或处理 `执行者：agent` checkpoint 时，由承载此 Skill
的工程师角色执行；能力契约仍以本 Skill 为准。

## 系统全景

系统由三个独立安装和升级的核心层组成：桌面平台负责本地感知与执行，Agent Skill 负责方法和安全
约束，经验流程负责可复用的业务步骤与验证。DSH 等面向特定 Agent 运行时的适配属于独立桥接交付物，
不是第四个核心层；应用增强、屏幕连接器和 Agent Provider 属于桌面平台的可选扩展。

分析能力时先分层，再分别盘点成熟度和缺口。新版桌面端用三个只读入口回答不同问题：

- `cli describe`：产品结构、运行特性和安全契约；
- `cli capabilities`：当前安装版本声明的 CLI/SDK 动作契约，不代表产品架构或可选组件状态；
- `cli access list`：能力门类、来源、授权、依赖和访问状态，不是完整动作清单。

调用方必须忽略 `cli describe` 中未知的新增字段。旧桌面端可能没有 `cli describe`：若该命令明确返回
不支持或未知命令，继续使用本节的三层摘要，并以 `cli capabilities`、`cli access list` 和 `cli health`
分别核对动作、访问状态和运行环境；不得因此把旧版输出误解为完整系统模型，也不得把缺少自描述命令
当成桌面动作能力缺失。

## 连接

先按本机平台取得小助手原生 CLI 路径并读取真实能力：

```powershell
$AppCli = & "<Skill目录>\scripts\resolve_cli.ps1"
& $AppCli cli status
& $AppCli cli describe
& $AppCli cli capabilities
& $AppCli cli access list
```

```bash
APP_CLI="$(bash "<Skill目录>/scripts/resolve_cli.sh")"
"$APP_CLI" cli status
"$APP_CLI" cli describe
"$APP_CLI" cli capabilities
"$APP_CLI" cli access list
```

不得猜测安装位置、递归扫描用户目录或绕过小助手改用其他输入工具。找不到桌面端时停止，并让用户自行
下载安装：Windows `https://www.xiaozs.com/sah/downloads/windows/latest`；macOS
`https://www.xiaozs.com/sah/downloads/mac/latest`。未经同意不下载、安装、重启或升级。

Skill 与桌面端版本号相互独立。只以 `cli status` 和 `cli capabilities` 的当前返回为准；能力未列出或
命令报告不支持时停止该调用，不猜参数、不声称功能已存在。

## 低版本桌面端兼容

最新 Skill 可以配合较早的小助手完成该版本已经声明的基础任务，但不能把最新文档中的命令、流程语法或
扩展能力当成旧版已经具备。逐个执行只读探测并分别记录结果；一个查询命令不存在，不代表其他查询或基础
屏幕动作也不存在：

- **基础屏幕任务**：只有 `status`、`capabilities` 和任务所需动作均成功声明时才继续；未列出的命令或
  参数不调用，也不改用其他输入工具绕过小助手。
- **流程创建或修改**：除动作探测外，还必须让当前桌面端返回 `cli workflow schema`，并用同一桌面端
  执行 `workflow validate`。无法取得当前流程语言契约时，可以运行该版本已安装且健康检查通过的流程，
  但停止创建、升级或修复流程，不能凭最新版标准生成后尝试安装。
- **可选扩展**：`access list`、组件状态或 Agent Provider 探测无法执行时，将状态记为“无法确认”；不进入
  不存在的设置入口，不把未授权、未安装、平台不支持和版本过旧混成同一种故障。

`describe`、`access list`、`workflow schema` 和具体动作都要独立容错。旧版不支持某项能力时，向用户说明
“当前桌面版本仍可使用哪些基础能力、哪项任务暂不能安全执行”；只有用户询问或任务确实需要时才提供官方
下载地址，由用户决定是否升级。不得自动下载、安装、重启或中断正在运行的小助手。

## 完成当前任务

1. 用 `cli window list-visible` 或 `cli window wait-selection` 确认目标窗口；有多个候选时让用户选择。
2. 用 `cli task begin` 绑定目标，再用 `cli task observe` 获取当前可见状态。
3. 优先依据当前观察结果定位。坐标只能来自本次有效观察或明确的相对规则，不能沿用旧截图坐标。
4. 每次改变屏幕前确认动作仍在用户授权范围内；关键或不可逆动作在执行前单独确认。
5. 点击、输入、滚动或拖动后重新观察并验证可见结果。命令成功只证明事件已发送。
6. 完成、失败或用户停止时执行 `cli task end`，报告结果、未完成项和保存位置。

常用原生命令形态：

```text
cli task begin [--handle <窗口句柄>]
cli task observe
cli task find --text <文字>
cli task wait --text <文字> --timeout <秒> --interval <秒>
cli task click --point <x,y> --button left
cli task write --text <文字> --interval <秒>
cli task scroll --point <x,y> --amount <数值> --direction up|down|left|right
cli task hotkey <按键...>
cli task end
```

具体参数仍以 `cli capabilities` 为准。横向滚动必须使用 `--direction left|right`，不得以
`Shift+滚轮` 假冒。事件发送后若画面未变化，按 `no_change` 处理。

## 可选能力

浏览器增强、Agent 接入和 VLM 屏幕理解均可能需要单独组件或能力码。能力缺失、未授权或需要激活时停止，
由用户联系开发者购买并自行激活；不得猜测、索取、记录、代输或绕过能力码。浏览器增强不接管用户日常
Profile，也不是独立执行者。

任务涉及 `执行者：agent`、Agent Bridge、Provider、DSH、VLM、截图外发或 Agent 接入诊断时，必须先
完整读取 [Agent 接入与视觉模型标准](references/agent-bridge.md)。创建、修改或修复流程前，必须完整
读取 [流程开发标准](references/workflow-standard.md)，并使用本目录 `scripts/workflow_dev.ps1` 或
`scripts/workflow_dev.sh` 定位 CLI 和执行标准中明确列出的兼容桥接动作。不得凭示例或旧对话补写规则。

流程工程坚持“确认窗口—只读识别—页面判断—模拟运行—监督操作—失败恢复—小批量”的逐级验收；
流程语言 v2 中的变量、条件、有界循环、有界重试、验证和人工确认优先于原子动作堆叠。每个改变页面
的动作都必须有操作前观察和操作后验证，权限保持最低必要，第三方流程不得冒充目标平台官方能力。

## 安全边界

- 密码、验证码、密钥和支付信息不得写入日志、流程或结果；需要时让用户在界面中自行输入。
- 截图或屏幕内容发送到外部模型前，明确 Provider、范围和用途并取得确认；避开无关隐私。
- 不扩大用户授权，不后台接管未确认窗口，不用技术成功替代业务结果验证。
- 用户说暂停、停止或取消时立即停止后续操作。
