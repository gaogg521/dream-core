# 修复提示词模板库（Prompt Library）

所有模板遵循同一原则：**先声明修复、后列出问题、最后加保真约束**。直接复制段落拼接即可，prompt 建议使用英文（图生图模型对英文指令响应更稳定），中文说明用于辅助理解。

## 1. 核心指令（必选，二选一）

```
EN: Restore and enhance this old photograph. Repair visible damage and improve clarity while keeping it looking like an authentic vintage photo.
CN: 修复并增强这张旧照片，修复可见损伤、提升清晰度，同时保持真实的年代老照片质感。
```

分步修复的「结构修复」阶段用：

```
EN: Carefully repair the damaged/torn areas of this old photo, reconstructing the missing parts to match the surrounding content. Do not alter the intact areas.
CN: 仔细修复照片的破损/撕裂区域，按周围内容补全缺失部分，不得改动完好区域。
```

## 2. 问题段落（按需拼接，可多段并列，用逗号分隔）

| 问题 | Prompt 段落 |
|---|---|
| 噪点颗粒 | EN: remove film grain, noise and speckles, smooth the texture |
| 划痕 | EN: erase all scratches, scan lines and hairline marks across the image |
| 折痕/褶皱 | EN: remove creases and fold lines, flatten the crumpled areas |
| 污渍/霉斑 | EN: clean yellow stains, water marks and mold spots, restore clean paper tone |
| 破损缺失 | EN: rebuild missing corners and torn sections naturally, fill gaps with matching content |
| 模糊 | EN: sharpen the out-of-focus areas, recover fine detail, deblur the image |
| 褪色/偏色 | EN: correct faded colors and color cast, restore natural tonal balance |
| 泛黄（黑白） | EN: remove yellowing and sepia discoloration, return to clean neutral gray tones |
| 分辨率低 | EN: upscale and enhance detail, make the image crisp and clear without adding artifacts |
| 对比度弱 | EN: improve contrast and dynamic range while keeping highlights and shadows natural |

## 3. 保真约束（默认必带）

```
EN: Preserve the original composition, faces, clothing, background and historical character. Do NOT restyle, do NOT redraw faces, do NOT change backgrounds, do NOT add or remove objects, do NOT colorize, do NOT beautify or airbrush the people.
CN: 保持原图构图、人物五官、服饰与背景不变；不要重绘、不要改变人物长相、不要更换背景、不要增删物体、不要上色、不要对人像磨皮美颜。
```

**严格版**（前一轮模型自由发挥过强时使用，放在 prompt 开头）：

```
EN: This is a strict restoration task. Keep every element exactly as in the original photo — same faces, same poses, same clothes, same background. Only remove damage and improve clarity. Any creative change is forbidden.
CN: 这是严格的修复任务。所有元素必须与原图完全一致——同样的面孔、姿态、衣着、背景。只允许去除损伤、提升清晰度，禁止任何创作性改动。
```

## 4. 黑白 / 彩色说明（按需）

- 黑白照片（默认翻新不上色）：
  ```
  EN: Keep the photo black and white / monochrome, do not add color.
  ```
- 彩色照片轻微褪色（允许还原色彩）：
  ```
  EN: Restore the original faded colors naturally, keep it realistic.
  ```

## 5. 完整示例

### 示例 A：泛黄黑白合影，划痕+折痕+噪点（一次调用）

```
Restore and enhance this old photograph. Remove yellowing and sepia discoloration, return to clean neutral gray tones; erase all scratches and fold lines; remove film grain and speckles; sharpen the faces and recover fine detail. Keep the photo black and white, do not add color. Preserve the original composition, faces, clothing and background — do NOT redraw faces, do NOT change poses or background, do NOT add or remove objects. Make it look like a clean, crisp vintage photograph.
```

### 示例 B：重度破损，第一步结构修复

```
Carefully repair the damaged and torn areas of this old photo. Reconstruct the missing corners and ripped sections so the content matches the surrounding area naturally. Do not alter the intact parts. Preserve every visible face exactly as-is. This is a structural repair pass only — do not restyle or recolor.
```

### 示例 C：第二步整体增强（输入 = 示例 B 的输出）

```
Enhance this photo further: remove remaining noise and scratches, sharpen soft details, clean stains. Preserve the original faces, clothing and composition exactly. Keep it natural and vintage, do not airbrush, do not recolor.
```

### 示例 D：彩色老照片褪色偏色 + 轻度划痕

```
Restore this old color photo. Correct the faded colors and color cast, restore natural tonal balance; erase light scratches and dust. Sharpen details slightly. Preserve the original composition, faces and background. Do not restyle or over-saturate — keep the colors realistic and natural.
```

## 6. 参数速查

| 场景 | size | quality | input_fidelity |
|---|---|---|---|
| 默认修复 | 按构图 1536x1024 / 1024x1536 / 1024x1024 | high | high |
| 模型自由发挥/改写过强 | 同上 | high | high（并换严格保真模板） |
| 分步第二次精修 | 同上 | high | high |
