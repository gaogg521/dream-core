'use strict';
// Recognise a labelled hull lines plan (三视图型线图) in a flat DXF and turn its half-breadth plan into
// station sections for loft.cjs.
//
// What is relied on (and nothing else): the half-breadth plan draws one smooth curve per waterline and
// writes the waterline height next to it ("17.0WL"); its height axis carries plain numbers ("0.5 … 16.0")
// which give the drawing-units-per-label-unit scale and where the centreline is. Station labels, the
// sheer plan and the body plan are NOT needed: sections are cut from the waterlines at dense x positions.

// "17.0WL", but also "WL TOP(20.0WL)" / "(20.0WL)": the number may follow a prefix or an opening bracket.
// "14.0BL" (buttock) and prose containing no "<number>WL" at the end do not match.
const WL_RE = /(?:^|[\s(（])(-?\d+(?:\.\d+)?)\s*(?:WL|W\.L\.?|水线)\s*[)）]?$/i;
const NUM_RE = /^-?\d+(?:\.\d+)?$/;

function bbox(pts) {
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const p of pts) { if (p[0] < x0) x0 = p[0]; if (p[0] > x1) x1 = p[0]; if (p[1] < y0) y0 = p[1]; if (p[1] > y1) y1 = p[1]; }
  return { x0, y0, x1, y1 };
}

function distToPoly(p, x, y) {
  let best = Infinity;
  for (let i = 0; i + 1 < p.length; i++) {
    const [ax, ay] = p[i], [bx, by] = p[i + 1], dx = bx - ax, dy = by - ay, L2 = dx * dx + dy * dy || 1;
    const t = Math.max(0, Math.min(1, ((x - ax) * dx + (y - ay) * dy) / L2));
    const d = Math.hypot(x - (ax + t * dx), y - (ay + t * dy));
    if (d < best) best = d;
  }
  return best;
}

/** y of a curve at x; the outermost crossing when the curve folds back. */
function yAt(p, x) {
  let y = null;
  for (let i = 0; i + 1 < p.length; i++) {
    const [ax, ay] = p[i], [bx, by] = p[i + 1];
    if ((ax - x) * (bx - x) <= 0 && ax !== bx) {
      const yy = ay + (by - ay) * (x - ax) / (bx - ax);
      if (y === null || yy > y) y = yy;
    }
  }
  return y;
}

function cluster(items, gap) {
  const parent = items.map((_, i) => i);
  const find = (i) => { while (parent[i] !== i) { parent[i] = parent[parent[i]]; i = parent[i]; } return i; };
  for (let i = 0; i < items.length; i++) {
    for (let j = i + 1; j < items.length; j++) {
      const a = items[i].bb, b = items[j].bb;
      if (a.x0 - gap <= b.x1 && b.x0 - gap <= a.x1 && a.y0 - gap <= b.y1 && b.y0 - gap <= a.y1) parent[find(i)] = find(j);
    }
  }
  const groups = new Map();
  items.forEach((_, i) => { const r = find(i); if (!groups.has(r)) groups.set(r, []); groups.get(r).push(i); });
  return [...groups.values()];
}

/** Pooled within-group regression: pairs are [value, y, side]; returns {a, b:[bLeft, bRight]}. */
function axisFit(pairs) {
  const groups = [[], []];
  for (const p of pairs) groups[p[2]].push(p);
  let num = 0, den = 0;
  const mean = groups.map((g) => (g.length ? [g.reduce((s, p) => s + p[0], 0) / g.length, g.reduce((s, p) => s + p[1], 0) / g.length] : null));
  groups.forEach((g, i) => { for (const p of g) { num += (p[0] - mean[i][0]) * (p[1] - mean[i][1]); den += (p[0] - mean[i][0]) ** 2; } });
  if (!den) return null;
  const a = num / den;
  return { a, b: mean.map((m) => (m ? m[1] - a * m[0] : null)) };
}

