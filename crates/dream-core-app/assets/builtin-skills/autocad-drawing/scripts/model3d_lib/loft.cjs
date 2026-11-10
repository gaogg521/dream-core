'use strict';
// Lines plan (型线图) -> hull surface: chain each station's section curve, resample by
// normalized arc length, and skin consecutive stations into a quad grid (mirrored to port).

const fs = require('fs');
const { readDxfFile, walk } = require('./dxf.cjs');
const { planToStations } = require('./lines_plan.cjs');

function dist(a, b) { return Math.hypot(a[0] - b[0], a[1] - b[1], (a[2] || 0) - (b[2] || 0)); }

/** Join polylines whose end points touch (within tol) into as few chains as possible. */
function chain(polys, tol) {
  const pool = polys.filter((p) => p.length >= 2).map((p) => p.slice());
  if (!pool.length) return [];
  pool.sort((a, b) => Math.min(a[0][1], a[a.length - 1][1]) - Math.min(b[0][1], b[b.length - 1][1]));
  const out = [];
  while (pool.length) {
    let cur = pool.shift();
    let grew = true;
    while (grew && pool.length) {
      grew = false;
      let best = -1, bestD = Infinity, mode = 0;
      pool.forEach((p, i) => {
        const cands = [[dist(cur[cur.length - 1], p[0]), 0], [dist(cur[cur.length - 1], p[p.length - 1]), 1], [dist(cur[0], p[p.length - 1]), 2], [dist(cur[0], p[0]), 3]];
        for (const [d, m] of cands) if (d < bestD) { bestD = d; best = i; mode = m; }
      });
      if (best >= 0 && bestD <= tol) {
        const p = pool.splice(best, 1)[0];
        if (mode === 0) cur = cur.concat(p.slice(1));
        else if (mode === 1) cur = cur.concat(p.slice().reverse().slice(1));
        else if (mode === 2) cur = p.concat(cur.slice(1));
        else cur = p.slice().reverse().concat(cur.slice(1));
        grew = true;
      }
    }
    out.push(cur);
  }
  return out;
}

function arcLen(pts) { let s = 0; for (let i = 1; i < pts.length; i++) s += dist(pts[i - 1], pts[i]); return s; }

function resample(pts, n) {
  const total = arcLen(pts);
  if (total === 0) return Array.from({ length: n }, () => pts[0].slice());
  const out = [pts[0].slice()];
  let seg = 1, acc = 0;
  for (let k = 1; k < n - 1; k++) {
    const target = total * k / (n - 1);
    while (seg < pts.length - 1 && acc + dist(pts[seg - 1], pts[seg]) < target) { acc += dist(pts[seg - 1], pts[seg]); seg++; }
    const a = pts[seg - 1], b = pts[seg], l = dist(a, b) || 1, t = (target - acc) / l;
    out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
  }
  out.push(pts[pts.length - 1].slice());
  return out;
}

/** Stations: [{x, curves: [[[y,z],...], ...]}] -> model parts. */
function buildHull(model, stations, opts) {
  const n = opts.samples || 60;
  const mirror = opts.mirror !== false;
  const all = stations.flatMap((s) => s.curves.flat());
  if (!all.length) throw new Error('没有找到任何横剖线 / no section curves found');
  let ymin = Infinity, ymax = -Infinity, zmin = Infinity, zmax = -Infinity;
  for (const p of all) { ymin = Math.min(ymin, p[0]); ymax = Math.max(ymax, p[0]); zmin = Math.min(zmin, p[1]); zmax = Math.max(zmax, p[1]); }
  const tol = opts.tol || Math.hypot(ymax - ymin, zmax - zmin) * 0.01;

  const secs = [];
  for (const s of stations.slice().sort((a, b) => a.x - b.x)) {
    const chains = chain(s.curves, tol);
    if (!chains.length) continue;
    if (chains.length > 1) model.warn(`站位 x=${s.x} 的横剖线断成 ${chains.length} 段，已按最长一段放样，请检查是否有缺口`);
    let c = chains.sort((a, b) => arcLen(b) - arcLen(a))[0];
    // Orient keel -> deck: start at the lower end (ties: the end nearer the centreline).
    const a = c[0], b = c[c.length - 1];
    if (b[1] < a[1] - tol || (Math.abs(b[1] - a[1]) <= tol && b[0] < a[0])) c = c.slice().reverse();
    secs.push({ x: s.x, pts: resample(c, n) });
  }
  if (secs.length < 2) throw new Error('至少需要 2 个站位的横剖线才能放样 / need at least 2 stations');

  const sides = mirror ? [1, -1] : [1];
  const hull = model.part(opts.name || '船体曲面 Hull', '#c9d3de');
  const lines = model.part('横剖线 Sections', '#d9480f');
  const longi = model.part('纵向参考线 Long. lines', '#1c7ed6');
  for (const sgn of sides) {
    const base = hull.pos.length / 3;
    for (const s of secs) for (const p of s.pts) hull.pos.push(s.x, sgn * p[0], p[1]);
    for (let i = 0; i + 1 < secs.length; i++) for (let j = 0; j + 1 < n; j++) {
      const a = base + i * n + j, b = a + 1, c = a + n, d = c + 1;
      if (sgn > 0) hull.idx.push(a, c, d, a, d, b); else hull.idx.push(a, d, c, a, b, d);
    }
    for (const s of secs) model.addPolyline(lines, s.pts.map((p) => [s.x, sgn * p[0], p[1]]), false);
    for (const k of [0, Math.round(n / 4), Math.round(n / 2), Math.round(3 * n / 4), n - 1]) {
      model.addPolyline(longi, secs.map((s) => [s.x, sgn * s.pts[k][0], s.pts[k][1]]), false);
    }
  }
  return secs.length;
}

