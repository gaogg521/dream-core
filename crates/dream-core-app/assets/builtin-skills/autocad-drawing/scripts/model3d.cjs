#!/usr/bin/env node
'use strict';
/*
 * model3d.cjs — CAD / 3D files -> a single self-contained, rotatable 3D viewer (.html).
 * Zero dependencies beyond Node 18+; works offline (three.js and OpenCascade are vendored).
 *
 *   node model3d.cjs view <file> [--out x.html] [--title T] [--lang zh|en] [--up z|y] [--quality low|medium|high]
 *   node model3d.cjs loft <lines.json|lines.dxf> [--out x.html] [--layers REGEX] [--centerline 0]
 *                         [--station-scale 1] [--samples 60] [--no-mirror]
 *                         [--mode auto|plan|sections] [--scale 1000] [--stations 140]
 *     --mode  auto (default): a DXF whose half-breadth plan has "17.0WL"-style waterline labels is read as a
 *             labelled 三视图型线图 (plan); otherwise sections are read from 3D curves / layer names.
 *     --scale drawing units per label unit (only if the height-axis numbers are missing; mm drawing, m labels = 1000)
 *     --centerline  y of the centreline (plan mode) / of the body plan (sections mode); auto-detected in plan mode
 *   node model3d.cjs read <file>          # plain-text overview of a drawing (what is in it, flat sheet or real 3D, next step)
 *   node model3d.cjs info <file>          # print what would be shown, no HTML
 *   common: --unit mm|m|in   (override a wrong $INSUNITS)   --encoding gbk|utf-8|big5   (old DXF only)
 *
 * view inputs: .dxf (3D wireframe, 3DFACE, polyface/MESH, thickness, blocks), .stl, .obj,
 *              .step/.stp, .iges/.igs, .brep. DWG: save as DXF first.
 */

const fs = require('fs');
const path = require('path');
const { Model, zFlatness } = require('./model3d_lib/model.cjs');
const { readDxfFile, walk } = require('./model3d_lib/dxf.cjs');
const { readStl, readObj, readOcct } = require('./model3d_lib/meshio.cjs');
const { loftJson, loftDxf } = require('./model3d_lib/loft.cjs');
const { readDxfReport } = require('./model3d_lib/report.cjs');

const SKILL_DIR = path.join(__dirname, '..');

function parseArgs(argv) {
  const a = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const t = argv[i];
    if (t === '--no-mirror') a.mirror = false;
    else if (t.startsWith('--')) a[t.slice(2).replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = argv[++i];
    else a._.push(t);
  }
  return a;
}

function usage(code) {
  const head = fs.readFileSync(__filename, 'utf8').split('*/')[0];
  process[code ? 'stderr' : 'stdout'].write(head.replace(/^\/\*|^ \* ?/gm, '') + '\n');
  process.exit(code);
}

function sniffDwg(file) {
  const fd = fs.openSync(file, 'r');
  const b = Buffer.alloc(6);
  fs.readSync(fd, b, 0, 6, 0);
  fs.closeSync(fd);
  return /^AC10\d\d/.test(b.toString('latin1'));
}

// Native formats of commercial CAD kernels: no public spec, no open reader. Fail fast with
// the export path instead of letting an agent spend a session reverse-engineering bytes.
const NATIVE = {
  '.sldprt': 'SolidWorks 零件', '.sldasm': 'SolidWorks 装配体', '.slddrw': 'SolidWorks 工程图',
  '.ipt': 'Inventor 零件', '.iam': 'Inventor 装配体', '.catpart': 'CATIA 零件', '.catproduct': 'CATIA 装配体',
  '.prt': 'NX / Creo 零件', '.asm': 'Creo / Solid Edge 装配体', '.par': 'Solid Edge 零件', '.psm': 'Solid Edge 钣金',
  '.x_t': 'Parasolid', '.x_b': 'Parasolid', '.f3d': 'Fusion 360', '.skp': 'SketchUp', '.3dm': 'Rhino',
  '.rvt': 'Revit', '.dgn': 'MicroStation',
};

