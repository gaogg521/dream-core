'use strict';
// Shared in-memory model: named parts holding triangles and/or line segments.

const ACI_BASE = {
  1: '#ff0000', 2: '#ffff00', 3: '#00ff00', 4: '#00ffff', 5: '#0000ff', 6: '#ff00ff',
  7: '#4a5260', 8: '#808080', 9: '#c0c0c0',
};

/** AutoCAD Color Index -> hex. 10..249 follow the hue/shade wheel; 250..255 are greys. */
function aciToHex(aci) {
  aci = Math.abs(aci | 0);
  if (ACI_BASE[aci]) return ACI_BASE[aci];
  if (aci >= 250 && aci <= 255) {
    const g = Math.round(51 + (aci - 250) * 40.8);
    return rgbHex(g, g, g);
  }
  if (aci < 10 || aci > 249) return ACI_BASE[7];
  const hue = Math.floor((aci - 10) / 10) * 15;
  const shade = (aci - 10) % 10;
  const value = [1, 1, 0.8, 0.8, 0.6, 0.6, 0.5, 0.5, 0.3, 0.3][shade];
  const sat = shade % 2 === 0 ? 1 : 0.5;
  return hsvHex(hue, sat, value);
}

function hsvHex(h, s, v) {
  const c = v * s, x = c * (1 - Math.abs(((h / 60) % 2) - 1)), m = v - c;
  let r = 0, g = 0, b = 0;
  if (h < 60) [r, g, b] = [c, x, 0];
  else if (h < 120) [r, g, b] = [x, c, 0];
  else if (h < 180) [r, g, b] = [0, c, x];
  else if (h < 240) [r, g, b] = [0, x, c];
  else if (h < 300) [r, g, b] = [x, 0, c];
  else [r, g, b] = [c, 0, x];
  return rgbHex((r + m) * 255, (g + m) * 255, (b + m) * 255);
}

function rgbHex(r, g, b) {
  return '#' + [r, g, b].map((v) => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, '0')).join('');
}

function trueColorToHex(v) {
  v = Number(v) >>> 0;
  return rgbHex((v >> 16) & 255, (v >> 8) & 255, v & 255);
}

class Model {
  constructor() {
    this.parts = new Map(); // key -> {name, color, pos:[], idx:[], lines:[]}
    this.warnings = [];
    this.unit = 'mm';
    this.up = 'z';
  }

  part(name, color) {
    const key = name + '\u0000' + color;
    let p = this.parts.get(key);
    if (!p) {
      p = { name, color, pos: [], idx: [], lines: [] };
      this.parts.set(key, p);
    }
    return p;
  }

  /** Add a fan-triangulated planar polygon (3 or more points). */
  addPolygon(part, pts) {
    if (pts.length < 3) return;
    const base = part.pos.length / 3;
    for (const p of pts) part.pos.push(p[0], p[1], p[2]);
    for (let i = 1; i + 1 < pts.length; i++) part.idx.push(base, base + i, base + i + 1);
  }

  /** Add a polyline as consecutive line segments. */
  addPolyline(part, pts, closed) {
    for (let i = 0; i + 1 < pts.length; i++) pushSeg(part.lines, pts[i], pts[i + 1]);
    if (closed && pts.length > 2) pushSeg(part.lines, pts[pts.length - 1], pts[0]);
  }

  /** Extrude an open/closed polyline along vector d into wall quads (DXF thickness). */
  addExtrusion(part, pts, closed, d) {
    const n = pts.length;
    const count = closed ? n : n - 1;
    for (let i = 0; i < count; i++) {
      const a = pts[i], b = pts[(i + 1) % n];
      this.addPolygon(part, [a, b, add(b, d), add(a, d)]);
    }
  }

  warn(msg) {
    if (!this.warnings.includes(msg)) this.warnings.push(msg);
  }

  stats() {
    let tris = 0, segs = 0;
    for (const p of this.parts.values()) { tris += p.idx.length / 3; segs += p.lines.length / 6; }
    return { parts: this.parts.size, tris, segs };
  }
}

function pushSeg(arr, a, b) {
  if (a[0] === b[0] && a[1] === b[1] && a[2] === b[2]) return;
  arr.push(a[0], a[1], a[2], b[0], b[1], b[2]);
}

function add(a, b) { return [a[0] + b[0], a[1] + b[1], a[2] + b[2]]; }

module.exports = { Model, aciToHex, trueColorToHex, rgbHex };
