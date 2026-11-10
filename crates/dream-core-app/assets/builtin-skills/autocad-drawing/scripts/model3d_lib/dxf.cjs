'use strict';
// Minimal ASCII DXF reader producing 3D geometry (WCS) for the viewer and the lofter.
// Handles blocks/inserts, OCS, thickness, 3D polylines, polyface/polygon meshes,
// 3DFACE, MESH, SOLID/TRACE, arcs, circles, ellipses and NURBS splines.

const fs = require('fs');
const { aciToHex, trueColorToHex } = require('./model.cjs');

const UNITS = { 0: '', 1: 'in', 2: 'ft', 4: 'mm', 5: 'cm', 6: 'm', 7: 'km', 8: 'µin', 9: 'mil', 10: 'yd', 14: 'dm' };
const ACIS_TYPES = new Set(['3DSOLID', 'BODY', 'REGION', 'SURFACE', 'PLANESURFACE', 'EXTRUDEDSURFACE', 'LOFTEDSURFACE', 'REVOLVEDSURFACE', 'SWEPTSURFACE', 'NURBSURFACE']);
const IGNORED = new Set(['ATTDEF', 'ATTRIB', 'DIMENSION', 'LEADER', 'MLEADER', 'MULTILEADER', 'HATCH', 'POINT', 'VIEWPORT', 'IMAGE', 'WIPEOUT', 'TOLERANCE', 'OLE2FRAME', 'XLINE', 'RAY', 'SEQEND', 'ENDBLK']);

// ---------- decoding ----------

const CODEPAGES = { ANSI_936: 'gbk', ANSI_950: 'big5', ANSI_932: 'shift_jis', ANSI_949: 'euc-kr', ANSI_1252: 'windows-1252', ANSI_1251: 'windows-1251', ANSI_1250: 'windows-1250' };

function tryDecode(buf, enc) {
  try { return new TextDecoder(enc, { fatal: true }).decode(buf); } catch { return null; }
}

/**
 * R2007+ DXF is always UTF-8. Older files declare $DWGCODEPAGE, but Chinese files
 * are routinely mislabelled ANSI_1252 while holding GBK bytes, so: strict UTF-8 first,
 * then a declared non-Western codepage, then GBK, then Windows-1252.
 */
function decodeDxf(buf, encoding) {
  if (buf.slice(0, 22).toString('latin1').startsWith('AutoCAD Binary DXF')) {
    throw new Error('二进制 DXF 暂不支持：请在 CAD 里另存为「ASCII DXF」。/ Binary DXF is not supported; save as ASCII DXF.');
  }
  let text;
  if (encoding) text = new TextDecoder(encoding).decode(buf);
  else {
    const head = buf.slice(0, Math.min(buf.length, 200000)).toString('latin1');
    const ver = /\$ACADVER\s*\r?\n\s*1\s*\r?\n\s*(AC\d+)/.exec(head);
    const cp = /\$DWGCODEPAGE\s*\r?\n\s*3\s*\r?\n\s*(\S+)/.exec(head);
    const declared = cp && CODEPAGES[cp[1].toUpperCase()];
    if (!ver || ver[1] >= 'AC1021') text = buf.toString('utf8');
    else {
      text = tryDecode(buf, 'utf-8');
      if (text === null && declared && declared !== 'windows-1252') text = tryDecode(buf, declared);
      if (text === null) text = tryDecode(buf, 'gbk');
      if (text === null) text = new TextDecoder(declared || 'windows-1252').decode(buf);
    }
  }
  return text.replace(/\\U\+([0-9a-fA-F]{4})/g, (_, h) => String.fromCharCode(parseInt(h, 16)));
}

function toPairs(text) {
  const lines = text.split(/\r?\n/);
  const pairs = [];
  for (let i = 0; i + 1 < lines.length; i += 2) {
    const code = parseInt(lines[i].trim(), 10);
    if (Number.isNaN(code)) { i -= 1; continue; } // resync on stray lines
    pairs.push([code, lines[i + 1].trim()]);
  }
  return pairs;
}