/**
 * A drawing whose points nearly all share one Z is a flat 2D sheet, not a 3D model. Exact equality is too
 * strict: real sheets carry a few stray 3D entities (masts, ladders, one odd block), and a viewer of a
 * 580k-segment general arrangement that merely warns about ACIS solids reads as "a 3D model".
 */
function flatnessWarning(model) {
  const zs = [];
  let hx0 = Infinity, hx1 = -Infinity, hy0 = Infinity, hy1 = -Infinity, zmin = Infinity, zmax = -Infinity;
  const scan = (arr) => {
    for (let i = 0; i + 2 < arr.length; i += 3) {
      const x = arr[i], y = arr[i + 1], z = arr[i + 2];
      if (x < hx0) hx0 = x; if (x > hx1) hx1 = x; if (y < hy0) hy0 = y; if (y > hy1) hy1 = y;
      if (z < zmin) zmin = z; if (z > zmax) zmax = z;
      zs.push(z);
    }
  };
  for (const p of model.parts.values()) { scan(p.pos); scan(p.lines); }
  if (!zs.length) return;
  const f = zFlatness(zs, Math.max(hx1 - hx0, hy1 - hy0));
  const share = f.share;
  if (f.exact) {
    model.view = 'top';
    model.warn('这张 DXF 是纯平面图（所有 Z 坐标相同），没有三维信息。二维型线图请用 loft 子命令放样；二维平/立面要建模需先补充高度');
  } else if (share >= 0.9 && zs.length >= 200) {
    model.view = 'top';
    const pct = (100 * share).toFixed(1);
    const z = +f.z.toFixed(1);
    model.warn(`这张 DXF 有 ${pct}% 的点落在同一个平面（Z=${z}），三维只来自 ${(100 - 100 * share).toFixed(1)}% 的零散图元（Z 范围 ${Math.round(zmin)} ~ ${Math.round(zmax)}）：`
      + '它是平铺的二维工程图（总布置图、三视图一类，各视图并排画在同一个平面上），转出来基本是一张平的图，不是三维模型，已默认用顶视图打开。'
      + '要得到三维：用原始三维模型导出 STEP；船体可用带标注的型线图走 loft 放样');
  }
}

async function load(file, args, model) {
  const ext = path.extname(file).toLowerCase();
  if (NATIVE[ext]) {
    throw new Error(`${path.basename(file)} 是 ${NATIVE[ext]}的私有格式，没有公开规范，本工具和任何开源库都无法直接读取（不要尝试逆向解析二进制）。`
      + '请在原软件里「文件 → 另存为 / 导出」选 STEP（*.step / *.stp，首选，保留零件和颜色）或 STL，再用本工具转换；'
      + '文件是别人发来的，就请对方导出 STEP 发过来。'
      + ` / ${NATIVE[ext]} native format is proprietary: export STEP (preferred) or STL from the authoring CAD.`);
  }
  if (ext === '.dwg' || sniffDwg(file)) {
    throw new Error('DWG 是 Autodesk 私有格式，无法直接读取。请在 AutoCAD/中望/浩辰里「另存为 DXF」（版本选 2010 或更高的 ASCII DXF）后再转换；'
      + '二维型线图用 loft 子命令处理 DXF（保留图上的文字标注）；实体模型也可以用 AutoCAD 的 EXPORT 导出 STEP，或 STLOUT 导出 STL。'
      + ' / DWG is proprietary: save as ASCII DXF (2010+) and run loft for a lines plan, or export STEP/STL.');
  }
  if (ext === '.dxf') {
    const doc = readDxfFile(file, args.encoding);
    if (doc.unit) model.unit = doc.unit;
    const skipped = walk(doc, {
      onCurve: (layer, color, pts, closed) => model.addPolyline(model.part(layer, color), pts, closed),
      onFace: (layer, color, pts) => model.addPolygon(model.part(layer, color), pts),
    });
    if (skipped.acis) {
      model.warn(`有 ${skipped.acis} 个 ACIS 实体（3DSOLID/REGION/曲面）无法从 DXF 读取几何。请在 AutoCAD 里用 EXPORT 导出 STEP（或 STLOUT 导出 STL）再转换`);
    }
    const other = Object.entries(skipped).filter(([k]) => k !== 'acis');
    if (other.length) model.warn('未显示的实体类型：' + other.map(([k, v]) => `${k}×${v}`).join('、'));
    flatnessWarning(model);
    return;
  }
  if (ext === '.stl') return readStl(file, model);
  if (ext === '.obj') return readObj(file, model);
  if (['.step', '.stp', '.iges', '.igs', '.brep'].includes(ext)) return readOcct(file, model, args.quality);
  throw new Error(`不支持的格式 ${ext}；支持 dxf/stl/obj/step/stp/iges/igs/brep / unsupported format`);
}

