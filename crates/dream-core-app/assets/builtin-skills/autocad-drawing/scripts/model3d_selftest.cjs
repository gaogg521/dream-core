#!/usr/bin/env node
'use strict';
// Self-test for the labelled lines-plan recogniser. No fixtures, no dependencies:
//   node scripts/model3d_selftest.cjs
// Builds a synthetic DXF (analytic hull half-breadth plan + an unrelated second view + a sheet frame),
// then checks the recovered sections against the analytic hull for several drawing conventions.

const fs = require('fs');
const os = require('os');
const path = require('path');
const { readDxfFile, walk } = require('./model3d_lib/dxf.cjs');
const { planToStations, WL_RE } = require('./model3d_lib/lines_plan.cjs');

const L = 100000, B2 = 8000, HEIGHTS = [0.5, 1, 2, 3, 4, 6, 8, 10, 12];
// z in metres, result in mm. Test tolerance (12 mm) is the chord error of the 80-segment polylines.
const half = (x, z) => B2 * Math.max(0, 1 - Math.pow((2 * x) / L - 1, 4)) * Math.min(1, Math.pow(z / 12, 0.4));

function dxf(entities) {
  return ['0', 'SECTION', '2', 'HEADER', '9', '$INSUNITS', '70', '4', '0', 'ENDSEC', '0', 'SECTION', '2', 'ENTITIES', ...entities.flat(), '0', 'ENDSEC', '0', 'EOF'].join('\n');
}
const lwpoly = (layer, pts, closed) => ['0', 'LWPOLYLINE', '8', layer, '90', String(pts.length), '70', closed ? '1' : '0', ...pts.flatMap(([x, y]) => ['10', String(x), '20', String(y)])];
const text = (layer, s, x, y, mt) => (mt
  ? ['0', 'MTEXT', '8', layer, '10', String(x), '20', String(y), '30', '0', '40', '250', '1', s]
  : ['0', 'TEXT', '8', layer, '10', String(x), '20', String(y), '30', '0', '40', '250', '1', s]);

/** opts: flip (centreline on top), offset (label anchor offset on the right column), mtext, shift */
function build(o = {}) {
  const ent = [];
  const Y0 = (o.shift || 0);
  const ty = (v) => (o.flip ? Y0 + B2 * 1.0 - v : Y0 + v); // map half-breadth/height value to drawing y
  // frame
  ent.push(lwpoly('frame', [[-9000 + (o.shift || 0), -9000], [120000 + (o.shift || 0), -9000], [120000 + (o.shift || 0), 60000], [-9000 + (o.shift || 0), 60000]], true));
  const X = (x) => x + (o.shift || 0);
  // waterlines, 80 points each, labelled at the stern end
  HEIGHTS.forEach((z) => {
    const pts = [];
    for (let i = 0; i <= 80; i++) { const x = (L * i) / 80; pts.push([X(x), ty(half(x, z))]); }
    ent.push(lwpoly('wl', pts, false));
    ent.push(text('t', `${z.toFixed(1)}WL`, X(6000), ty(half(6000, z)) + (o.flip ? -80 : 80), o.mtext));
  });
  // height axis: label value v (metres) sits at drawing y of v*1000 on both margins
  for (let v = 0; v <= 8; v++) {
    ent.push(text('t', v.toFixed(1), X(-1500), ty(v * 1000) - 5, false));
    ent.push(text('t', v.toFixed(1), X(L + 800), ty(v * 1000) + (o.offset ? -125 : -5), false));
  }
  // a second, unrelated view far above (a few dense curves) and a short fragment inside the plan
  for (let k = 0; k < 6; k++) {
    const pts = []; for (let i = 0; i <= 30; i++) pts.push([X(20000 + i * 1200), 30000 + k * 900 + 400 * Math.sin(i / 4)]);
    ent.push(lwpoly('other', pts, false));
  }
  return dxf(ent);
}

function recognise(src) {
  const f = path.join(os.tmpdir(), `lp-${process.pid}-${Math.random().toString(36).slice(2)}.dxf`);
  fs.writeFileSync(f, src);
  try {
    const doc = readDxfFile(f);
    const curves = [], texts = [];
    walk(doc, { onCurve: (layer, c, pts) => curves.push({ layer, pts }), onFace() {}, onText: (layer, t, p) => texts.push({ layer, text: t, x: p[0], y: p[1] }) });
    return planToStations(curves, texts, {});
  } finally { fs.unlinkSync(f); }
}

if (process.env.DUMP) { fs.writeFileSync(process.env.DUMP, build({})); process.exit(0); } // write the plain synthetic DXF for inspection

let failed = 0;
function check(name, cond, detail) { if (!cond) { failed++; console.log('FAIL', name, detail || ''); } else console.log('ok  ', name, detail || ''); }

