---
name: supertonic-tts
display_name: "本地超快TTS合成"
description: 用 supertonic-mnn（Supertone TTS on MNN）本地合成高质量多语言音频。CLI + Python API，支持中英日韩等 30+ 语言，10 种音色（。触发：用户提出「本地超快TTS合成」相关需求时使用（常见说法：本地超快TTS合成、本地超快TTS合成；英文：supertonic/tts）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# supertonic-tts

Supertone 的开源 TTS 模型 `supertonic`，通过 MNN 推理引擎在 Mac CPU 上做极速本地合成。适合需要"开箱即用、零网络、隐私安全"的高质量英文/多语言配音场景。

## 何时用

- 需要生成短到中等长度（句子、段落级别）的高质量英文/多语言音频。
- 不想走云 API、想完全离线 / 隐私优先。
- 想批量合成、给视频/播客/学习材料配音。
- 已有 Whisper/f5-tts 之类方案，但想换更轻量、推理更快的 TTS。

不适合：超长篇章一次性合成（会非常慢且爆内存），超拟真情感/克隆需求（它是通用合成，不是声音克隆）。

## 前置

- **Python 3.10+**（Mac 上 MNN 约束 3.10；Windows 实测 3.11 可用）。
- HF 模型缓存目录：`~/.cache/supertonic-mnn`（首次运行自动下载 ~150 MB int8 模型）。
- 默认下载源 `huggingface.co` 在国内/部分网络会 502，**国内网络务必设置 `HF_ENDPOINT=https://hf-mirror.com`** 走镜像。

### Windows 注意（实测）

- 安装走 pip 即可：`pip install supertonic-mnn`（不必建 venv，`scripts/speak.sh` 是 bash/macOS 专用）。
- **已知坑**：`supertonic_mnn/model.py` 在 Windows 上用 `path.split("/")` 切 voice_styles 文件名，反斜杠会混进 HF 下载 URL 导致 `Entry Not Found ... %5Cvoice_styles%5C`。首次合成前手动补齐即可跳过坏代码：

```powershell
mkdir "$env:USERPROFILE\.cache\supertonic-mnnoice_styles" -Force
foreach ($v in "M1","M2","F1","F2") {
  curl.exe -sL -o "$env:USERPROFILE\.cache\supertonic-mnnoice_styles\$v.json" `
    "https://hf-mirror.com/yunfengwang/supertonic-tts-mnn/resolve/main/voice_styles/$v.json"
}
```

- 之后正常调用：`python -c "from supertonic_mnn import SupertonicTTS; ..."`（见下方 Python API）。

## 安装

- **Windows**：`pip install supertonic-mnn`（技能目录无需 venv；合成示例见下方 Python API 与上方 Windows 注意）。
- **macOS**：skill 自带隔离的 venv 位于 `~/.dream/skills/supertonic-tts/venv/`，依赖 `supertonic-mnn`。如丢失：

```bash
brew install python@3.10  # 或用 uv 拉 3.10
~/.dream/skills/supertonic-tts/scripts/setup.sh
```

## 调用方式

### 快速合成（CLI）

```bash
HF_ENDPOINT=https://hf-mirror.com \
  ~/.dream/skills/supertonic-tts/venv/bin/supertonic-mnn \
  -o /tmp/out.wav --voice F1 --precision int8
```

文本从 stdin 读，或用 `-i sentences.txt` 每行一句批量合成。

### 推荐封装脚本

`scripts/speak.sh "文本" [输出.wav] [音色] [precision]`

```bash
~/.dream/skills/supertonic-tts/scripts/speak.sh \
  "Hello world." /tmp/hello.wav M1 int8
```

`scripts/speak.py` 是 Python 版本，可用于程序内调用。

## 参数速查

| 参数 | 说明 | 默认 |
|---|---|---|
| `--voice / -v` | M1-M5 男声，F1-F5 女声；也可指向自定义风格 json | M1 |
| `--precision / -p` | fp32 / fp16 / int8（**推荐 int8**，RTF ~0.07，几无质量损失） | fp16 |
| `--steps / -s` | 去噪步数，越大越慢越平滑 | 5 |
| `--speed` | 语速倍率，1.0 原速 | 1.0 |
| `--output / -o` | 输出 wav | output.wav |
| `--input-file / -i` | 每行一句的批量输入 | stdin |
| `--model-dir` | 模型缓存目录 | `~/.cache/supertonic-mnn` |

## 音色

v3 包含 M1-M5、F1-F5 共 10 种。常用：
- **M1**：英文男声，最稳；新闻/旁白风。
- **F1 / F2**：英文女声，干净清晰，适合学习素材。
- **M2**：男声，偏低沉。

## Python API 例子

```python
from supertonic_mnn import SupertonicTTS

tts = SupertonicTTS(precision="int8")  # 模型会自动下载
audio, sr = tts.synthesize(
    "Hello world.",
    voice="F1",
    output_file="out.wav",
)
```

## 性能参考（Mac M 系列 CPU）

实测 Mac mini M4 / M-series：

| 精度 | RTF | 备注 |
|---|---|---|
| int8 | ~0.07-0.09 | **首选**，无明显质量损失 |
| fp16 | ~0.2-0.3 | 略大模型，兼容性最好 |
| fp32 | ~0.5+ | 仓库默认 fp16，老 CPU 兜底 |

10 秒英文音频 ≈ 0.6-1 秒生成。

## 常见坑

1. **`502 Bad Gateway` 下载失败**
   → 国内网络必设 `HF_ENDPOINT=https://hf-mirror.com`。

2. **`Python 版本不符` 报错**
   → MNN 仅支持 3.10。需用 `/opt/homebrew/bin/python3.10`（或 `uv python install 3.10`）建独立 venv，**不能**用托管的 3.13/3.14。

3. **首次推理比后续慢**
   → MNN 首次跑会编译 cache，后续秒出。

4. **导出音频是 wav/PCM 44100Hz mono 16-bit**
   → 不直接是 mp3，要 mp3 用 ffmpeg 转：`ffmpeg -i out.wav -b:a 128k out.mp3`。

5. **想拼接长文**
   → 按句切分，逐句调用 `synthesize`，再 `ffmpeg` concat；不要一次塞整本书。

6. **采样率异常**
   → 默认就是 44.1kHz / mono，主流播放器和剪辑工具都吃。

## 验证记录

- Mac mini M 系列，Python 3.10.17，int8 精度：
  - 9.40s 英文 / M1 → 生成 2.63s（RTF 0.28）
  - 6.83s 英文 / F1 → 生成 0.63s（RTF 0.09）
  - 输出 mono 44100Hz 16-bit wav

## 相关链接

- 项目主页：https://supertonictts.com
- GitHub：https://github.com/vra/supertonic-mnn
- 模型仓库：https://huggingface.co/yunfengwang/supertonic-tts-mnn
- 原版 ONNX：https://huggingface.co/Supertone/supertonic-2
- 论文：arXiv:2503.23108