function b64(arr, Ctor) { return Buffer.from(new Ctor(arr).buffer).toString('base64'); }

/** Recentre to keep float32 precision on survey-sized coordinates, then pack for the viewer. */
function pack(model, title, lang) {
  const min = [Infinity, Infinity, Infinity], max = [-Infinity, -Infinity, -Infinity];
  const scan = (arr) => { for (let i = 0; i < arr.length; i++) { const k = i % 3; if (arr[i] < min[k]) min[k] = arr[i]; if (arr[i] > max[k]) max[k] = arr[i]; } };
  for (const p of model.parts.values()) { scan(p.pos); scan(p.lines); }
  if (!Number.isFinite(min[0])) throw new Error('文件里没有可显示的几何 / nothing to display');
  // $INSUNITS is often left at a template default; a 5 km "building" is really millimetres.
  const span = Math.max(max[0] - min[0], max[1] - min[1], max[2] - min[2]);
  if (!model.unitFromUser && ((model.unit === 'm' && span > 5000) || (model.unit === 'km' && span > 50))) {
    model.warn(`图纸声明单位是 ${model.unit}，但最大尺寸 ${Math.round(span)} 明显偏大，实际很可能是毫米；可加 --unit mm 重新生成`);
  }
  const off = min.map((v, k) => {
    const c = (v + max[k]) / 2, span = max[k] - v;
    return Math.abs(c) > span * 10 + 1e3 ? Math.round(c) : 0;
  });
  const shift = (arr) => arr.map((v, i) => v - off[i % 3]);
  const meshes = [], lines = [];
  for (const p of model.parts.values()) {
    if (p.idx.length) meshes.push({ name: p.name, color: p.color, pos: b64(shift(p.pos), Float32Array), idx: b64(p.idx, Uint32Array) });
    if (p.lines.length) lines.push({ name: p.name, color: p.color, pos: b64(shift(p.lines), Float32Array) });
  }
  if (off.some((v) => v)) model.warn(`坐标已整体平移 (${off.join(', ')}) 以保证显示精度，尺寸不受影响`);
  return { title, unit: model.unit, up: model.up, view: model.view || 'iso', meshes, lines, warnings: model.warnings };
}

function writeHtml(out, data, lang) {
  const tpl = fs.readFileSync(path.join(SKILL_DIR, 'templates', 'viewer3d.html'), 'utf8');
  const three = fs.readFileSync(path.join(SKILL_DIR, 'vendor', 'three-viewer.iife.min.js'), 'utf8');
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  // Function replacers: the payloads contain `$` sequences that string replacers would expand.
  const html = tpl
    .replace('__THREE__', () => three)
    .replace('__MODEL__', () => JSON.stringify(data).replace(/</g, '\\u003c'))
    .replace(/__LANG__/g, () => lang)
    .replace('__TITLE__', () => esc(data.title));
  fs.mkdirSync(path.dirname(path.resolve(out)), { recursive: true });
  fs.writeFileSync(out, html);
  return html.length;
}