for (const [name, o] of [['plain', {}], ['flipped centreline', { flip: true }], ['right-label offset', { offset: true }], ['MTEXT labels', { mtext: true }], ['translated', { shift: 250000 }]]) {
  const plan = recognise(build(o));
  if (!plan) { check(name, false, 'not recognised'); continue; }
  const shift = o.shift || 0;
  let err = 0, n = 0;
  for (const s of plan.stations) {
    for (const [y, z] of s.points) {
      if (y === 0) continue; // keel closure point
      const x = s.x - shift;
      err = Math.max(err, Math.abs(y - half(x, z / 1000)));
      n++;
    }
  }
  check(name, plan.info.waterlines === HEIGHTS.length && plan.info.scale === 1000 && err < 12, `waterlines ${plan.info.waterlines}/${HEIGHTS.length}, scale ${plan.info.scale}, max err ${err.toFixed(3)} mm over ${n} points`);
}

// Label spellings seen in real drawings: plain, bracketed with a prefix, and look-alikes that must NOT match.
for (const [s, want] of [['17.0WL', 17], ['0.25WL', 0.25], ['WL TOP(20.0WL)', 20], ['WL TO (20.0WL)', 20], ['10.7 WL', 10.7], ['14.0BL', null], ['BOTTOM TANGENTIAL LINE', null], ['DRAFT (MLD)', null], ['ST9.75', null], ['3WL extra', null]]) {
  const m = WL_RE.exec(s);
  check('label ' + JSON.stringify(s), (m ? parseFloat(m[1]) : null) === want);
}

// A drawing with no waterline labels must be declined, not guessed.
check('no labels -> declined', recognise(build({}).replace(/WL/g, 'XX')) === null);
// Labels but no height axis must ask for --scale instead of inventing one.
let msg = '';
try { recognise(build({}).split('\n').join('\n').replace(/\n1\n(\d\.0)\n/g, '\n1\nax$1\n')); } catch (e) { msg = e.message; }
check('no axis numbers -> asks for --scale', /--scale/.test(msg), msg.slice(0, 40));