/** Strip MTEXT/TEXT formatting codes (fonts, colours, paragraph breaks) down to plain text. */
function cleanText(s) {
  return String(s || '')
    .replace(/\\[fFcCHhQqWwTtAa][^;]*;/g, '')
    .replace(/\\[Pp]/g, ' ')
    .replace(/\\~/g, ' ')
    .replace(/%%[cC]/g, 'Ø')
    .replace(/[{}]/g, '')
    .replace(/\s+/g, ' ')
    .trim();
}

// ---------- structure ----------

/** Split the ENTITIES/BLOCKS stream into entity records: {type, g:[[code,val]...], children:[]} */
function readEntities(pairs, start, stopAt) {
  const out = [];
  let i = start, cur = null, owner = null;
  for (; i < pairs.length; i++) {
    const [c, v] = pairs[i];
    if (c === 0) {
      if (stopAt.includes(v)) break;
      const ent = { type: v, g: [], children: [] };
      if (v === 'VERTEX' && owner) owner.children.push(ent);
      else if (v === 'ATTRIB' && owner) { /* belongs to INSERT, ignored */ }
      else if (v === 'SEQEND') owner = null;
      else { out.push(ent); owner = (v === 'POLYLINE' || v === 'INSERT') ? ent : null; }
      cur = ent;
    } else if (cur) cur.g.push([c, v]);
  }
  return { list: out, end: i };
}

function parseDxf(buf, encoding) {
  const pairs = toPairs(decodeDxf(buf, encoding));
  // version/codepage come from the HEADER; `comments` are the leading 999 lines, where converters sign their work
  // ("LibreDWG 0.12.2", "FreeCAD v1.1 ...") — worth reporting because what a tool drops is tool-specific.
  const doc = { unit: '', version: '', codepage: '', comments: pairs.filter(([c]) => c === 999).slice(0, 3).map(([, v]) => v), layers: new Map(), blocks: new Map(), entities: [] };
  for (let i = 0; i < pairs.length; i++) {
    if (pairs[i][0] !== 0 || pairs[i][1] !== 'SECTION') continue;
    const name = pairs[i + 1] && pairs[i + 1][1];
    i += 2;
    if (name === 'HEADER') {
      for (; i < pairs.length && !(pairs[i][0] === 0 && pairs[i][1] === 'ENDSEC'); i++) {
        if (pairs[i][0] === 9 && pairs[i][1] === '$INSUNITS' && pairs[i + 1]) doc.unit = UNITS[parseInt(pairs[i + 1][1], 10)] || '';
        if (pairs[i][0] === 9 && pairs[i][1] === '$ACADVER' && pairs[i + 1]) doc.version = pairs[i + 1][1];
        if (pairs[i][0] === 9 && pairs[i][1] === '$DWGCODEPAGE' && pairs[i + 1]) doc.codepage = pairs[i + 1][1];
      }
    } else if (name === 'TABLES') {
      for (; i < pairs.length && !(pairs[i][0] === 0 && pairs[i][1] === 'ENDSEC'); i++) {
        if (pairs[i][0] === 0 && pairs[i][1] === 'LAYER') {
          const L = { name: '0', color: 7, tc: null, off: false };
          for (i++; i < pairs.length && pairs[i][0] !== 0; i++) {
            const [c, v] = pairs[i];
            if (c === 2) L.name = v;
            else if (c === 62) { const n = parseInt(v, 10); L.color = Math.abs(n); if (n < 0) L.off = true; }
            else if (c === 420) L.tc = trueColorToHex(v);
            else if (c === 70 && (parseInt(v, 10) & 1)) L.off = true; // frozen
          }
          i--;
          doc.layers.set(L.name, L);
        }
      }
    } else if (name === 'BLOCKS') {
      while (i < pairs.length && !(pairs[i][0] === 0 && pairs[i][1] === 'ENDSEC')) {
        if (pairs[i][0] === 0 && pairs[i][1] === 'BLOCK') {
          const blk = { name: '', base: [0, 0, 0], ents: [] };
          for (i++; i < pairs.length && pairs[i][0] !== 0; i++) {
            const [c, v] = pairs[i];
            if (c === 2) blk.name = v;
            else if (c === 10) blk.base[0] = +v; else if (c === 20) blk.base[1] = +v; else if (c === 30) blk.base[2] = +v;
          }
          const r = readEntities(pairs, i, ['ENDBLK', 'ENDSEC']);
          blk.ents = r.list; i = r.end;
          doc.blocks.set(blk.name, blk);
        } else i++;
      }
    } else if (name === 'ENTITIES') {
      const r = readEntities(pairs, i, ['ENDSEC']);
      doc.entities = r.list; i = r.end;
    }
  }
  return doc;
}

