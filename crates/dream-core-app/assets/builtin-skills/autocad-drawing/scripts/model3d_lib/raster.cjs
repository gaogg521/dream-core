'use strict';
// preview: rasterise a DXF's lines to a PNG an agent can look at directly (zero dependencies: Bresenham + zlib).
//
// Why it exists: a flat multi-view sheet (general arrangement, three-view drawing) cannot be shown by the 3D viewer
// — that needs WebGL, which neither the in-app browser pane nor the HTML-to-image tool reliably has. Without a picture,
// an agent that wants to "see the layout" ends up writing its own DXF renderer (95 steps in one real run).
// The image carries a labelled grid and the command prints the box and scale, so a follow-up `--box` zoom is one step.

const zlib = require('zlib');

const CRC = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; t[n] = c >>> 0; }
  return t;
})();
function crc32(buf) { let c = 0xffffffff; for (let i = 0; i < buf.length; i++) c = CRC[(c ^ buf[i]) & 255] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; }
function chunk(type, data) {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}
function encodePng(w, h, rgb) {
  const stride = w * 3 + 1, raw = Buffer.alloc(stride * h);
  for (let y = 0; y < h; y++) { raw[y * stride] = 0; Buffer.from(rgb.buffer, rgb.byteOffset + y * w * 3, w * 3).copy(raw, y * stride + 1); }
  const ihdr = Buffer.alloc(13); ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4); ihdr[8] = 8; ihdr[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
}

// 3x5 digits, enough to label grid lines with their drawing coordinates.
const GLYPH = {
  0: '111101101101111', 1: '010110010010111', 2: '111001111100111', 3: '111001111001111', 4: '101101111001001',
  5: '111100111001111', 6: '111100111101111', 7: '111001001001001', 8: '111101111101111', 9: '111101111001111', '-': '000000111000000',
};

function niceStep(raw) {
  const p = Math.pow(10, Math.floor(Math.log10(raw))), f = raw / p;
  return (f < 1.5 ? 1 : f < 3.5 ? 2 : f < 7.5 ? 5 : 10) * p;
}

/** Liang–Barsky: clip segment to the box; returns null when it lies outside. */
function clip(x0, y0, x1, y1, b) {
  let t0 = 0, t1 = 1;
  const dx = x1 - x0, dy = y1 - y0;
  for (const [p, q] of [[-dx, x0 - b[0]], [dx, b[2] - x0], [-dy, y0 - b[1]], [dy, b[3] - y0]]) {
    if (p === 0) { if (q < 0) return null; continue; }
    const r = q / p;
    if (p < 0) { if (r > t1) return null; if (r > t0) t0 = r; } else { if (r < t0) return null; if (r < t1) t1 = r; }
  }
  return [x0 + t0 * dx, y0 + t0 * dy, x0 + t1 * dx, y0 + t1 * dy];
}

/**
 * @param curves [{layer, pts:[[x,y,z]...]}]   @param opts {width, box:[x0,y0,x1,y1], layers:RegExp, skip:RegExp}
 * @returns {png, box, width, height, scale, gridStep, segments}
 */
