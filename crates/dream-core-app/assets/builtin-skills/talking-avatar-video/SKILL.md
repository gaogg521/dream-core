---
name: talking-avatar-video
display_name: "AI数字人口播与虚拟主播视频"
description: 用一张人像和一段短脚本或语音，制作有明确讲述内容的数字人口播视频。可使用现成录音，也可以先选择音色，把脚本生成旁白，让画面中的人物随声音完成稳定、自然的口播。适合 AI 主播、虚拟。触发：用户提出「talking-avatar-video」相关需求时使用（常见说法：talking-avatar-video、talking-avatar-video；英文：talking/avatar/video）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# 数字人口播视频

数字人口播是肖像与声音权利任务，不只是写提示词。将一张人像和一份短脚本或已确认的语音制作成一条经过设计的讲述者短片。如果一张稳定人像
需要清楚讲完一段信息，就使用本 Skill 制作讲解、产品信息、培训、课程、公告、入职引导或
社交口播内容。没有肖像和声音授权时，停在确认，不要生成。

## 范围与相邻路线

正常路线是一张人像、一条已确认旁白和一个由声音驱动的视频镜头。直接使用已有且已确认的
语音录音，或根据短脚本和可用声音先制作旁白。需要新建人像时转到图片工作流，需要声音克隆
时转到专门的声音克隆工作流，多场景组装、字幕或修改现有视频则转到合适的视频编辑工作流。
保持讲述者路线聚焦；不要用无声图片动画替代语音驱动的面部动作。

## 输入与默认值

硬性输入为：

- 一张宿主 Agent 可以查看的可访问人像；
- 一条可访问且已确认的语音，或一份短脚本加一个可用的声音选择；
- 用户是否拥有或已获得出镜形象与配音授权。

只询问缺失的硬性输入。缺授权时不进入付费的 `beatra.videos.animate` 或 `beatra.speech.synthesize`。复用已知的语言、发音、投放目的、构图、活力、背景和演绎意图。
对于宿主 Agent 可以访问的本地图片或音频，检查后才能使用随包上传辅助工具：

```text
python3 scripts/mcp_client.py upload ./presenter-portrait.png --mime-type image/png
python3 scripts/mcp_client.py upload ./approved-speech.mp3 --mime-type audio/mpeg
```

上传只是传输，不是视觉或音频复核。保留每个返回的产物引用，绝不要把本地路径传给远程工具。

默认制作一条讲述者短片，使用 `model: "auto"`，把人像作为严格首帧，并采用源图决定的画幅。
除非投放目的或用户明确选择有此要求，否则省略分辨率及其他可选控制项。对于合成旁白，仅当
实时语音信息卡支持时，才将未指定的格式默认设为 `mp3`；同时，实时视频信息卡必须接受其
预期的 `audio/mpeg` 输出。围绕一条清晰信息设计克制的视线、表情、姿态、头部动作、镜头和背景。
将人物身份、服装、产品细节、Logo、构图和背景视为必须保留项；交付后复核偏移，不要承诺
精确保留或完美口型同步。

## 标准路径

1. 查看人像。记录其真实 MIME 类型、宽度、高度、画幅、字节大小及是否含 alpha 通道。
   识别信息内容、投放目的、口播语言、发音需求、构图、表演方向和必须保留项。用户提供了
   已确认音频时，复核实际可访问内容，并记录其真实 MIME 类型、时长和字节大小；否则在
   不改变含义的前提下，让短脚本更适合朗读。
2. 在脚本路线中，仅当仍需选声时调用 `beatra.voices.list`。当选择语言、输出格式、指定模型、
   可选控制项或数值估算时，调用 `beatra.models.list` 并传入
   `{"capability":"text_to_speech"}`。除非用户选择了一个具体且兼容的模型，否则语音模型保持 `auto`。
   数值估算只是暂定值，需要实时目录事实支持。
