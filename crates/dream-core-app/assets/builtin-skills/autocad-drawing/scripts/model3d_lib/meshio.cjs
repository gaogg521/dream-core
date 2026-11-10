'use strict';
// STL / OBJ readers (pure JS) and STEP / IGES / BREP via the vendored occt-import-js (WASM).

const fs = require('fs');
const path = require('path');
const { rgbHex } = require('./model.cjs');

function readStl(file, model) {
  const buf = fs.readFileSync(file);
  const part = model.part(path.basename(file, path.extname(file)), '#b8c4d0');
  const nBin = buf.length >= 84 ? buf.readUInt32LE(80) : -1;
  if (nBin >= 0 && 84 + nBin * 50 === buf.length) {
    for (let i = 0; i < nBin; i++) {
      const o = 84 + i * 50 + 12;
      const base = part.pos.length / 3;
      for (let v = 0; v < 3; v++) part.pos.push(buf.readFloatLE(o + v * 12), buf.readFloatLE(o + v * 12 + 4), buf.readFloatLE(o + v * 12 + 8));
      part.idx.push(base, base + 1, base + 2);
    }
    return;
  }
  const re = /vertex\s+(\S+)\s+(\S+)\s+(\S+)/g;
  const text = buf.toString('latin1');
  let m, k = 0;
  while ((m = re.exec(text))) { part.pos.push(+m[1], +m[2], +m[3]); part.idx.push(k++); }
  if (!k) throw new Error('STL 文件里没有三角面 / no facets found in STL');
}

function readObj(file, model) {
  const text = fs.readFileSync(file, 'utf8');
  const verts = [];
  const fallback = path.basename(file, path.extname(file));
  let part = null; // created lazily so an `o` before the first face leaves no empty part
  const cur = () => part || (part = model.part(fallback, '#b8c4d0'));
  const map = new Map(); // per part: global vertex index -> local index
  const local = (p, gi) => {
    const key = p.name + '#' + gi;
    let li = map.get(key);
    if (li === undefined) { const v = verts[gi]; li = p.pos.length / 3; p.pos.push(v[0], v[1], v[2]); map.set(key, li); }
    return li;
  };
  const resolve = (tok) => { const i = parseInt(tok.split('/')[0], 10); return i < 0 ? verts.length + i : i - 1; };
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line[0] === '#') continue;
    const t = line.split(/\s+/);
    if (t[0] === 'v') verts.push([+t[1], +t[2], +t[3]]);
    else if (t[0] === 'o' || t[0] === 'g') { if (t[1]) part = model.part(t.slice(1).join(' '), '#b8c4d0'); }
    else if (t[0] === 'f') {
      const p = cur();
      const ids = t.slice(1).map(resolve).filter((i) => i >= 0 && i < verts.length).map((gi) => local(p, gi));
      for (let i = 1; i + 1 < ids.length; i++) p.idx.push(ids[0], ids[i], ids[i + 1]);
    } else if (t[0] === 'l') {
      const ids = t.slice(1).map(resolve).filter((i) => i >= 0 && i < verts.length);
      const p = cur();
      for (let i = 0; i + 1 < ids.length; i++) p.lines.push(...verts[ids[i]], ...verts[ids[i + 1]]);
    }
  }
  model.up = 'y'; // OBJ exporters (Blender, 3ds Max, SketchUp) are Y-up by convention
}

const QUALITY = { low: 0.004, medium: 0.0015, high: 0.0005 };

async function readOcct(file, model, quality) {
  const occtimportjs = require(path.join(__dirname, '..', '..', 'vendor', 'occt', 'occt-import-js.js'));
  const occt = await occtimportjs();
  const content = new Uint8Array(fs.readFileSync(file));
  const ext = path.extname(file).toLowerCase();
  const params = { linearUnit: 'millimeter', linearDeflectionType: 'bounding_box_ratio', linearDeflection: QUALITY[quality] || QUALITY.medium, angularDeflection: 0.5 };
  const res = ext === '.brep' ? occt.ReadBrepFile(content, params)
    : (ext === '.igs' || ext === '.iges') ? occt.ReadIgesFile(content, params)
      : occt.ReadStepFile(content, params);
  if (!res || !res.success) throw new Error('OpenCascade 无法读取该文件（可能损坏或不是标准 STEP/IGES）/ OpenCascade failed to read the file');
  const names = new Map();
  res.meshes.forEach((m, i) => {
    // Pure black is how many exporters say "no colour"; it would render as a silhouette.
    const dark = !m.color || m.color[0] + m.color[1] + m.color[2] < 0.05;
    const c = dark ? '#b8c4d0' : rgbHex(m.color[0] * 255, m.color[1] * 255, m.color[2] * 255);
    let name = m.name || ('part-' + (i + 1));
    const seen = names.get(name) || 0; names.set(name, seen + 1);
    if (seen) name += ' #' + (seen + 1);
    const part = model.part(name, c);
    const base = part.pos.length / 3;
    for (const v of m.attributes.position.array) part.pos.push(v);
    for (const ix of m.index.array) part.idx.push(base + ix);
  });
  if (!res.meshes.length) model.warn('文件里没有可显示的实体 / no solids found');
}

module.exports = { readStl, readObj, readOcct };