/**
 * @param curves [{layer, pts:[[x,y,z]...]}]  @param texts [{layer, text, x, y}]
 * @returns null when the drawing does not look like a labelled lines plan, else {stations, notes, info}
 */
function planToStations(curves, texts, opts = {}) {
  const wlTexts = texts.map((t) => ({ ...t, m: WL_RE.exec(t.text) })).filter((t) => t.m).map((t) => ({ ...t, v: parseFloat(t.m[1]) }));
  if (wlTexts.length < 3) return null;
  const notes = [];

  for (const minPts of [6, 3]) {
    const all = curves.filter((c) => c.pts.length >= minPts).map((c) => ({ pts: c.pts.map((p) => [p[0], p[1]]), layer: c.layer }));
    const axisAligned = (p) => p.every((q) => Math.abs(q[0] - p[0][0]) < 1e-6) || p.every((q) => Math.abs(q[1] - p[0][1]) < 1e-6);
    const items = all.filter((c) => !axisAligned(c.pts)).map((c) => ({ ...c, bb: bbox(c.pts) }));
    if (items.length < 3) continue;
    const tot = bbox(items.flatMap((c) => c.pts));
    const W0 = tot.x1 - tot.x0, H0 = tot.y1 - tot.y0;
    // The sheet frame is one closed polyline spanning almost everything: not a view.
    const dense = items.filter((c) => !((c.bb.x1 - c.bb.x0) > 0.9 * W0 && (c.bb.y1 - c.bb.y0) > 0.9 * H0));
    const groups = cluster(dense, 0.012 * Math.max(W0, H0));

    // The half-breadth plan is the view the WL labels sit beside.
    const score = new Map();
    for (const t of wlTexts) {
      let bi = -1, bd = Infinity;
      dense.forEach((c, i) => { const d = distToPoly(c.pts, t.x, t.y); if (d < bd) { bd = d; bi = i; } });
      const g = groups.findIndex((gr) => gr.includes(bi));
      score.set(g, (score.get(g) || 0) + 1);
    }
    const best = [...score.entries()].sort((a, b) => b[1] - a[1])[0];
    if (!best || best[0] < 0) continue;
    const view = groups[best[0]].map((i) => dense[i]);
    const vb = bbox(view.flatMap((c) => c.pts));
    const VW = vb.x1 - vb.x0, VH = vb.y1 - vb.y0;
    if (VW <= 0 || VH <= 0) continue;

    // Waterline family: curves running (nearly) the whole length of the view.
    const fam = view.filter((c) => c.bb.x0 <= vb.x0 + 0.08 * VW && c.bb.x1 >= vb.x1 - 0.08 * VW).map((c) => ({ pts: c.pts, bb: c.bb, z: null, conf: -1, alt: [], est: false }));
    if (fam.length < 3) continue;

    // Height axis: plain numbers just outside the view's left/right edge -> y = a*v + b.
    let pairs = texts.filter((t) => NUM_RE.test(t.text) && t.y >= vb.y0 - 0.04 * VH && t.y <= vb.y1 + 0.04 * VH
      && ((t.x < vb.x0 + 0.01 * VW && t.x > vb.x0 - 0.06 * VW) || (t.x > vb.x1 - 0.01 * VW && t.x < vb.x1 + 0.06 * VW)))
      .map((t) => [parseFloat(t.text), t.y, t.x < (vb.x0 + vb.x1) / 2 ? 0 : 1]);
    // One slope for both sides, one intercept per side: the left and right label columns are often anchored
    // differently (text hangs below its line on one side only), and mixing them biases the slope.
    let fit = null;
    for (let k = 0; k < 4 && pairs.length >= 3; k++) {
      fit = axisFit(pairs);
      if (!fit) break;
      const keep = pairs.filter(([v, y, s]) => Math.abs(fit.a * v + fit.b[s] - y) <= 0.03 * VH);
      if (keep.length === pairs.length) break;
      pairs = keep;
    }
    if (fit) { const bs = fit.b.filter((b) => b !== null); fit = { a: fit.a, b: bs.reduce((s, b) => s + b, 0) / bs.length }; }
    let scale, yCL, sign = 1;
    if (fit && pairs.length >= 3 && Math.abs(fit.a) > 0) {
      scale = Math.abs(fit.a); sign = Math.sign(fit.a); yCL = fit.b;
      if (Math.abs(scale - Math.round(scale)) < 1e-4 * scale) scale = Math.round(scale); // regression noise on text positions
      // Text insertion points sit a little off the grid line they label; the centreline is a view edge.
      if (Math.abs(yCL - vb.y0) <= 0.04 * VH) yCL = vb.y0; else if (Math.abs(yCL - vb.y1) <= 0.04 * VH) yCL = vb.y1;
    } else {
      if (!opts.scale) throw new Error('找到了水线标注，但找不到高度轴刻度数字，无法确定图纸单位与中线位置；请用 --scale <图纸单位/标注单位> 指定（例如图纸单位 mm、标注单位 m 时为 1000）。');
      scale = +opts.scale; yCL = vb.y0; sign = 1;
      notes.push('未找到高度轴刻度数字，中线按视图底边处理（可用 --centerline 指定）');
    }
    if (opts.centerline !== undefined && opts.centerline !== '') { yCL = +opts.centerline; sign = (vb.y1 + vb.y0) / 2 >= yCL ? 1 : -1; }

    // Order the family innermost -> outermost (= lowest -> highest waterline) so label order can be checked
    // against geometry. Nested waterlines never cross, so compare every pair at many x positions and let the
    // majority decide; positions where two curves nearly coincide (the vertical side) carry no information.
    const hb = (y) => (y - yCL) * sign;
    const xs = Array.from({ length: 60 }, (_, k) => vb.x0 + (VW * (k + 0.5)) / 60);
    const h = fam.map((c) => xs.map((x) => { const y = yAt(c.pts, x); return y === null ? null : hb(y); }));
    const thr = 0.003 * VH, wins = fam.map(() => 0);
    for (let a = 0; a < fam.length; a++) {
      for (let b = a + 1; b < fam.length; b++) {
        let up = 0, down = 0;
        for (let k = 0; k < xs.length; k++) {
          if (h[a][k] === null || h[b][k] === null) continue;
          const d = h[a][k] - h[b][k];
          if (d > thr) up++; else if (d < -thr) down++;
        }
        if (up > down) wins[a]++; else if (down > up) wins[b]++;
      }
    }
    const order = fam.map((c, i) => ({ c, w: wins[i], i })).sort((p, q) => p.w - q.w);
    const keep = order.map((o) => o.c);
    return finish(keep, wlTexts, scale, sign, yCL, vb, VH, notes, opts);
  }
  return null;
}

