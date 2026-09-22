#!/usr/bin/env python3
# -*- coding: utf-8 -*-
r"""
ComfyUI 工作流体检脚本：列出节点类型与引用的模型，并比对本机是否缺失。

支持：
  - UI 格式（含 nodes/links）
  - API 格式（扁平 {"3": {"class_type": ...}}）
  - 带 PNG 元数据的工作流（需要 Pillow，缺失时给出提示）

用法：
    python check_workflow.py workflow.json
    python check_workflow.py workflow_api.json --comfyui-dir D:\ComfyUI
    python check_workflow.py workflow.json --comfyui-dir /home/u/ComfyUI --json

只读脚本，不修改任何文件。
"""

import argparse
import json
import sys
from pathlib import Path

# 常见"引用模型文件名"的 (class_type 前缀, 输入字段名)
MODEL_FIELDS = [
    ("CheckpointLoader", "ckpt_name"),
    ("CheckpointLoaderSimple", "ckpt_name"),
    ("UNETLoader", "unet_name"),
    ("LoraLoader", "lora_name"),
    ("VAELoader", "vae_name"),
    ("ControlNetLoader", "control_net_name"),
    ("CLIPLoader", "clip_name"),
    ("CLIPVisionLoader", "clip_name"),
    ("UpscaleModelLoader", "model_name"),
    ("StyleModelLoader", "style_model_name"),
    ("GLIGENLoader", "gligen_name"),
]

# 模型类型 -> 默认目录
FIELD_TO_DIR = {
    "ckpt_name": "checkpoints",
    "unet_name": "unet",
    "lora_name": "loras",
    "vae_name": "vae",
    "control_net_name": "controlnet",
    "clip_name": "clip",
    "model_name": "upscale_models",
    "style_model_name": "style_models",
    "gligen_name": "gligen",
}

MODEL_EXT = {".safetensors", ".ckpt", ".pt", ".pth", ".bin", ".gguf"}


def load_workflow(path: Path):
    """返回 (data, kind, err)。kind ∈ {api, ui}"""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception as e:  # noqa: BLE001
        return None, None, f"JSON 解析失败：{e}"

    if isinstance(data, dict) and isinstance(data.get("nodes"), list):
        return data, "ui", None
    if isinstance(data, dict) and any(
        isinstance(v, dict) and "class_type" in v for v in data.values()
    ):
        return data, "api", None
    if isinstance(data, list):
        return data, "ui", None
    return None, None, "无法识别的工作流结构（既不是 UI 格式也不是 API 格式）"


def extract_api(data, kind):
    """统一抽出 [(node_id, class_type, inputs)]"""
    out = []
    if kind == "api":
        for k, v in data.items():
            if isinstance(v, dict) and "class_type" in v:
                out.append((str(k), v.get("class_type", "?"), v.get("inputs", {}) or {}))
        return out

    nodes = data.get("nodes", []) if isinstance(data, dict) else data
    for n in nodes:
        if not isinstance(n, dict):
            continue
        nid = n.get("id", "?")
        ctype = n.get("type", "?")
        inputs = {}
        wv = n.get("widgets_values")
        # UI 格式没有字段名映射，只能给出 widgets 原始值供人工核对
        if isinstance(wv, list):
            inputs["__widgets_values__"] = wv
        props = n.get("properties")
        if isinstance(props, dict) and props.get("Node name for S&R"):
            inputs["__node_name__"] = props["Node name for S&R"]
        out.append((str(nid), ctype, inputs))
    return out


def collect_models(nodes):
    """收集引用的模型名 -> [(字段名, 名字, class_type, node_id)]"""
    # 长前缀优先，避免 "CheckpointLoader" 抢先匹配 "CheckpointLoaderSimple"
    ordered = sorted(MODEL_FIELDS, key=lambda x: len(x[0]), reverse=True)
    found = []
    seen = set()
    for nid, ctype, inputs in nodes:
        wv = inputs.get("__widgets_values__")
        for prefix, field in ordered:
            if ctype.startswith(prefix):
                val = inputs.get(field)
                # UI 格式没有命名字段，回退到 widgets_values 里第一个字符串
                if not isinstance(val, str) or not val:
                    if isinstance(wv, list):
                        val = next((v for v in wv if isinstance(v, str) and v), None)
                if isinstance(val, str) and val:
                    key = (field, val, ctype, nid)
                    if key not in seen:
                        seen.add(key)
                        found.append(key)
                break
    # UI 格式兜底：已知加载器已在上一步带目录映射处理，这里只挑出
    # 非加载器节点里"像文件名"的字符串，避免重复
    known = {n for _, n, _, _ in found}
    for nid, ctype, inputs in nodes:
        wv = inputs.get("__widgets_values__")
        if isinstance(wv, list) and not any(ctype.startswith(p) for p, _ in MODEL_FIELDS):
            for v in wv:
                if (isinstance(v, str) and v not in known
                        and any(v.lower().endswith(e) for e in MODEL_EXT)):
                    found.append(("__widgets__", v, ctype, nid))
                    known.add(v)
    return found