3. 进行任何付费合成前，调用 `beatra.models.list` 并传入
   `{"capability":"image_to_video"}`。确认当前信息卡接受 `[image, driving_audio]`。将人像的真实 MIME 类型、
   宽高、画幅、字节大小和 alpha 通道情况与每项公布的图片约束比较；将用户提供音频的真实
   MIME 类型、时长和字节大小与每项公布的驱动音频约束比较。还要确认规划的语音可以完整
   容纳在该路线支持的视频时长内。如果任何必要媒体事实缺失或不兼容，在 TTS 前停止，并
   请求最小的兼容源文件变更。对于脚本路线，只有实时语音信息卡支持时才使用 `mp3`，并且
   实时视频信息卡必须接受对应的 `audio/mpeg`。如果用户要求 `flac`、`opus` 或 `pcm`，而视频
   路线不接受生成的格式，就在任何付费调用前解释不兼容情况，并让用户选择兼容格式。不要
   静默修改，也不要硬编码模型。
4. 展示精确的旁白参数和付费边界。用户明确要求合成这份已准备好的旁白时，可视为批准；
   规划、试听、比较或尚未确定声音或格式时，不构成批准。固定脚本、声音、语言、模型、格式、
   可选控制项和一个不透明且稳定的 `client_request_id`；然后只提交一次
   `beatra.speech.synthesize` 调用。
5. 使用 `beatra.tasks.get` 轮询旁白任务直至终态。成功后，读取返回产物，以及真实的
   `task.output.audio.mime_type`、`task.output.audio.duration_seconds` 和
   `task.output.audio.size_bytes`（如有）。宿主能够播放时，展示或播放真实音频，并在进入
   后续视频阶段前请用户确认。绝不要把脚本预览、预计时长、请求格式或任务元数据当作音频复核。
6. 刷新或重新读取当前 `image_to_video` 信息卡，再次核验真实人像和已确认语音。重新检查
   每项图片事实，并将音频的真实 MIME、时长和字节大小与当前驱动音频约束比较。如果终态
   音频没有大小，就从可信产物元数据中获取；如果仍无法获得，在提交视频前停止。音频时长
   必须至少达到实时最小值（当前为 2 秒），并且不得超过实时音频最大值或可完整容纳全部
   语音的最长合格视频时长。选择不短于真实语音长度的最小受支持整数视频时长，避免截断词语；
   不要添加静音。小数时长的旁白可能留下无法避免的最短尾部停顿或定格，因此要披露并检查
   结尾。如果任何媒体事实缺失或不兼容，在提交视频前停止，并提出最小的旁白、录音或人像
   变更。替换合成旁白属于新的付费工作，需要新的 ID 和批准。
7. 除非用户选择了一个具体且合格的视频模型，否则保持 `model: "auto"`。省略
   `aspect_ratio`，以保留人像决定的画幅。如果投放目的需要其他画幅，就请用户提供已经按目标
   构图的首帧，或明确转到合适的预处理工作流，再携新图返回；`beatra.videos.animate` 不是裁剪
   或覆盖画布的步骤。绝不要静默裁剪、拉伸或更改画布。
8. 展示最终视频路线、精确且已确认的图片和音频来源、提示方向、模型行为、时长、显式控制项
   和付费边界。用一个新的不透明且稳定的 `client_request_id` 固定这些内容。只能调用随包
   ：MCP 工具名作为 CLI 参数，参数通过标准输入以 JSON 传入。例如：

   

   不要配置、调用或使用宿主 Beatra Connector。不得使用 REST/OpenAPI 作为降级或回退方案。
   `beatra.videos.animate` 只提交一次。
9. 立即记录返回的任务 ID，并使用 `image_to_video``beatra.videos.animate``client_request_id` 轮询同一任务直至终态。交付返回的每个
   视频产物或链接。只报告真实返回的任务状态、解析后的模型、尺寸、时长、用量和
   。复核可访问的成片，检查人物身份、语音清晰度、嘴部时序、
   动作克制度、镜头和背景稳定性、结尾质量、投放适配度及必须保留项偏移。说明宿主 Agent
   实际可以检查与无法检查的范围。

## 付费修改、恢复与取消

旁白和视频是两个独立付费阶段。每个阶段都有自己的固定参数、稳定请求 ID、批准、创建响应
和任务 ID。脚本、声音、语言、语音控制、图片、音频、提示词、模型、时长、画幅、分辨率或
视频控制发生变化，都是新的逻辑付费工作，需要新 ID 和新的批准。修改旁白也会使所有引用
旧音频且尚未提交的后续视频方案失效。