// ---------- math ----------

const I4 = () => [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
function mul(a, b) {
  const o = new Array(16);
  for (let r = 0; r < 4; r++) for (let c = 0; c < 4; c++) {
    o[r * 4 + c] = a[r * 4] * b[c] + a[r * 4 + 1] * b[4 + c] + a[r * 4 + 2] * b[8 + c] + a[r * 4 + 3] * b[12 + c];
  }
  return o;
}
function apply(m, p) {
  return [m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3], m[4] * p[0] + m[5] * p[1] + m[6] * p[2] + m[7], m[8] * p[0] + m[9] * p[1] + m[10] * p[2] + m[11]];
}
function applyDir(m, d) {
  return [m[0] * d[0] + m[1] * d[1] + m[2] * d[2], m[4] * d[0] + m[5] * d[1] + m[6] * d[2], m[8] * d[0] + m[9] * d[1] + m[10] * d[2]];
}
function norm(v) { const l = Math.hypot(v[0], v[1], v[2]) || 1; return [v[0] / l, v[1] / l, v[2] / l]; }
function cross(a, b) { return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]; }

/** AutoCAD arbitrary-axis algorithm: OCS -> WCS rotation matrix for extrusion vector n. */
function ocsMatrix(n) {
  n = norm(n);
  if (Math.abs(n[0]) < 1e-12 && Math.abs(n[1]) < 1e-12 && n[2] > 0) return I4();
  const ax = norm(Math.abs(n[0]) < 1 / 64 && Math.abs(n[1]) < 1 / 64 ? cross([0, 1, 0], n) : cross([0, 0, 1], n));
  const ay = norm(cross(n, ax));
  return [ax[0], ay[0], n[0], 0, ax[1], ay[1], n[1], 0, ax[2], ay[2], n[2], 0, 0, 0, 0, 1];
}

// ---------- entity helpers ----------

function fields(e) {
  const f = {};
  for (const [c, v] of e.g) if (!(c in f)) f[c] = v;
  return f;
}
function num(f, c, d = 0) { return c in f ? parseFloat(f[c]) : d; }
function pt(f, c) { return [num(f, c), num(f, c + 10), num(f, c + 20)]; }
function extrusion(f) { return [num(f, 210, 0), num(f, 220, 0), num(f, 230, 1)]; }

function arcPoints(cx, cy, r, a0, a1, z, segsFull) {
  let sweep = a1 - a0;
  while (sweep <= 0) sweep += Math.PI * 2;
  const n = Math.max(4, Math.ceil(segsFull * sweep / (Math.PI * 2)));
  const pts = [];
  for (let i = 0; i <= n; i++) {
    const a = a0 + sweep * i / n;
    pts.push([cx + r * Math.cos(a), cy + r * Math.sin(a), z]);
  }
  return pts;
}

/** Expand LWPOLYLINE/2D POLYLINE vertices with bulges into a dense point list (OCS). */
function bulgeExpand(verts, closed, z, segs) {
  const out = [];
  const n = verts.length;
  for (let i = 0; i < n; i++) {
    const a = verts[i];
    const last = i === n - 1;
    if (last && !closed) { out.push([a.x, a.y, z]); break; }
    const b = verts[(i + 1) % n];
    out.push([a.x, a.y, z]);
    if (a.b && Math.abs(a.b) > 1e-9) {
      const theta = 4 * Math.atan(a.b);
      const dx = b.x - a.x, dy = b.y - a.y, chord = Math.hypot(dx, dy);
      if (chord > 0) {
        const r = chord / (2 * Math.sin(Math.abs(theta) / 2));
        const mx = (a.x + b.x) / 2, my = (a.y + b.y) / 2;
        const h = r * Math.cos(theta / 2) * Math.sign(a.b);
        const cx = mx - dy / chord * h, cy = my + dx / chord * h;
        const a0 = Math.atan2(a.y - cy, a.x - cx);
        const k = Math.max(2, Math.ceil(segs * Math.abs(theta) / (Math.PI * 2)));
        for (let j = 1; j < k; j++) {
          const t = a0 + theta * j / k;
          out.push([cx + r * Math.cos(t), cy + r * Math.sin(t), z]);
        }
      }
    }
  }
  return out;
}