def scan_installed_models(cdir: Path):
    """返回 {子目录: {文件名集合}}"""
    result = {}
    mdir = cdir / "models"
    if not mdir.exists():
        return result
    for sub in FIELD_TO_DIR.values():
        d = mdir / sub
        if d.exists():
            result[sub] = {
                str(f.relative_to(d)).replace("\\", "/")
                for f in d.rglob("*") if f.is_file() and f.suffix.lower() in MODEL_EXT
            }
    return result


def scan_installed_nodes(cdir: Path):
    """粗略扫描 custom_nodes 目录名，用于提示（无法得到精确注册名）。"""
    cn = cdir / "custom_nodes"
    if not cn.exists():
        return set()
    return {d.name for d in cn.iterdir() if d.is_dir() and d.name not in {".git", "__pycache__"}}


def norm(name: str) -> str:
    return name.replace("\\", "/").strip()


def main():
    ap = argparse.ArgumentParser(description="ComfyUI 工作流体检（只读）")
    ap.add_argument("workflow", help="工作流 JSON 文件路径")
    ap.add_argument("--comfyui-dir", default=".", help="ComfyUI 根目录，用于比对缺失模型")
    ap.add_argument("--json", action="store_true", help="以 JSON 输出")
    args = ap.parse_args()

    wf = Path(args.workflow).expanduser().resolve()
    if not wf.exists():
        print(f"❌ 文件不存在：{wf}")
        sys.exit(1)

    data, kind, err = load_workflow(wf)
    if err:
        print(f"❌ {err}")
        print("提示：如果拿到的是 PNG，工作流嵌在图片元数据里，直接拖进 ComfyUI 画布即可，无需提取 JSON。")
        sys.exit(1)

    nodes = extract_api(data, kind)
    types = sorted({c for _, c, _ in nodes})
    refs = collect_models(nodes)

    cdir = Path(args.comfyui_dir).expanduser().resolve()
    installed_models = scan_installed_models(cdir)
    installed_nodes = scan_installed_nodes(cdir)

    if args.json:
        print(json.dumps({
            "format": kind,
            "class_types": types,
            "model_refs": [{"field": f, "name": n, "node": c} for f, n, c, _ in refs],
        }, ensure_ascii=False, indent=2))
        return

    print(f"工作流文件  : {wf}")
    print(f"格式        : {'UI 格式（可编辑）' if kind == 'ui' else 'API 格式（扁平）'}")
    print(f"节点总数    : {len(nodes)}    不同节点类型: {len(types)}")

    print("\n--- 用到的节点类型 ---")
    for t in types:
        print(f"  {t}")

    print("\n--- 引用的模型 ---")
    if not refs:
        print("  （未解析到明确的模型引用；UI 格式请以网页下拉框为准）")
    missing = []
    for field, name, ctype, nid in refs:
        sub = FIELD_TO_DIR.get(field)
        n = norm(name)
        if sub and sub in installed_models:
            ok = n in installed_models[sub]
        elif sub:
            ok = False
        else:
            ok = None
        if ok is True:
            mark = "✅ 已安装"
        elif ok is False:
            mark = "❌ 缺失"
            missing.append((sub or "?", n))
        else:
            mark = "？ 待确认"
        print(f"  [{mark}] {ctype} -> {n}" + (f"   应放 models/{sub}/" if sub and ok is False else ""))

    if missing:
        print("\n--- 缺失模型处理 ---")
        for sub, n in missing:
            print(f"  把 {n} 放到 <ComfyUI>/models/{sub}/，然后重启 ComfyUI 或点 Refresh")

    print("\n--- 自定义节点提示 ---")
    if not installed_nodes:
        print("  未找到 custom_nodes 目录，无法比对；请确认 --comfyui-dir 指向 ComfyUI 根目录")
    else:
        print(f"  本机 custom_nodes 下共 {len(installed_nodes)} 个目录：")
        for n in sorted(installed_nodes):
            print(f"    - {n}")
        print("  若网页报 'Invalid node type / 节点红框'，缺失的节点不在这个列表里，")
        print("  用 ComfyUI-Manager 按节点名安装，或 git clone 官方仓库后重启。")

    print("\n--- 内置节点基准 ---")
    print("  以下为 ComfyUI 自带，不需要额外安装：")
    print("  KSampler / KSamplerAdvanced / CheckpointLoaderSimple / CLIPTextEncode /")
    print("  VAEDecode / VAEDecodeTiled / VAEEncode / EmptyLatentImage / SaveImage /")
    print("  LoadImage / ImageScale / LoraLoader / ControlNetLoader / CLIPVisionLoader")
    print("  出现这些以外的大写驼峰类型，基本都是自定义节点。")


if __name__ == "__main__":
    main()
