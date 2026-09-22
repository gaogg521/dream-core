# 工作流解析与修复

## 1. 两种 JSON 形态（先分清，否则白修）

| 形态 | 特征 | 用途 |
|---|---|---|
| **UI 格式**（前端导出） | 顶层有 `nodes`、`links`、`groups`，每个 node 含 `type`、`widgets_values`、`pos` | 在网页里编辑用 |
| **API 格式**（"保存为 API 格式"） | 顶层是扁平字典，key 是序号字符串 `"3": {"class_type": "KSampler", "inputs": {...}}` | 脚本调用 / 排查最快 |

**排障优先要 API 格式的 JSON**，字段少、一眼看到 `class_type`。UI 格式的也能解析，用 `scripts/check_workflow.py`。

## 2. 典型报错与定位

### `Invalid node type` / `Node not found: <XXX>` / 节点红框
**根因**：工作流用了某个自定义节点，本机没装或该节点加载失败。
**定位**：
```bash
python scripts/check_workflow.py <工作流.json> --comfyui-dir <ComfyUI目录>
```
脚本会列出全部 `class_type` 与本机已注册节点，差集就是缺失节点。
**修复**：
1. 用 ComfyUI-Manager 搜节点名安装；
2. 或 `git clone` 节点仓库（只认官方公开地址）到 `custom_nodes/`；
3. 装完重启 ComfyUI，**仍红则看启动日志里的 `IMPORT FAILED`**（多半是依赖没装）。

### `Prompt outputs failed validation` / `Value not in list` / `Failed to validate prompt for output`
**根因**：某个下拉框的值（最常见是 checkpoint 名、lora 名、vae 名）本机不存在。
**定位**：`check_workflow.py` 会列出 `CheckpointLoaderSimple`、`LoraLoader`、`VAELoader` 等节点 widgets 里引用的文件名。
**修复**：把对应模型放到正确目录（见 `models-and-paths.md`），或在网页里手动改选现有模型后重新保存。

### `JSON parse error` / 加载后一片空白
**根因**：文件被截断、编码错误、或被当成图片下载（有些站点分享的是 png，元数据在 PNG chunk 里）。
**修复**：
1. 如果拿到的是 **PNG**：这是正常的——ComfyUI 把工作流嵌在 PNG 元数据里，直接把图片拖进 ComfyUI 画布即可，不需要提取 JSON。
2. 如果是 `.json` 打不开：用文本编辑器看最后一行是否完整；尝试从来源重新下载；不要手工修补大段 JSON，优先重新导出。

### 跨系统迁移后节点名对不上
**根因**：模型引用带子目录分隔符，Windows 是 `\`，Linux 是 `/`。
**修复**：改工作流里的路径分隔符，或把模型移到不带子目录的位置。

## 3. 手工排查速查（不用脚本时）

```bash
# 列出工作流里用到的所有节点类型（API 格式）
python -c "import json;d=json.load(open('workflow_api.json'));print(sorted({v['class_type'] for v in d.values() if 'class_type' in v}))"

# UI 格式
python -c "import json;d=json.load(open('workflow.json'));print(sorted({n['type'] for n in d['nodes']}))"

# 找所有引用的 checkpoint
python -c "import json;d=json.load(open('workflow_api.json'));print([v['inputs'].get('ckpt_name') for v in d.values() if v.get('class_type','').startswith('CheckpointLoader')])"
```

## 4. 修复流程（推荐顺序）

1. 让用户导出 **API 格式** JSON（网页：设置 → 开启 Dev mode → Save (API Format)）。
2. 跑 `check_workflow.py`，拿到"缺失节点清单"和"缺失模型清单"。
3. **先补节点，再补模型**——节点没装时，模型下拉框可能根本不会出现。
4. 每补一项重启一次 ComfyUI，再看剩余报错。
5. 全部补齐后，让用户重新在网页里保存一份，避免旧 JSON 里残留过期字段。

## 5. 不要做的事

- 不要为了"修好"而把报错节点从 JSON 里硬删掉——会破坏连线逻辑，正确做法是装上节点。
- 不要相信来路不明的"一键修复脚本"。
- 修改前先备份原 JSON。