/** Sample a (rational) B-spline curve with de Boor's algorithm. */
function sampleSpline(cps, knots, weights, degree, samples) {
  const n = cps.length;
  if (n < 2) return cps.slice();
  if (!knots || knots.length !== n + degree + 1) {
    knots = [];
    for (let i = 0; i < n + degree + 1; i++) knots.push(Math.min(Math.max(i - degree, 0), n - degree));
  }
  const w = weights && weights.length === n ? weights : cps.map(() => 1);
  const t0 = knots[degree], t1 = knots[n];
  const out = [];
  for (let s = 0; s <= samples; s++) {
    const t = s === samples ? t1 - 1e-10 * (t1 - t0) : t0 + (t1 - t0) * s / samples;
    let k = degree;
    while (k < n - 1 && t >= knots[k + 1]) k++;
    const d = [];
    for (let j = 0; j <= degree; j++) {
      const p = cps[k - degree + j], wj = w[k - degree + j];
      d.push([p[0] * wj, p[1] * wj, p[2] * wj, wj]);
    }
    for (let r = 1; r <= degree; r++) {
      for (let j = degree; j >= r; j--) {
        const i = k - degree + j;
        const den = knots[i + degree - r + 1] - knots[i];
        const a = den === 0 ? 0 : (t - knots[i]) / den;
        for (let q = 0; q < 4; q++) d[j][q] = (1 - a) * d[j - 1][q] + a * d[j][q];
      }
    }
    const h = d[degree];
    out.push([h[0] / h[3], h[1] / h[3], h[2] / h[3]]);
  }
  return out;
}

/**
 * Fit-point-only SPLINE, interpolated the way AutoCAD/BricsCAD do it (ezdxf
 * `cad_fit_point_interpolation`): cubic, chord-length parameters, "natural" knots
 * (the parameters themselves) and zero second derivative at both ends.
 */
function interpolateFit(fit, samples) {
  if (fit.length < 3) return fit.slice();
  const p = 3, nf = fit.length, n = nf + 2; // n control points
  const d = [0];
  for (let i = 1; i < nf; i++) d.push(d[i - 1] + (Math.hypot(fit[i][0] - fit[i - 1][0], fit[i][1] - fit[i - 1][1], fit[i][2] - fit[i - 1][2]) || 1e-9));
  const u = d.map((v) => v / d[nf - 1]);
  const U = [0, 0, 0, 0, ...u.slice(1, -1), 1, 1, 1, 1];
  const basis = (t) => {
    const N = new Array(n).fill(0);
    if (t >= 1) { N[n - 1] = 1; return N; }
    let k = p; while (k < n - 1 && t >= U[k + 1]) k++;
    const Nk = [1], left = [], right = [];
    for (let j = 1; j <= p; j++) {
      left[j] = t - U[k + 1 - j]; right[j] = U[k + j] - t;
      let saved = 0;
      for (let r = 0; r < j; r++) { const tmp = Nk[r] / (right[r + 1] + left[j - r]); Nk[r] = saved + right[r + 1] * tmp; saved = left[j - r] * tmp; }
      Nk[j] = saved;
    }
    for (let j = 0; j <= p; j++) N[k - p + j] = Nk[j];
    return N;
  };
  // Rows: interpolation at every parameter plus C''(0) = 0 and C''(1) = 0
  // (Piegl & Tiller eq. 3.9 / 3.10). Dense Gaussian elimination; fit-point counts are small.
  const zeros = new Array(n - 3).fill(0);
  const u1 = U[p + 1], u2 = U[p + 2], f0 = p * (p - 1) / u1;
  const m = U.length - 1, v1 = U[m - p - 1], v2 = U[m - p - 2], f1 = p * (p - 1) / (1 - v1);
  const A = u.map((t) => basis(t));
  A.splice(1, 0, [f0 / u1, -f0 * (u1 + u2) / (u1 * u2), f0 / u2, ...zeros]);
  A.splice(A.length - 1, 0, [...zeros, f1 / (1 - v2), -f1 * (2 - v1 - v2) / (1 - v1) / (1 - v2), f1 / (1 - v1)]);
  const B = fit.map((q) => q.slice());
  B.splice(1, 0, [0, 0, 0]);
  B.splice(B.length - 1, 0, [0, 0, 0]);
  for (let c = 0; c < n; c++) {
    let piv = c; for (let r = c + 1; r < n; r++) if (Math.abs(A[r][c]) > Math.abs(A[piv][c])) piv = r;
    [A[c], A[piv]] = [A[piv], A[c]]; [B[c], B[piv]] = [B[piv], B[c]];
    const v = A[c][c]; if (Math.abs(v) < 1e-14) return fit.slice();
    for (let r = 0; r < n; r++) {
      if (r === c || !A[r][c]) continue;
      const f = A[r][c] / v;
      for (let k = c; k < n; k++) A[r][k] -= f * A[c][k];
      for (let k = 0; k < 3; k++) B[r][k] -= f * B[c][k];
    }
  }
  const cps = B.map((row, i) => row.map((x) => x / A[i][i]));
  return sampleSpline(cps, U, null, p, samples);
}

