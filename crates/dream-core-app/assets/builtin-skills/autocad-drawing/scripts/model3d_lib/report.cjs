'use strict';
// `read`: a plain-text overview of a DXF drawing, for "阅读一下这个图纸" / "看看这张图" / "分析一下这个图".
// Zero dependencies and one pass, so an agent never needs to pip-install a DXF library just to look.

const fs = require('fs');
const path = require('path');
const { readDxfFile, walk } = require('./dxf.cjs');
const { zFlatness } = require('./model.cjs');
const { planToStations } = require('./lines_plan.cjs');

const VERSIONS = { AC1006: 'R10', AC1009: 'R11/R12', AC1012: 'R13', AC1014: 'R14', AC1015: '2000', AC1018: '2004', AC1021: '2007', AC1024: '2010', AC1027: '2013', AC1032: '2018+' };
const ACIS = new Set(['3DSOLID', 'BODY', 'REGION', 'SURFACE', 'PLANESURFACE', 'EXTRUDEDSURFACE', 'LOFTEDSURFACE', 'REVOLVEDSURFACE', 'SWEPTSURFACE', 'NURBSURFACE']);

const isPaperSpace = (e) => e.g.some(([c, v]) => c === 67 && v.trim() === '1');
const nf = (n) => Math.round(n).toLocaleString('en-US');
const fmt = (v) => (Math.abs(v) < 1e-6 ? '0' : Math.abs(v) >= 1000 ? nf(v) : String(+v.toPrecision(4))); // 4.9e-14 is float noise, not a height