/** JSON: {unit, stations:[{x, points:[[y,z],...]} | {x, curves:[[[y,z],...]]}], mirror, samples} */
function loftJson(file, model, opts) {
  const spec = JSON.parse(fs.readFileSync(file, 'utf8'));
  if (spec.unit) model.unit = spec.unit;
  const stations = (spec.stations || []).map((s) => ({ x: +s.x, curves: s.curves || [s.points] }));
  return buildHull(model, stations, { ...opts, mirror: spec.mirror !== undefined ? spec.mirror : opts.mirror, samples: spec.samples || opts.samples });
}

/**
 * DXF lines plan. Two layouts are recognised:
 *  - 3D: each section is a curve lying in a plane x = const (exported from Rhino/Maxsurf);
 *  - 2D body plan: sections drawn in the Y–Z view, station position taken from the layer
 *    name (e.g. "ST_1500", "站-3000", "Station 12" with --station-scale).
 * A third, tried first unless --mode sections: the labelled three-view lines plan ("17.0WL" next to
 * each waterline of the half-breadth plan), see lines_plan.cjs.
 */
function loftDxf(file, model, opts) {
  const doc = readDxfFile(file, opts.encoding);
  if (doc.unit) model.unit = doc.unit;
  const layerRe = opts.layers ? new RegExp(opts.layers, 'i') : null;
  const curves = [];
  const texts = [];
  walk(doc, {
    onCurve: (layer, color, pts) => { if (!layerRe || layerRe.test(layer)) curves.push({ layer, pts }); },
    onFace: () => {},
    onText: (layer, text, p) => { texts.push({ layer, text, x: p[0], y: p[1] }); },
  });
  if (opts.mode !== 'sections') {
    const plan = planToStations(curves, texts, opts);
    if (plan) {
      for (const n of plan.notes) model.warn(n);
      model.planInfo = plan.info;
      return buildHull(model, plan.stations.map((s) => ({ x: s.x, curves: [s.points] })), { ...opts, samples: opts.samples || 64 });
    }
    if (opts.mode === 'plan') throw new Error('没有识别到带标注的型线图：需要半宽水线图里每条水线旁写着形如「17.0WL」的线名，以及高度轴刻度数字。/ no labelled lines plan found');
  }
  if (!curves.length) throw new Error('DXF 里没有找到曲线（检查 --layers 过滤条件）/ no curves found');
  let xmin = Infinity, xmax = -Infinity, span = 0;
  for (const c of curves) for (const p of c.pts) { xmin = Math.min(xmin, p[0]); xmax = Math.max(xmax, p[0]); }
  for (const c of curves) span = Math.max(span, arcLen(c.pts));
  const planarTol = Math.max(span, xmax - xmin) * 1e-4;
  // A 3D section lies in a plane x = const AND rises in Z. A flat body plan (all Z equal)
  // also has degenerate vertical curves at x = const (zero-breadth end stations), so the
  // Z test is what tells the two layouts apart.
  const zSpan = (c) => { let lo = Infinity, hi = -Infinity; for (const p of c.pts) { lo = Math.min(lo, p[2]); hi = Math.max(hi, p[2]); } return hi - lo; };
  const planar = curves.filter((c) => zSpan(c) > planarTol && c.pts.every((p) => Math.abs(p[0] - c.pts[0][0]) <= planarTol));
  const distinctX = new Set(planar.map((c) => Math.round(c.pts[0][0] / Math.max(planarTol, 1e-9))));
  const stations = new Map();
  const put = (x, pts) => { const k = Math.round(x * 1000) / 1000; if (!stations.has(k)) stations.set(k, []); stations.get(k).push(pts); };
  const cl = +opts.centerline || 0;

  if (distinctX.size >= 2) {
    for (const c of planar) put(c.pts[0][0], c.pts.map((p) => [Math.abs(p[1] - cl), p[2]]));
    const other = curves.length - planar.length;
    if (other) {
      const extra = model.part('其他型线 Other lines', '#5c7cfa');
      for (const c of curves) if (!planar.includes(c)) model.addPolyline(extra, c.pts, false);
    }
  } else {
    const scale = +opts.stationScale || 1;
    const re = /(-?\d+(?:\.\d+)?)\s*$/;
    let unnamed = 0;
    for (const c of curves) {
      const m = re.exec(c.layer.trim());
      if (!m) { unnamed++; continue; }
      put(parseFloat(m[1]) * scale, c.pts.map((p) => [Math.abs(p[0] - cl), p[1]]));
    }
    if (unnamed) model.warn(`${unnamed} 条曲线所在图层名里没有站位数值，已忽略（期望形如 ST_1500 / 站-3000）`);
  }
  return buildHull(model, [...stations.entries()].map(([x, cs]) => ({ x, curves: cs })), opts);
}

module.exports = { loftJson, loftDxf, buildHull, chain, resample };