function report(model, out, size) {
  const s = model.stats();
  const lines = [
    `OK  ${out}`,
    `    parts=${s.parts} triangles=${s.tris} segments=${s.segs} unit=${model.unit || '-'} size=${(size / 1048576).toFixed(1)}MB`,
  ];
  for (const w of model.warnings) lines.push('    WARN ' + w);
  if (size > 30 * 1048576) lines.push('    WARN 文件超过 30MB，打开可能较慢；STEP 可加 --quality low 降低面数');
  console.log(lines.join('\n'));
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  let [cmd, input] = args._;
  if (!cmd || cmd === '-h' || cmd === '--help') usage(0);
  if (!['read', 'view', 'loft', 'info'].includes(cmd)) { input = cmd; cmd = 'view'; }
  if (!input || !fs.existsSync(input)) { console.error(`找不到输入文件 / input not found: ${input}`); process.exit(2); }

  if (cmd === 'read' && path.extname(input).toLowerCase() === '.dxf' && !sniffDwg(input)) {
    console.log(readDxfReport(input, { encoding: args.encoding }));
    return;
  }

  const model = new Model();
  if (cmd === 'read') {
    // DWG and native CAD formats throw their "how to export" guidance here; STEP/STL/OBJ get a short overview.
    await load(input, args, model);
    const min = [Infinity, Infinity, Infinity], max = [-Infinity, -Infinity, -Infinity];
    for (const p of model.parts.values()) for (const arr of [p.pos, p.lines]) for (let i = 0; i < arr.length; i++) { const k = i % 3; min[k] = Math.min(min[k], arr[i]); max[k] = Math.max(max[k], arr[i]); }
    const s = model.stats();
    console.log([`文件：${path.basename(input)}`, `内容：${s.parts} 个零件/图层、${Math.round(s.tris)} 个三角面、${Math.round(s.segs)} 条线段｜单位 ${model.unit || '未声明'}`,
      `尺寸：${[0, 1, 2].map((k) => Math.round((max[k] - min[k]) * 100) / 100).join(' × ')}`, ...model.warnings.map((w) => '注意：' + w), '', `下一步：node scripts/model3d.cjs view "${path.basename(input)}"`].join('\n'));
    return;
  }
  if (cmd === 'loft') {
    const opts = { mode: args.mode, stations: args.stations, scale: args.scale, centerline: args.centerline, encoding: args.encoding, layers: args.layers, stationScale: args.stationScale, samples: args.samples ? +args.samples : undefined, mirror: args.mirror };
    const n = path.extname(input).toLowerCase() === '.json' ? loftJson(input, model, opts) : loftDxf(input, model, opts);
    model.warn(`已用 ${n} 个站位放样；曲面由横剖线按弧长对应插值，适合查看外形，不能替代光顺后的型值`);
  } else {
    await load(input, args, model);
  }
  if (args.up) model.up = args.up === 'y' ? 'y' : 'z';
  if (args.unit) model.unit = args.unit;
  model.unitFromUser = Boolean(args.unit);
  const lang = args.lang === 'en' ? 'en' : 'zh';
  const title = args.title || path.basename(input, path.extname(input));
  const data = pack(model, title, lang);
  if (cmd === 'info') {
    const min = [Infinity, Infinity, Infinity], max = [-Infinity, -Infinity, -Infinity];
    for (const p of model.parts.values()) for (const arr of [p.pos, p.lines]) for (let i = 0; i < arr.length; i++) { const k = i % 3; min[k] = Math.min(min[k], arr[i]); max[k] = Math.max(max[k], arr[i]); }
    const parts = [...model.parts.values()].map((p) => ({ name: p.name, color: p.color, tris: p.idx.length / 3, segs: p.lines.length / 6 }));
    console.log(JSON.stringify({ ...model.stats(), unit: model.unit, up: model.up, bbox: { min, max }, parts, warnings: model.warnings }, null, 2));
    return;
  }
  const out = args.out || path.join(path.dirname(input), path.basename(input, path.extname(input)) + '.3d.html');
  report(model, out, writeHtml(out, data, lang));
}

main().catch((e) => { console.error('ERROR ' + (e && e.message ? e.message : e)); process.exit(1); });