function renderPng(curves, opts = {}) {
  const sel = curves.filter((c) => (!opts.layers || opts.layers.test(c.layer)) && (!opts.skip || !opts.skip.test(c.layer)));
  let box = opts.box;
  if (!box) {
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const c of sel) for (const p of c.pts) { if (p[0] < x0) x0 = p[0]; if (p[0] > x1) x1 = p[0]; if (p[1] < y0) y0 = p[1]; if (p[1] > y1) y1 = p[1]; }
    if (!Number.isFinite(x0)) throw new Error('没有可画的线（检查 --layers / --skip-layers 过滤条件）');
    const mx = (x1 - x0) * 0.02 || 1, my = (y1 - y0) * 0.02 || 1;
    box = [x0 - mx, y0 - my, x1 + mx, y1 + my];
  }
  if (!(box[2] > box[0] && box[3] > box[1])) throw new Error('--box 必须是 x0,y0,x1,y1 且 x1>x0、y1>y0');
  const maxSide = Math.min(4000, Math.max(200, +opts.width || 2000));
  const bw = box[2] - box[0], bh = box[3] - box[1];
  const W = bw >= bh ? maxSide : Math.max(50, Math.round((maxSide * bw) / bh));
  const H = bw >= bh ? Math.max(50, Math.round((maxSide * bh) / bw)) : maxSide;
  const sx = (W - 1) / bw, sy = (H - 1) / bh;

  // labelled light grid first, drawing on top
  const rgb = new Uint8Array(W * H * 3).fill(255);
  const put = (x, y, r, g, b) => { if (x >= 0 && y >= 0 && x < W && y < H) { const i = (y * W + x) * 3; rgb[i] = r; rgb[i + 1] = g; rgb[i + 2] = b; } };
  const gridStep = niceStep(Math.max(bw, bh) / 8);
  for (let gx = Math.ceil(box[0] / gridStep) * gridStep; gx <= box[2]; gx += gridStep) { const px = Math.round((gx - box[0]) * sx); for (let y = 0; y < H; y++) put(px, y, 218, 228, 242); }
  for (let gy = Math.ceil(box[1] / gridStep) * gridStep; gy <= box[3]; gy += gridStep) { const py = Math.round((box[3] - gy) * sy); for (let x = 0; x < W; x++) put(x, py, 218, 228, 242); }

  // density: overlapping lines darken, so dense regions read as dark areas and sparse ones as thin lines
  const cnt = new Uint8Array(W * H);
  let segments = 0;
  const plot = (x, y) => { if (x >= 0 && y >= 0 && x < W && y < H) { const i = y * W + x; if (cnt[i] < 255) cnt[i]++; } };
  for (const c of sel) {
    const p = c.pts;
    for (let i = 0; i + 1 < p.length; i++) {
      const s = clip(p[i][0], p[i][1], p[i + 1][0], p[i + 1][1], box);
      if (!s) continue;
      segments++;
      let ax = Math.round((s[0] - box[0]) * sx), ay = Math.round((box[3] - s[1]) * sy);
      const bx = Math.round((s[2] - box[0]) * sx), by = Math.round((box[3] - s[3]) * sy);
      const dx = Math.abs(bx - ax), dy = -Math.abs(by - ay), stx = ax < bx ? 1 : -1, sty = ay < by ? 1 : -1;
      let err = dx + dy;
      for (;;) {
        plot(ax, ay);
        if (ax === bx && ay === by) break;
        const e2 = 2 * err;
        if (e2 >= dy) { err += dy; ax += stx; }
        if (e2 <= dx) { err += dx; ay += sty; }
      }
    }
  }
  for (let i = 0; i < cnt.length; i++) {
    if (!cnt[i]) continue;
    const k = 1 - Math.exp(-0.55 * cnt[i]); // 0..1
    rgb[i * 3] = Math.round(255 - k * 225); rgb[i * 3 + 1] = Math.round(255 - k * 215); rgb[i * 3 + 2] = Math.round(255 - k * 170);
  }

  // grid labels on top edge (x) and left edge (y), in drawing units
  const text = (str, x, y, r, g, b) => {
    for (let ci = 0; ci < str.length; ci++) {
      const gl = GLYPH[str[ci]]; if (!gl) continue;
      for (let row = 0; row < 5; row++) for (let col = 0; col < 3; col++) if (gl[row * 3 + col] === '1') for (let dy = 0; dy < 2; dy++) for (let dx = 0; dx < 2; dx++) put(x + ci * 8 + col * 2 + dx, y + row * 2 + dy, r, g, b);
    }
  };
  const lab = (v) => String(Math.round(v));
  for (let gx = Math.ceil(box[0] / gridStep) * gridStep; gx <= box[2]; gx += gridStep) text(lab(gx), Math.round((gx - box[0]) * sx) + 3, 3, 40, 80, 160);
  for (let gy = Math.ceil(box[1] / gridStep) * gridStep; gy <= box[3]; gy += gridStep) {
    const py = Math.round((box[3] - gy) * sy) + 3;
    if (py > 16) text(lab(gy), 3, py, 160, 70, 40); // the top-left corner belongs to the first x label
  }

  return { png: encodePng(W, H, rgb), box, width: W, height: H, scale: 1 / sx, gridStep, segments };
}

module.exports = { renderPng, encodePng };