function finish(keep, wlTexts, scale, sign, yCL, vb, VH, notes, opts) {
  // Label -> curve: nearest curve; a curve carrying several labels keeps the best-separated one
  // (bow labels crowd together and attach to the wrong neighbour).
  const reach = 0.05 * VH;
  const per = keep.map(() => []);
  for (const t of wlTexts) {
    let bi = -1, bd = Infinity, second = Infinity;
    keep.forEach((c, i) => { const d = distToPoly(c.pts, t.x, t.y); if (d < bd) { second = bd; bd = d; bi = i; } else if (d < second) second = d; });
    if (bi >= 0 && bd <= reach) per[bi].push({ v: t.v, margin: second - bd });
  }
  keep.forEach((c, i) => {
    const v = per[i].slice().sort((a, b) => b.margin - a.margin);
    if (v.length) { c.v = v[0].v; c.conf = v[0].margin; c.alt = [...new Set(v.map((q) => q.v))].filter((z) => z !== c.v); } else { c.v = null; c.conf = -1; c.alt = []; }
  });

  // Heights must rise with rank. Drop the least confident label of any pair that breaks the order.
  let changed = true;
  while (changed) {
    changed = false;
    for (let k = 0; k < keep.length; k++) {
      const c = keep[k];
      if (c.v === null) continue;
      const prev = keep.slice(0, k).reverse().find((q) => q.v !== null), next = keep.slice(k + 1).find((q) => q.v !== null);
      const bad = (prev && c.v <= prev.v) ? (c.conf < prev.conf ? c : prev) : (next && c.v >= next.v) ? (c.conf < next.conf ? c : next) : null;
      if (bad) { notes.push(`水线标注「${bad.v}」与曲线的上下顺序矛盾，已丢弃`); bad.v = null; changed = true; break; }
    }
  }
  // Unlabelled curves between two labelled ones are interpolated by rank.
  const est = [];
  for (let k = 0; k < keep.length; k++) {
    if (keep[k].v !== null) continue;
    let lo = k - 1; while (lo >= 0 && keep[lo].v === null) lo--;
    let hi = k + 1; while (hi < keep.length && keep[hi].v === null) hi++;
    if (lo < 0 || hi >= keep.length) continue;
    keep[k].v = keep[lo].v + (keep[hi].v - keep[lo].v) * (k - lo) / (hi - lo);
    keep[k].est = true; est.push(+keep[k].v.toFixed(2));
  }
  const wl = keep.filter((c) => c.v !== null).map((c) => ({ z: c.v * scale, pts: c.pts, est: c.est, alt: c.alt, v: c.v }));
  if (wl.length < 3) throw new Error('可用的水线曲线少于 3 条，无法放样；请检查线名标注是否紧贴曲线。');
  if (est.length) notes.push(`${est.length} 条水线没有可靠标注，高度按上下相邻水线内插（约 ${est.join('、')}）`);
  const alts = wl.filter((w) => w.alt.length);
  if (alts.length) notes.push('同一曲线有多个线名时取最清晰的一个：' + alts.map((w) => `${w.v}（舍 ${w.alt.join('/')}）`).join('；'));

  const X0 = Math.min(...wl.map((w) => Math.min(...w.pts.map((p) => p[0])))), X1 = Math.max(...wl.map((w) => Math.max(...w.pts.map((p) => p[0]))));
  const ranges = wl.map((w) => { const xs = w.pts.map((p) => p[0]); return [Math.min(...xs), Math.max(...xs)]; });
  const N = +opts.stations || 140, stations = [];
  for (let i = 0; i <= N; i++) {
    const x = X0 + ((X1 - X0) * i) / N;
    const pts = [];
    wl.forEach((w, k) => {
      if (x < ranges[k][0] - 1e-6 || x > ranges[k][1] + 1e-6) return;
      const y = yAt(w.pts, x);
      if (y !== null) pts.push([Math.max(0, (y - yCL) * sign), w.z]);
    });
    if (pts.length < 3) continue;
    pts.sort((a, b) => a[1] - b[1]);
    const uniq = [];
    for (const q of pts) if (!uniq.length || q[1] > uniq[uniq.length - 1][1] + 1e-9) uniq.push(q);
    stations.push({ x, points: [[0, uniq[0][1]], ...uniq] });
  }
  if (stations.length < 3) throw new Error('放样站位不足，水线曲线没有覆盖足够的船长。');
  notes.push(`只放样了主水线族；艏艉细节曲线（球鼻艏、艉部、尾轴区域）和船底升高未建模，船底按平底处理且从最低水线（${+wl[0].v.toFixed(2)}）起算，不含龙骨线，船壳敞口无甲板`);
  return {
    stations,
    notes,
    info: { waterlines: wl.length, heights: wl.map((w) => +w.v.toFixed(2)), scale, centerlineY: yCL, view: vb, length: X1 - X0 },
  };
}

module.exports = { planToStations, WL_RE };