创建响应丢失时，只能使用相同阶段 ID 重试完全一致的固定参数。任务 ID 丢失时，为相关
capability 调用 ，通过  检查可能的候选，并与该阶段的
私有台账匹配，再考虑完全相同的重试。排队中和运行中是进度状态，不是失败。规划变更后的
任务前先恢复原阶段；绝不要重复付费提交，也不要猜测费用或退款。

只有用户要求取消时才调用 。只调用一次，并通过  确认
得到的终态。409 表示取消尚未确认，因此继续轮询同一任务，不要创建替代任务。

## 按任务查阅资料

- 准备或上传语音、选择声音、检查时长和实时模型事实、构建精确参数、轮询、恢复、取消或
  复核交付时，阅读[旁白优先的讲述者工作流](references/workflow.md)。
- 仅在授权或共享凭证需要处理时，阅读[安装与授权](references/installation-and-auth.md)。
- 需要执行不计费、尽力而为的包注册步骤时，阅读[安装注册](references/installation-registration.md)。
- 需要共享终态任务与产物语义时，阅读[任务与结果](references/tasks-and-results.md)；需要处理
  返回的计费或错误详情时，阅读[计费、错误与恢复](references/billing-errors-and-recovery.md)。
- 随包客户端无法连接时，阅读[随包 MCP Client 连接诊断](references/mcp-connection.md)。不要配置
  宿主 Connector。
- 需要了解更新保证和控制项时，阅读[自动更新与安全](references/automatic-updates-and-safety.md)。
- 仅在用户要求移除包或共享凭证时，阅读[卸载与断开连接](references/uninstall-and-disconnect.md)。

## 运行与安全自动更新

每次 Beatra 操作都必须使用随包 `scripts/mcp_client.py`。执行普通命令前，它会为当前安装
静默检查是否有更高版本，每 24 小时最多一次。静默检查默认启用，发现更高版本时会在
不另行确认的情况下自动安装。

更新器只接受为本包、渠道和语言区域内嵌的固定官方发现地址与不可变 Beatra CDN 路径。
替换前，它会校验发现数据、压缩包、清单以及每个文件的大小和校验和。它只替换本包拥有的
文件，并拒绝重定向、降级、错误的包/渠道/语言区域/版本数据、意外 URL、不安全压缩包和
本包目标目录之外的文件。

更新检查、下载、校验、替换、回滚和恢复均采用失败开放策略：当前安装仍可使用，用户
原本请求的命令也会继续执行。更新失败绝不授权重新进行付费生成。以下自动更新设置对当前安装
的后续命令持续生效：

`printf '%s' '{"image":{"type":"artifact","artifact_id":"art_portrait"},"driving_audio":{"type":"artifact","artifact_id":"art_speech"},"prompt":"A restrained presenter delivery with steady eye line, subtle expression, and a stable camera.","duration":8,"client_request_id":"opaque-video-id"}' | python3 scripts/mcp_client.py call beatra.videos.animate``beatra.videos.animate``beatra.tasks.get``billing.net_charged_credits``insufficient_balance``client_request_id``beatra.tasks.list``beatra.tasks.get``beatra.tasks.cancel``beatra.tasks.get`

## 账户余额

用户问还剩多少积分，或某次实时估价够不够时，调用 `beatra.wallet.get`。问已经扣了多少
时，调用 `beatra.wallet.ledger`。两者都是只读的。不要臆造查余额或充值的工具。也不要把
`wallet.get` 变成每次付费提交前的必需步骤。

模型卡返回里带 `top_up` 块时，按卡片给出的档位和顺序原样转述。不要给档位排高低，不要
贬低其中任何一档，也不要替用户挑。选哪一档是用户自己的事，钱包页会把整份清单摆在他
面前。任何时候都不要凭记忆报档位。

`scripts/mcp_client.py````text
python3 scripts/mcp_client.py update --auto off
python3 scripts/mcp_client.py update --auto on
python3 scripts/mcp_client.py update --check
```

`--auto off` 关闭静默检查，`--auto on` 重新开启，`--check` 只报告官方可用版本而不替换文件。