function readDxfReport(file, opts = {}) {
  const doc = readDxfFile(file, opts.encoding);
  const out = [];
  const say = (s = '') => out.push(s);
  const stat = fs.statSync(file);

  // ---- file
  const gen = doc.comments.length ? doc.comments.join(' / ') : '（未署名，多半是 AutoCAD 等原生软件保存）';
  say(`文件：${path.basename(file)}（${(stat.size / 1048576).toFixed(1)} MB）`);
  say(`格式：ASCII DXF ${VERSIONS[doc.version] || doc.version || '?'}（${doc.version || '无版本号'}）｜代码页 ${doc.codepage || '-'}｜单位 ${doc.unit || '未声明'}｜生成：${gen}`);

  // ---- entities (model space only: paper-space layouts only hold viewports)
  const census = {};
  for (const e of doc.entities) if (!isPaperSpace(e)) census[e.type] = (census[e.type] || 0) + 1;
  const userBlocks = [...doc.blocks.keys()].filter((n) => !n.startsWith('*'));
  say(`实体：模型空间 ${nf(Object.values(census).reduce((a, b) => a + b, 0))} 个（${Object.entries(census).sort((a, b) => b[1] - a[1]).slice(0, 10).map(([k, v]) => `${k}×${nf(v)}`).join('、')}）｜块定义 ${userBlocks.length} 个｜图层表 ${doc.layers.size} 个`);

  // ---- geometry: one walk collects everything
  const layerStat = new Map();
  const bump = (layer, key, n) => { let s = layerStat.get(layer); if (!s) layerStat.set(layer, (s = { segs: 0, tris: 0 })); s[key] += n; };
  const zs = [], texts = [], curves = [];
  let x0 = Infinity, x1 = -Infinity, y0 = Infinity, y1 = -Infinity;
  const take = (p) => { if (p[0] < x0) x0 = p[0]; if (p[0] > x1) x1 = p[0]; if (p[1] < y0) y0 = p[1]; if (p[1] > y1) y1 = p[1]; zs.push(p[2]); };
  const skipped = walk(doc, {
    onCurve(layer, color, pts) { bump(layer, 'segs', Math.max(0, pts.length - 1)); for (const p of pts) take(p); curves.push({ layer, pts }); },
    onFace(layer, color, pts) { bump(layer, 'tris', Math.max(1, pts.length - 2)); for (const p of pts) take(p); },
    onText(layer, text, p) { if (text) texts.push({ layer, text, x: p[0], y: p[1] }); },
  });
  const segs = [...layerStat.values()].reduce((a, s) => a + s.segs, 0), tris = [...layerStat.values()].reduce((a, s) => a + s.tris, 0);
  if (!zs.length) { say('几何：没有可显示的线或面（可能只有文字、标注或读不出的实体）。'); return out.join('\n'); }
  const f = zFlatness(zs, Math.max(x1 - x0, y1 - y0));
  say(`几何：${nf(segs)} 条线段、${nf(tris)} 个三角面，用到 ${layerStat.size} 个图层｜范围 X ${fmt(x0)} ~ ${fmt(x1)}（${fmt(x1 - x0)}）、Y ${fmt(y0)} ~ ${fmt(y1)}（${fmt(y1 - y0)}）、Z ${fmt(f.zmin)} ~ ${fmt(f.zmax)}（${fmt(f.zmax - f.zmin)}）`);

  // ---- ACIS solids: present, and do they carry geometry? (converters such as LibreDWG write empty shells)
  let acis = 0, acisData = 0;
  const scanAcis = (ents) => { for (const e of ents) if (ACIS.has(e.type)) { acis++; if (e.g.some(([c, v]) => (c === 1 || c === 3) && v.trim())) acisData++; } };
  scanAcis(doc.entities);
  for (const b of doc.blocks.values()) scanAcis(b.ents);
  if (acis) say(`实体（ACIS）：${nf(acis)} 个 3DSOLID/REGION/BODY，其中带几何数据的 ${nf(acisData)} 个${acisData === 0 ? '——全是空壳，没有可还原的实体' : '（DXF 里读不出，需导出 STEP）'}`);

  // ---- layers
  const rows = [...layerStat.entries()].sort((a, b) => (b[1].segs + b[1].tris * 3) - (a[1].segs + a[1].tris * 3));
  say();
  say(`图层（按几何量，前 ${Math.min(12, rows.length)} / 共 ${rows.length}）：`);
  for (const [name, s] of rows.slice(0, 12)) say(`  ${name.padEnd(26)} 线段 ${nf(s.segs).padStart(8)}  面 ${nf(s.tris).padStart(6)}`);

  // ---- text (title block, principal dimensions, labels)
  const seen = new Set(), picked = [];
  for (const t of texts.slice().sort((a, b) => b.text.length - a.text.length)) {
    if (t.text.length < 4 || seen.has(t.text)) continue;
    seen.add(t.text); picked.push(t.text.length > 40 ? t.text.slice(0, 40) + '…' : t.text);
    if (picked.length >= 30) break;
  }
  say();
  say(`文字：${nf(texts.length)} 条（不重复 ${new Set(texts.map((t) => t.text)).size} 条）${picked.length ? '；较长的几条：' : ''}`);
  if (picked.length) say('  ' + picked.join(' ｜ '));

  // ---- verdict
  say();
  say('结论：');
  let plan = null;
  try { plan = planToStations(curves, texts, {}); } catch { /* an unusable plan is reported by loft, not here */ }
  const flat = f.exact || (f.share >= 0.9 && f.n >= 200);
  if (plan) {
    say(`  - 这是带水线标注的型线图：识别出 ${plan.info.waterlines} 条水线、船长约 ${fmt(plan.info.length)}。可用 loft 放样成 3D 船体（只适合看外形，不是型值表）。`);
  } else if (flat) {
    say(`  - 这是平铺的二维工程图：${(100 * f.share).toFixed(1)}% 的点在同一个平面（Z=${+f.z.toFixed(1)}）${f.exact ? '，没有任何三维信息' : `，三维只来自零散图元（Z ${fmt(f.zmin)} ~ ${fmt(f.zmax)}）`}。总布置图、三视图一类各视图并排画在一个平面上，无法自动合成三维。`);
  } else {
    say(`  - 有真正的三维几何（Z 跨度 ${fmt(f.zmax - f.zmin)}，最集中的平面也只占 ${(100 * f.share).toFixed(1)}%），可以用 view 转成可旋转的 3D 模型。`);
  }
  if (acis && acisData === 0) say('  - 文件里的 ACIS 实体没有几何数据，常见于 LibreDWG 等工具转出的 DXF；要保留实体，请用 AutoCAD/中望等原生软件另存 DXF，或直接导出 STEP。');
  const other = Object.entries(skipped).filter(([k]) => k !== 'acis');
  if (other.length) say('  - 未读取的实体类型：' + other.map(([k, v]) => `${k}×${v}`).join('、'));
  say();
  say('下一步（都不需要装任何包）：');
  const q = JSON.stringify(path.basename(file));
  if (plan) say(`  node scripts/model3d.cjs loft ${q}`);
  else if (!flat) say(`  node scripts/model3d.cjs view ${q}`);
  else say(`  node scripts/model3d.cjs view ${q}    （按顶视图打开，可缩放、按图层显隐）`);
  say(`  node scripts/model3d.cjs info ${q}    （机器可读的 JSON：图层、面数、包围盒、警告）`);
  return out.join('\n');
}

module.exports = { readDxfReport };