// ---------- conversion ----------

/**
 * Walk the drawing and emit primitives through callbacks in WCS:
 *   onCurve(layer, color, pts, closed)   - wire geometry
 *   onFace(layer, color, pts)            - planar polygon (3/4 pts) or triangle
 * Returns counters of skipped entity kinds.
 */
function walk(doc, cb, opts = {}) {
  const segs = opts.arcSegments || 72;
  const skipped = {};
  const layerOf = (name) => doc.layers.get(name) || { name, color: 7, tc: null, off: false };

  function colorOf(f, layer, inherit) {
    if (420 in f) return trueColorToHex(f[420]);
    const aci = 62 in f ? parseInt(f[62], 10) : 256;
    if (aci === 0) return inherit.color; // ByBlock
    if (aci > 0 && aci < 256) return aciToHex(aci);
    const L = layerOf(layer);
    return L.tc || aciToHex(L.color);
  }

  function visit(ents, m, inherit, depth) {
    for (const e of ents) {
      const f = fields(e);
      if (num(f, 67) === 1) continue; // paper space
      if (num(f, 60) === 1) continue; // invisible
      let layer = f[8] || '0';
      if (layer === '0' && inherit.layer) layer = inherit.layer;
      if (layerOf(layer).off) continue;
      const color = colorOf(f, layer, inherit);
      const curve = (pts, closed) => cb.onCurve(layer, color, pts.map((p) => apply(m, p)), closed);
      const face = (pts) => cb.onFace(layer, color, pts.map((p) => apply(m, p)));
      const thick = num(f, 39);
      const ext = extrusion(f);
      const ocs = ocsMatrix(ext);
      const tdir = (p) => applyDir(m, applyDir(ocs, [0, 0, thick]));
      const extrude = (pts, closed) => {
        if (!thick) return;
        const d = tdir();
        const wp = pts.map((p) => apply(m, p));
        const n = wp.length, count = closed ? n : n - 1;
        for (let i = 0; i < count; i++) {
          const a = wp[i], b = wp[(i + 1) % n];
          cb.onFace(layer, color, [a, b, [b[0] + d[0], b[1] + d[1], b[2] + d[2]], [a[0] + d[0], a[1] + d[1], a[2] + d[2]]]);
        }
        cb.onCurve(layer, color, wp.map((p) => [p[0] + d[0], p[1] + d[1], p[2] + d[2]]), closed);
      };

      switch (e.type) {
        case 'LINE': {
          const a = pt(f, 10), b = pt(f, 11);
          curve([a, b], false); extrude([a, b], false);
          break;
        }
        case 'CIRCLE': case 'ARC': {
          const c = pt(f, 10), r = num(f, 40);
          const a0 = e.type === 'ARC' ? num(f, 50) * Math.PI / 180 : 0;
          const a1 = e.type === 'ARC' ? num(f, 51) * Math.PI / 180 : Math.PI * 2;
          let pts = arcPoints(c[0], c[1], r, a0, a1, c[2], segs).map((p) => apply(ocs, p));
          const closed = e.type === 'CIRCLE';
          if (closed) pts = pts.slice(0, -1);
          curve(pts, closed); extrude(pts, closed);
          if (closed && thick) { face(pts.slice().reverse()); const d = applyDir(ocs, [0, 0, thick]); face(pts.map((p) => [p[0] + d[0], p[1] + d[1], p[2] + d[2]])); }
          break;
        }
        case 'ELLIPSE': {
          const c = pt(f, 10), maj = pt(f, 11), ratio = num(f, 40, 1);
          const n = norm(ext), minor = cross(n, maj).map((v) => v * ratio);
          let t0 = num(f, 41, 0), t1 = num(f, 42, Math.PI * 2);
          if (t1 <= t0) t1 += Math.PI * 2;
          const k = Math.max(8, Math.ceil(segs * (t1 - t0) / (Math.PI * 2)));
          const pts = [];
          for (let i = 0; i <= k; i++) {
            const t = t0 + (t1 - t0) * i / k;
            pts.push([0, 1, 2].map((j) => c[j] + maj[j] * Math.cos(t) + minor[j] * Math.sin(t)));
          }
          curve(pts, false);
          break;
        }
        case 'LWPOLYLINE': {
          const verts = [];
          for (const [c, v] of e.g) {
            if (c === 10) verts.push({ x: +v, y: 0, b: 0 });
            else if (c === 20 && verts.length) verts[verts.length - 1].y = +v;
            else if (c === 42 && verts.length) verts[verts.length - 1].b = +v;
          }
          const closed = (num(f, 70) & 1) === 1;
          const pts = bulgeExpand(verts, closed, num(f, 38), segs).map((p) => apply(ocs, p));
          curve(pts, closed); extrude(pts, closed);
          break;
        }
        case 'POLYLINE': {
          const flags = num(f, 70);
          const vs = e.children.map((v) => { const vf = fields(v); return { f: vf, flags: num(vf, 70), p: pt(vf, 10), b: num(vf, 42) }; });
          if (flags & 64) { // polyface mesh
            const verts = vs.filter((v) => (v.flags & 192) === 192).map((v) => v.p);
            for (const v of vs) {
              if ((v.flags & 192) !== 128) continue;
              const ids = [71, 72, 73, 74].map((c) => Math.abs(num(v.f, c))).filter((x) => x > 0 && x <= verts.length);
              if (ids.length >= 3) face(ids.map((x) => verts[x - 1]));
            }
          } else if (flags & 16) { // M x N polygon mesh
            const M = num(f, 71), N = num(f, 72);
            const grid = vs.map((v) => v.p);
            if (M * N === grid.length) {
              const cm = flags & 1, cn = flags & 32;
              for (let i = 0; i < (cm ? M : M - 1); i++) for (let j = 0; j < (cn ? N : N - 1); j++) {
                const i2 = (i + 1) % M, j2 = (j + 1) % N;
                face([grid[i * N + j], grid[i * N + j2], grid[i2 * N + j2], grid[i2 * N + j]]);
              }
            }
          } else if (flags & 8) { // 3D polyline (WCS)
            curve(vs.filter((v) => !(v.flags & 16)).map((v) => v.p), (flags & 1) === 1);
          } else { // 2D polyline in OCS
            const z = num(f, 30);
            const pts = bulgeExpand(vs.map((v) => ({ x: v.p[0], y: v.p[1], b: v.b })), (flags & 1) === 1, z, segs).map((p) => apply(ocs, p));
            curve(pts, (flags & 1) === 1); extrude(pts, (flags & 1) === 1);
          }
          break;
        }
        case '3DFACE': {
          const p = [pt(f, 10), pt(f, 11), pt(f, 12), pt(f, 13)];
          const same = p[2][0] === p[3][0] && p[2][1] === p[3][1] && p[2][2] === p[3][2];
          face(same ? p.slice(0, 3) : p);
          break;
        }
        case 'SOLID': case 'TRACE': {
          const p = [pt(f, 10), pt(f, 11), pt(f, 13), pt(f, 12)].map((q) => apply(ocs, q));
          face(p); extrude(p, true);
          break;
        }
        case 'SPLINE': {
          const deg = num(f, 71, 3);
          const knots = [], w = [], cps = [], fit = [];
          let cur = null;
          for (const [c, v] of e.g) {
            if (c === 40) knots.push(+v);
            else if (c === 41) w.push(+v);
            else if (c === 10) { cur = [+v, 0, 0]; cps.push(cur); } else if (c === 20 && cur) cur[1] = +v; else if (c === 30 && cur) cur[2] = +v;
            else if (c === 11) { cur = [+v, 0, 0]; fit.push(cur); } else if (c === 21 && cur) cur[1] = +v; else if (c === 31 && cur) cur[2] = +v;
          }
          const pts = cps.length >= 2 ? sampleSpline(cps, knots, w, deg, Math.max(24, cps.length * 8))
            : interpolateFit(fit, Math.max(24, fit.length * 8));
          curve(pts, (num(f, 70) & 1) === 1);
          break;
        }
        case 'MESH': {
          const verts = [], faces = [];
          let i = 0;
          const g = e.g;
          while (i < g.length) {
            const [c, v] = g[i];
            if (c === 92) {
              const cnt = parseInt(v, 10); i++;
              for (let k = 0; k < cnt && i < g.length; i++) { if (g[i][0] === 10) { verts.push([+g[i][1], +g[i + 1][1], +g[i + 2][1]]); i += 2; k++; } }
              continue;
            }
            if (c === 93) {
              const cnt = parseInt(v, 10); i++;
              const items = [];
              while (items.length < cnt && i < g.length && g[i][0] === 90) { items.push(parseInt(g[i][1], 10)); i++; }
              for (let k = 0; k < items.length;) { const n = items[k]; faces.push(items.slice(k + 1, k + 1 + n)); k += n + 1; }
              continue;
            }
            i++;
          }
          for (const fc of faces) if (fc.length >= 3 && fc.every((x) => x < verts.length)) face(fc.map((x) => verts[x]));
          break;
        }
        case 'TEXT': case 'MTEXT': {
          if (!cb.onText) break;
          let raw = f[1] || '';
          if (e.type === 'MTEXT') raw = e.g.filter(([c]) => c === 3).map(([, v]) => v).join('') + raw;
          const p = pt(f, 10);
          cb.onText(layer, cleanText(raw), apply(m, e.type === 'TEXT' ? apply(ocs, p) : p), num(f, 40));
          break;
        }
        case 'INSERT': {
          const blk = doc.blocks.get(f[2]);
          if (!blk || depth > 16) break;
          const ins = pt(f, 10), sx = num(f, 41, 1), sy = num(f, 42, 1), sz = num(f, 43, 1);
          const rot = num(f, 50) * Math.PI / 180, cols = Math.max(1, num(f, 70, 1)), rows = Math.max(1, num(f, 71, 1));
          const cs = num(f, 44), rs = num(f, 45);
          const c = Math.cos(rot), s = Math.sin(rot);
          for (let r = 0; r < rows; r++) for (let k = 0; k < cols; k++) {
            const ox = k * cs, oy = r * rs;
            const R = [c, -s, 0, ins[0] + c * ox - s * oy, s, c, 0, ins[1] + s * ox + c * oy, 0, 0, 1, ins[2], 0, 0, 0, 1];
            const S = [sx, 0, 0, -sx * blk.base[0], 0, sy, 0, -sy * blk.base[1], 0, 0, sz, -sz * blk.base[2], 0, 0, 0, 1];
            visit(blk.ents, mul(m, mul(ocs, mul(R, S))), { layer, color }, depth + 1);
          }
          break;
        }
        default:
          if (ACIS_TYPES.has(e.type)) skipped.acis = (skipped.acis || 0) + 1;
          else if (!IGNORED.has(e.type)) skipped[e.type] = (skipped[e.type] || 0) + 1;
      }
    }
  }

  visit(doc.entities, I4(), { layer: null, color: aciToHex(7) }, 0);
  return skipped;
}

function readDxfFile(file, encoding) {
  return parseDxf(fs.readFileSync(file), encoding);
}

module.exports = { readDxfFile, parseDxf, walk, sampleSpline, cleanText };