// Flat multi-view sheets (a general arrangement drawing) must be called out as 2D, even with a few stray 3D
// entities; a real 3D wireframe must not be flagged. Run through the CLI, as a user would.
{
  const { execFileSync } = require('child_process');
  const line = (a, b) => ['0', 'LINE', '8', 'L', '10', String(a[0]), '20', String(a[1]), '30', String(a[2]), '11', String(b[0]), '21', String(b[1]), '31', String(b[2])];
  const infoWarnings = (entities) => {
    const f = path.join(os.tmpdir(), `flat-${process.pid}-${Math.random().toString(36).slice(2)}.dxf`);
    fs.writeFileSync(f, dxf(entities));
    try { return JSON.parse(execFileSync(process.execPath, [path.join(__dirname, 'model3d.cjs'), 'info', f], { encoding: 'utf8', maxBuffer: 1 << 26 })).warnings; } finally { fs.unlinkSync(f); }
  };
  const sheet = [];
  for (let i = 0; i < 400; i++) sheet.push(line([i * 10, 0, 0], [i * 10 + 5, 80, 0])); // 800 points, all z = 0
  for (let i = 0; i < 6; i++) sheet.push(line([i * 100, 0, 0], [i * 100, 0, 3000])); // a few stray vertical lines
  check('flat sheet with stray 3D lines -> flagged as 2D', infoWarnings(sheet).some((w) => /同一个平面/.test(w)));
  check('exactly flat drawing -> flagged as 2D', infoWarnings(sheet.slice(0, 400)).some((w) => /纯平面图/.test(w)));
  const wire = [];
  for (let i = 0; i < 120; i++) wire.push(line([i * 10, 0, 0], [i * 10, 50, (i % 20) * 40 + 40])); // genuinely spread over z
  check('real 3D wireframe -> not flagged', !infoWarnings(wire).some((w) => /平面/.test(w)));

  // `read` is what an agent runs for "阅读一下这个图纸": it must answer flat-vs-3D itself, offline, with no package.
  const readText = (entities, ...extra) => {
    const f = path.join(os.tmpdir(), `read-${process.pid}-${Math.random().toString(36).slice(2)}.dxf`);
    fs.writeFileSync(f, dxf(entities));
    try { return execFileSync(process.execPath, [path.join(__dirname, 'model3d.cjs'), 'read', f, ...extra], { encoding: 'utf8', maxBuffer: 1 << 26 }); } finally { fs.unlinkSync(f); }
  };
  // `read --texts` lists the drawing's own words, correctly decoded, so nobody has to hand-roll a DXF text parser.
  const noted = [...sheet.slice(0, 5), text('t', '总布置图 CARGO HATCH 16800 x 19800', 10, 500, false), text('t', 'plain note', 10, 100, true), text('t', 'plain note', 20, 100, true)];
  const allTexts = readText(noted, '--texts'), grepped = readText(noted, '--texts', '--grep', '总布置');
  check('read --texts: lists Chinese text intact, top to bottom', allTexts.indexOf('总布置图 CARGO HATCH 16800 x 19800') > 0 && allTexts.indexOf('总布置图') < allTexts.indexOf('plain note'));
  check('read --texts: merges repeats and honours --grep', /2x/.test(allTexts) && grepped.includes('总布置图') && !grepped.includes('plain note'));
  const flatRead = readText(sheet), wireRead = readText(wire);
  check('read: flat sheet -> says 平铺的二维工程图 and does not offer a 3D view', /平铺的二维工程图/.test(flatRead) && /Z=0/.test(flatRead));
  check('read: 3D wireframe -> says 真正的三维几何 and points to view', /真正的三维几何/.test(wireRead) && /model3d\.cjs view/.test(wireRead));
  check('read: reports entity census and layers', /LINE×120/.test(wireRead) && /图层/.test(wireRead));
  check('read: never tells the user to install anything', !/pip install|npm install/.test(flatRead + wireRead));
  check('read: a flat sheet is pointed at preview (an image), not at the WebGL viewer', /model3d\.cjs preview/.test(flatRead) && !/model3d\.cjs view/.test(flatRead));

  // `preview` must hand back a real PNG an agent can look at, written to the working directory, with the box printed
  // so that a `--box` zoom is a single follow-up step. Decode it: signature, size, ink where the line is, white elsewhere.
  {
    const zlib = require('zlib');
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pv-'));
    const src = path.join(dir, 'in.dxf');
    fs.writeFileSync(src, dxf([line([0, 0, 0], [1000, 0, 0]), line([0, 0, 0], [0, 500, 0]), line([1000, 0, 0], [1000, 500, 0]), line([0, 500, 0], [1000, 500, 0])]));
    const run = (...a) => execFileSync(process.execPath, [path.join(__dirname, 'model3d.cjs'), 'preview', src, ...a], { cwd: dir, encoding: 'utf8' });
    const say = run('--width', '400');
    const pngPath = path.join(dir, 'in.preview.png');
    const png = fs.existsSync(pngPath) ? fs.readFileSync(pngPath) : Buffer.alloc(0);
    check('preview: writes <name>.preview.png into the working directory', png.length > 100 && png.slice(1, 4).toString() === 'PNG');
    if (png.length > 100) {
      const w = png.readUInt32BE(16), h = png.readUInt32BE(20);
      const idat = [];
      for (let o = 8; o < png.length;) { const len = png.readUInt32BE(o), type = png.slice(o + 4, o + 8).toString(); if (type === 'IDAT') idat.push(png.slice(o + 8, o + 8 + len)); o += 12 + len; }
      const raw = zlib.inflateSync(Buffer.concat(idat)), stride = w * 3 + 1;
      const px = (x, y) => [raw[y * stride + 1 + x * 3], raw[y * stride + 2 + x * 3], raw[y * stride + 3 + x * 3]];
      let ink = 0;
      for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) if (px(x, y)[0] < 120) ink++;
      check('preview: 2:1 drawing -> 400 px wide, ~200 px tall, inked along the outline', w === 400 && h >= 190 && h <= 210 && ink > 800 && ink < 4000, `${w}x${h}, ${ink} dark px`);
      check('preview: the empty middle of the rectangle stays blank', px(Math.round(w / 2), Math.round(h / 2))[0] > 200);
    }
    // Agents cd into the skill directory before running the script. In a workspace that directory is a link into the
    // user's real skill library, so output must NOT land there: it goes to the workspace that owns the link.
    {
      const ws = fs.mkdtempSync(path.join(os.tmpdir(), 'ws-'));
      fs.mkdirSync(path.join(ws, '.dream', 'skills'), { recursive: true });
      const link = path.join(ws, '.dream', 'skills', 'autocad-drawing');
      fs.symlinkSync(path.join(__dirname, '..'), link, 'junction');
      execFileSync(process.execPath, [path.join('scripts', 'model3d.cjs'), 'preview', src, '--width', '300'], { cwd: link, encoding: 'utf8' });
      check('preview: run from <workspace>/.dream/skills/<skill> writes to the workspace', fs.existsSync(path.join(ws, 'in.preview.png')));
      check('preview: ...and never into the (linked) skill directory', !fs.existsSync(path.join(__dirname, '..', 'in.preview.png')));
      fs.rmSync(path.join(__dirname, '..', 'in.preview.png'), { force: true });
      fs.unlinkSync(link);
      fs.rmSync(ws, { recursive: true, force: true });
    }
    check('preview: prints box, scale and the zoom hint', /图纸范围 x -?\d+ ~ -?\d+/.test(say) && /1 像素 = /.test(say) && /--box x0,y0,x1,y1/.test(say));
    const zoom = run('--box', '0,0,500,250', '--width', '300', '--out', path.join(dir, 'z.png'));
    check('preview: --box zooms to the window', /x 0 ~ 500，y 0 ~ 250/.test(zoom) && fs.existsSync(path.join(dir, 'z.png')));
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

console.log(failed ? `\n${failed} FAILED` : '\nall passed');
process.exit(failed ? 1 : 0);
