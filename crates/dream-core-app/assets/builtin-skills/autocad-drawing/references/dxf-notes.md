# ezdxf 生成 DXF 说明与 JSON 规格

本技能用 Python 的 `ezdxf` 库生成 DXF 文件。DXF 是文本/二进制交换格式，AutoCAD 可直接打开并另存为 DWG。

## 依赖安装（隔离环境）

```bash
PY="C:/Users/Administrator/.dream/binaries/python/versions/3.13.12/python.exe"
"$PY" -m venv "C:/Users/Administrator/.dream/binaries/python/envs/default"
.venv/Scripts/pip.exe install ezdxf
```

运行：`".../envs/default/Scripts/python.exe" scripts/gen_dxf.py ...`

## 命令总览

```bash
python gen_dxf.py blank --size A3 --out blank_a3.dxf
python gen_dxf.py spec --json drawing.json --out result.dxf
python gen_dxf.py plate --w 100 --h 60 --holes "20,20;80,40" --out plate.dxf
python gen_dxf.py floorplan --json floorplan.json --out plan.dxf
```

## JSON 规格（`spec` 子命令）

顶层字段：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| size | string | 否 | 图幅 A0~A4，默认 A4 |
| landscape | bool | 否 | 横放，默认 true |
| title_block | bool | 否 | 是否画图框标题栏，默认 true |
| entities | array | 是 | 实体列表 |

实体类型与字段：

| type | 字段 | 说明 |
|------|------|------|
| line | start,end(layer) | 线段 |
| circle | center,radius(,centerline) | 圆；centerline=true 自动加点划线十字中心线 |
| arc | center,radius,start_angle,end_angle | 圆弧（角度制） |
| rect | corner,width,height | 矩形 |
| polyline | points(,closed) | 多段线 |
| text | insert,text,height(,rotation) | 文字 |
| dim | p1,p2,offset(,text) | 水平/垂直线性标注 |
| block_ref | name,insert(,scale) | 引用图块（高级） |

所有实体均可带 `layer`（默认「粗实线」）与 `color`（默认随层）。

### 示例（机械矩形板 + 中心线 + 标注）

```json
{
  "size": "A4",
  "landscape": true,
  "entities": [
    {"type": "rect", "layer": "粗实线", "corner": [0, 0], "width": 100, "height": 60},
    {"type": "circle", "layer": "粗实线", "center": [20, 30], "radius": 8, "centerline": true},
    {"type": "circle", "layer": "粗实线", "center": [80, 30], "radius": 8, "centerline": true},
    {"type": "dim", "layer": "标注", "p1": [0, 0], "p2": [100, 0], "offset": 10},
    {"type": "text", "layer": "文字", "insert": [50, 45], "text": "底板", "height": 5}
  ]
}
```

## 楼层平面 JSON（`floorplan` 子命令）

```json
{
  "size": "A3",
  "scale": 100,
  "wall_thickness": 240,
  "rooms": [
    {"name": "客厅", "x": 0, "y": 0, "w": 4800, "h": 4200},
    {"name": "卧室", "x": 4800, "y": 0, "w": 3600, "h": 4200}
  ],
  "doors": [{"x": 2400, "y": 0, "horizontal": true, "width": 900}]
}
```

说明：`scale=100` 表示 1:100，房间尺寸单位为 mm（真实尺寸），出图按比例缩放标注。

## 关键实现要点

- 用 `ezdxf.new(dxfversion="R2018", setup=True)` 新建文档，`doc.units = units.MM`。
- 线型需显式定义：`DASHED`、`CENTER`、`PHANTOM`（setup=True 不保证自带全部）。
- 中文文字：`st = doc.styles.add("GB", font="gbeitc.shx"); st.dxf.bigfont = "gbcbig.shx"`。
- 保存：`doc.saveas(out)`，文件名避免纯中文路径问题（Windows 下建议中文名前加安全处理）。
- 生成后可用 `ezdxf.recover.readfile` 校验，或直接交付。
