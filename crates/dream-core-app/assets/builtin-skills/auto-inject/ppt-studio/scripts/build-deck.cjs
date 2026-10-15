#!/usr/bin/env node
'use strict';
// One Work deck engine: deck.json (content + layout types + theme) -> .pptx
// Usage: node build-deck.cjs <deck.json> [-o out.pptx]
//        node build-deck.cjs --themes | --layouts
// Zero dependencies beyond the vendored pptxgenjs bundle next to this file.

const fs = require('fs');
const path = require('path');
const zlib = require('zlib');
const PptxGenJS = require('./vendor/pptxgenjs.min.cjs');
const { THEMES, CJK_FONT } = require('./themes.cjs');

const W = 13.333;
const H = 7.5;
const M = 0.7; // side margin
const CONTENT_TOP = 1.75;
const CONTENT_BOTTOM = 6.8;

// ---------------------------------------------------------------- text fitting

const CJK_RE = /[⺀-鿿가-힯豈-﫿＀-￯　-〿]/;

function textUnits(s, bold) {
  let u = 0;
  for (const ch of String(s)) {
    if (CJK_RE.test(ch)) u += 1.0;
    else if (ch === ' ') u += 0.28;
    else if (/[iljI.,:;'|!()[\]]/.test(ch)) u += 0.3;
    else if (/[%@&MWmw−—]/.test(ch)) u += 0.92;
    else if (/[A-Z]/.test(ch)) u += 0.68;
    else if (/[0-9#$+\-×~<>=]/.test(ch)) u += 0.6;
    else u += 0.54;
  }
  return bold ? u * 1.05 : u;
}

function lineCount(text, widthIn, pt, bold) {
  const perLine = Math.max(1, (widthIn * 72) / pt);
  let lines = 0;
  for (const para of String(text).split('\n')) {
    lines += Math.max(1, Math.ceil((textUnits(para, bold) * 1.03) / perLine));
  }
  return lines;
}

function textHeight(text, widthIn, pt, bold, spacing = 1.2) {
  return (lineCount(text, widthIn, pt, bold) * pt * spacing) / 72;
}

// Largest size in [min, max] at which `text` fits the box; records a warning when even `min` overflows.
function fitSize(ctx, text, w, h, max, min, opts = {}) {
  const spacing = opts.spacing || 1.2;
  const extra = opts.extraLines || 0; // paragraph gaps measured in lines
  for (let pt = max; pt >= min; pt -= 1) {
    const hh = textHeight(text, w, pt, opts.bold, spacing) + (extra * pt * spacing) / 72;
    if (hh <= h) return pt;
  }
  ctx.warn(`文字过多：「${String(text).slice(0, 24)}…」在 ${min}pt 仍放不下，请精简`);
  return min;
}

// ---------------------------------------------------------------- gradient backgrounds

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function pngChunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}

const hexRgb = (h) => [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
const BAYER = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

function gradientPng(base, blobs, w = 960, h = 540) {
  const b = hexRgb(base);
  const bl = blobs.map((g) => ({ ...g, rgb: hexRgb(g.c) }));
  const raw = Buffer.alloc((w * 3 + 1) * h);
  let o = 0;
  for (let y = 0; y < h; y++) {
    raw[o++] = 0;
    for (let x = 0; x < w; x++) {
      let r = b[0], g = b[1], bb = b[2];
      for (const p of bl) {
        const dx = x / w - p.x;
        const dy = (y / h - p.y) * (h / w);
        const d = Math.sqrt(dx * dx + dy * dy) / (p.r * 0.75);
        if (d >= 1) continue;
        const f = p.a * (1 - d) * (1 - d) * (3 - 2 * (1 - d)) * 0.5 + p.a * (1 - d) * (1 - d) * 0.5;
        r += (p.rgb[0] - r) * f;
        g += (p.rgb[1] - g) * f;
        bb += (p.rgb[2] - bb) * f;
      }
      const dither = BAYER[(y & 3) * 4 + (x & 3)] / 16 - 0.5;
      raw[o++] = Math.max(0, Math.min(255, Math.round(r + dither)));
      raw[o++] = Math.max(0, Math.min(255, Math.round(g + dither)));
      raw[o++] = Math.max(0, Math.min(255, Math.round(bb + dither)));
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 2;
  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk('IHDR', ihdr),
    pngChunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
    pngChunk('IEND', Buffer.alloc(0)),
  ]);
  return 'data:image/png;base64,' + png.toString('base64');
}

// ---------------------------------------------------------------- helpers

function asText(v) {
  if (v == null) return '';
  if (typeof v === 'string') return v;
  if (typeof v === 'number') return String(v);
  if (typeof v === 'object' && v.text != null) return String(v.text);
  return String(v);
}

function resolveImage(ctx, p) {
  if (!p) return null;
  if (/^https?:\/\//i.test(p)) {
    ctx.warn(`图片不支持网络地址，请先下载到本地：${p}`);
    return null;
  }
  const abs = path.isAbsolute(p) ? p : path.resolve(ctx.baseDir, p);
  if (!fs.existsSync(abs)) {
    ctx.warn(`图片不存在，已跳过：${p}`);
    return null;
  }
  return abs;
}

function header(ctx, slide, s) {
  const t = ctx.t;
  let y = 0.5;
  if (s.kicker) {
    slide.addShape(ctx.pptx.ShapeType.rect, { x: M, y: y + 0.05, w: 0.06, h: 0.22, fill: { color: t.accent }, line: { type: 'none' } });
    slide.addText(s.kicker, { x: M + 0.16, y, w: 8, h: 0.32, fontFace: ctx.font.body, fontSize: 12, bold: true, color: t.accent, charSpacing: 2, margin: 0, valign: 'middle' });
    y += 0.38;
  } else {
    y += 0.12;
  }
  const titleW = W - 2 * M;
  const tpt = fitSize(ctx, s.title || '', titleW, 0.62, 30, 22, { bold: true });
  slide.addText(s.title || '', { x: M, y, w: titleW, h: 0.62, fontFace: ctx.font.head, fontSize: tpt, bold: true, color: t.text, margin: 0, valign: 'middle', fit: 'shrink' });
  y += 0.66;
  if (s.subtitle) {
    slide.addText(s.subtitle, { x: M, y, w: titleW, h: 0.38, fontFace: ctx.font.body, fontSize: 14, color: t.muted, margin: 0, valign: 'top', fit: 'shrink' });
    y += 0.42;
  }
  return Math.max(CONTENT_TOP, y + 0.18);
}

function footer(ctx, slide) {
  const t = ctx.t;
  slide.addShape(ctx.pptx.ShapeType.line, { x: M, y: 7.02, w: W - 2 * M, h: 0, line: { color: t.line, width: 0.75 } });
  if (ctx.deck.footer) {
    slide.addText(ctx.deck.footer, { x: M, y: 7.08, w: 8, h: 0.26, fontFace: ctx.font.body, fontSize: 9, color: t.muted, margin: 0, valign: 'middle' });
  }
  slide.addText(String(ctx.page), { x: W - M - 1, y: 7.08, w: 1, h: 0.26, fontFace: ctx.font.body, fontSize: 9, color: t.muted, margin: 0, align: 'right', valign: 'middle' });
}

function bodyBackground(ctx, slide) {
  const t = ctx.t;
  if (t.dark && t.bodyGlow) {
    slide.background = { data: ctx.bg('body', t.bg, t.bodyGlow) };
  } else {
    slide.background = { color: t.bg };
  }
}

function heroBackground(ctx, slide) {
  const t = ctx.t;
  slide.background = { data: ctx.bg('hero', t.hero.bg, t.glow) };
}

function roundBox(ctx, slide, x, y, w, h, fill, opts = {}) {
  slide.addShape(ctx.pptx.ShapeType.roundRect, {
    x, y, w, h,
    rectRadius: opts.radius ?? 0.12,
    fill: { color: fill, transparency: opts.transparency || 0 },
    line: opts.line ? { color: opts.line, width: 0.75 } : { type: 'none' },
    shadow: opts.shadow ? { type: 'outer', color: '000000', opacity: 0.12, blur: 8, offset: 2, angle: 90 } : undefined,
  });
}

function badge(ctx, slide, x, y, d, label, fill, color) {
  slide.addShape(ctx.pptx.ShapeType.ellipse, { x, y, w: d, h: d, fill: { color: fill }, line: { type: 'none' } });
  const pt = CJK_RE.test(label) || label.length <= 2 ? Math.round(d * 30) : Math.round(d * 22);
  slide.addText(label, { x, y, w: d, h: d, fontFace: ctx.font.head, fontSize: pt, bold: true, color, align: 'center', valign: 'middle', margin: 0 });
}

const pad2 = (i) => String(i).padStart(2, '0');

// Thin concentric rings: the hero-slide motif that keeps an empty half from looking unfinished.
function rings(ctx, slide, cx, cy, color, scale = 1) {
  [5.6, 4.3, 3.0, 1.8].forEach((r, i) => {
    const rr = r * scale;
    slide.addShape(ctx.pptx.ShapeType.ellipse, { x: cx - rr / 2, y: cy - rr / 2, w: rr, h: rr, fill: { type: 'none' }, line: { color, width: i === 3 ? 2 : 1, transparency: 55 + i * 8 } });
  });
  slide.addShape(ctx.pptx.ShapeType.ellipse, { x: cx - 0.09 * scale + 1.5 * scale, y: cy - 2.08 * scale, w: 0.18 * scale, h: 0.18 * scale, fill: { color }, line: { type: 'none' } });
}

// Vertical start that balances a block of height `need` inside [top, CONTENT_BOTTOM]
// (slightly above centre reads better than dead centre).
const balance = (top, need) => top + Math.max(0, (CONTENT_BOTTOM - top - need) * 0.42);

function flattenBullets(items) {
  const flat = [];
  for (const it of items) {
    flat.push({ text: asText(it), level: 0 });
    if (it && typeof it === 'object' && Array.isArray(it.sub)) for (const s of it.sub) flat.push({ text: asText(s), level: 1 });
  }
  return flat;
}

const subSize = (pt) => Math.max(pt - 3, 11);

// Rendered height of a flattened bullet list at size `pt` (sub-items render smaller and indented).
function bulletsHeight(flat, w, pt) {
  let hh = 0;
  for (const f of flat) {
    const size = f.level === 0 ? pt : subSize(pt);
    hh += (lineCount(f.text, w - 0.35 - f.level * 0.3, size) * size * 1.2 + pt * 0.55) / 72;
  }
  return hh;
}

// Font size and rendered height of a bullet list fitted into a w x h box.
function measureBullets(ctx, items, w, h, max = 20, min = 12) {
  const flat = flattenBullets(items);
  for (let pt = max; pt >= min; pt -= 1) {
    const used = bulletsHeight(flat, w, pt);
    if (used <= h) return { pt, h: used };
  }
  if (flat.length) ctx.warn(`列表文字过多：「${flat[0].text.slice(0, 20)}…」等 ${flat.length} 行在 ${min}pt 仍放不下，请精简或拆页`);
  return { pt: min, h };
}

// Bullet list with custom markers, fitted inside the box.
function bulletList(ctx, slide, items, x, y, w, h, opts = {}) {
  const t = ctx.t;
  const flat = flattenBullets(items);
  const m = measureBullets(opts.pt ? { warn() {} } : ctx, items, w, h, opts.pt || opts.max || 20, opts.pt || opts.min || 12);
  const pt = m.pt;
  // Spread spare height across paragraphs (capped) so short lists don't huddle at the top.
  const spare = Math.max(0, h - m.h) * 72;
  const gapPt = Math.round(pt * 0.55 + Math.min(pt * 0.9, spare / Math.max(flat.length, 1) / 2));
  const runs = [];
  flat.forEach((f, i) => {
    const color = opts.color || t.text;
    runs.push({
      text: f.text,
      options: {
        bullet: f.level === 0 ? { code: '25A0', indent: pt * 1.1 } : { code: '2013', indent: pt * 1.1 },
        indentLevel: f.level,
        fontSize: f.level === 0 ? pt : subSize(pt),
        color: f.level === 0 ? color : opts.mutedColor || t.muted,
        paraSpaceAfter: gapPt,
        breakLine: i < flat.length - 1,
      },
    });
  });
  slide.addText(runs, { x, y, w, h, fontFace: ctx.font.body, valign: opts.valign || 'top', margin: 0, lineSpacingMultiple: 1.05, color: t.text });
  // Recolor bullet glyphs: pptxgenjs uses the run color for markers, so overlay is not needed.
}

// ---------------------------------------------------------------- layouts

const LAYOUTS = {};

LAYOUTS.cover = (ctx, slide, s) => {
  const t = ctx.t.hero;
  heroBackground(ctx, slide);
  const img = resolveImage(ctx, s.image);
  const textW = img ? 6.6 : 10.4;
  if (img) {
    slide.addImage({ path: img, x: 7.75, y: 0, w: W - 7.75, h: H, sizing: { type: 'cover', w: W - 7.75, h: H } });
  } else {
    rings(ctx, slide, 10.9, 3.4, t.accent);
  }
  let y = 1.9;
  if (s.kicker) {
    slide.addText(s.kicker, { x: M + 0.05, y, w: textW, h: 0.4, fontFace: ctx.font.body, fontSize: 14, bold: true, color: t.accent, charSpacing: 3, margin: 0 });
    y += 0.55;
  }
  const tpt = fitSize(ctx, s.title || '', textW, 1.9, 48, 30, { bold: true, spacing: 1.1 });
  const th = Math.min(1.95, textHeight(s.title || '', textW, tpt, true, 1.1) + 0.1);
  slide.addText(s.title || '', { x: M, y, w: textW, h: th, fontFace: ctx.font.head, fontSize: tpt, bold: true, color: t.text, margin: 0, valign: 'top', lineSpacingMultiple: 1.05 });
  y += th + 0.2;
  slide.addShape(ctx.pptx.ShapeType.rect, { x: M + 0.03, y, w: 1.1, h: 0.06, fill: { color: t.accent }, line: { type: 'none' } });
  y += 0.32;
  if (s.subtitle) {
    const spt = fitSize(ctx, s.subtitle, textW, 1.0, 20, 14);
    slide.addText(s.subtitle, { x: M, y, w: textW, h: 1.0, fontFace: ctx.font.body, fontSize: spt, color: t.muted, margin: 0, valign: 'top' });
  }
  const meta = [s.author, s.date].filter(Boolean).join('   ·   ');
  if (meta) slide.addText(meta, { x: M, y: 6.45, w: textW, h: 0.35, fontFace: ctx.font.body, fontSize: 12, color: t.muted, margin: 0 });
};

LAYOUTS.agenda = (ctx, slide, s) => {
  const t = ctx.t;
  bodyBackground(ctx, slide);
  const panelW = 4.1;
  slide.addShape(ctx.pptx.ShapeType.rect, { x: 0, y: 0, w: panelW, h: H, fill: { color: t.hero.bg }, line: { type: 'none' } });
  slide.addText(s.title || (ctx.cjk ? '目录' : 'Agenda'), { x: M, y: 2.6, w: panelW - M - 0.3, h: 0.9, fontFace: ctx.font.head, fontSize: 40, bold: true, color: t.hero.text, margin: 0 });
  slide.addText(s.kicker || 'CONTENTS', { x: M, y: 3.5, w: panelW - M - 0.3, h: 0.4, fontFace: ctx.font.body, fontSize: 14, color: t.hero.accent, charSpacing: 4, margin: 0 });
  const items = (s.items || []).map(asText);
  const n = Math.max(1, items.length);
  const cols = n > 6 ? 2 : 1;
  const perCol = Math.ceil(n / cols);
  const x0 = panelW + 0.8;
  const colW = (W - x0 - M - (cols - 1) * 0.4) / cols;
  const top = 1.0;
  const rowH = Math.min(1.0, (H - 2 * top) / perCol);
  items.forEach((txt, i) => {
    const c = Math.floor(i / perCol);
    const r = i % perCol;
    const x = x0 + c * (colW + 0.4);
    const y = top + r * rowH + (H - 2 * top - perCol * rowH) / 2;
    slide.addText(pad2(i + 1), { x, y, w: 0.9, h: rowH * 0.8, fontFace: ctx.font.head, fontSize: 26, bold: true, color: t.accent, margin: 0, valign: 'middle' });
    const pt = fitSize(ctx, txt, colW - 1.0, rowH * 0.8, 20, 13);
    slide.addText(txt, { x: x + 1.0, y, w: colW - 1.0, h: rowH * 0.8, fontFace: ctx.font.body, fontSize: pt, color: t.text, margin: 0, valign: 'middle' });
    if (r < perCol - 1 && i < n - 1) slide.addShape(ctx.pptx.ShapeType.line, { x, y: y + rowH * 0.9, w: colW, h: 0, line: { color: t.line, width: 0.75 } });
  });
};

LAYOUTS.section = (ctx, slide, s) => {
  const t = ctx.t.hero;
  heroBackground(ctx, slide);
  const no = s.no != null ? String(s.no) : pad2(ctx.sectionNo);
  rings(ctx, slide, 11.2, 3.75, t.accent, 0.85);
  slide.addText(no, { x: M - 0.1, y: 1.1, w: 6, h: 2.4, fontFace: ctx.font.head, fontSize: 150, bold: true, color: t.accent, transparency: 25, margin: 0, valign: 'bottom' });
  slide.addShape(ctx.pptx.ShapeType.rect, { x: M, y: 3.85, w: 1.1, h: 0.06, fill: { color: t.accent }, line: { type: 'none' } });
  const tpt = fitSize(ctx, s.title || '', 10.5, 1.3, 42, 28, { bold: true });
  slide.addText(s.title || '', { x: M, y: 4.1, w: 10.5, h: 1.3, fontFace: ctx.font.head, fontSize: tpt, bold: true, color: t.text, margin: 0, valign: 'top' });
  if (s.subtitle) slide.addText(s.subtitle, { x: M, y: 5.45, w: 10.5, h: 0.8, fontFace: ctx.font.body, fontSize: 18, color: t.muted, margin: 0, valign: 'top', fit: 'shrink' });
};

LAYOUTS.bullets = (ctx, slide, s) => {
  const t = ctx.t;
  bodyBackground(ctx, slide);
  const top = header(ctx, slide, s);
  const img = resolveImage(ctx, s.image);
  const h = CONTENT_BOTTOM - top;
  let textW = W - 2 * M;
  if (img) {
    const iw = 5.2;
    slide.addImage({ path: img, x: W - M - iw, y: top, w: iw, h, sizing: { type: 'cover', w: iw, h } });
    textW -= iw + 0.5;
  } else if (s.highlight) {
    const hw = 4.2;
    const hx = W - M - hw;
    roundBox(ctx, slide, hx, top, hw, h, t.hero.bg, { radius: 0.14 });
    const hl = s.highlight;
    const hlText = asText(hl.text);
    const hlPt = hlText ? fitSize(ctx, hlText, hw - 0.8, h - 2.8, 16, 11) : 0;
    const need = (hl.value ? 1.3 : 0) + (hl.title ? 0.65 : 0) + (hlText ? textHeight(hlText, hw - 0.8, hlPt) + 0.1 : 0);
    let yy = top + Math.max(0.45, (h - need) * 0.42);
    if (hl.value) {
      slide.addText(asText(hl.value), { x: hx + 0.4, y: yy, w: hw - 0.8, h: 1.2, fontFace: ctx.font.head, fontSize: fitSize(ctx, asText(hl.value), hw - 0.8, 1.2, 60, 30, { bold: true }), bold: true, color: t.hero.accent, margin: 0, valign: 'middle' });
      yy += 1.3;
    }
    if (hl.title) {
      slide.addText(hl.title, { x: hx + 0.4, y: yy, w: hw - 0.8, h: 0.6, fontFace: ctx.font.head, fontSize: 18, bold: true, color: t.hero.text, margin: 0, valign: 'top', fit: 'shrink' });
      yy += 0.65;
    }
    if (hlText) {
      const hh = top + h - 0.4 - yy;
      slide.addText(hlText, { x: hx + 0.4, y: yy, w: hw - 0.8, h: hh, fontFace: ctx.font.body, fontSize: hlPt, color: t.hero.muted, margin: 0, valign: 'top' });
    }
    textW -= hw + 0.5;
  }
  const items = s.bullets || s.items || [];
  const cols = !img && !s.highlight && items.length > 6 ? 2 : 1;
  if (cols === 2) {
    const half = Math.ceil(items.length / 2);
    const cw = (textW - 0.6) / 2;
    bulletList(ctx, slide, items.slice(0, half), M, top + 0.1, cw, h - 0.1);
    bulletList(ctx, slide, items.slice(half), M + cw + 0.6, top + 0.1, cw, h - 0.1);
  } else {
    bulletList(ctx, slide, items, M, top + 0.1, textW, h - 0.1, { max: 24 });
  }
};

LAYOUTS.cards = (ctx, slide, s) => {
  const t = ctx.t;
  bodyBackground(ctx, slide);
  const top = header(ctx, slide, s);
  const cards = s.cards || s.items || [];
  const n = cards.length || 1;
  const cols = n <= 4 ? n : n === 5 || n === 6 ? 3 : 4;
  const rows = Math.ceil(n / cols);
  const gap = 0.3;
  const cw = (W - 2 * M - (cols - 1) * gap) / cols;
  const avail = (CONTENT_BOTTOM - top - (rows - 1) * gap) / rows;
  const tw = cw - 0.7;
  const d = rows > 1 ? 0.55 : 0.7;
  const padTop = rows > 1 ? 0.35 : 0.5;
  const titles = cards.map((c) => asText(c.title));
  const bodies = cards.map((c) => asText(c.text));
  const tpt = Math.min(...titles.map((x) => fitSize(ctx, x, tw, 0.85, rows > 1 ? 18 : 20, 13, { bold: true })));
  const th = Math.min(0.9, Math.max(...titles.map((x) => textHeight(x, tw, tpt, true))) + 0.05);
  const bodyRoom = avail - padTop - d - 0.25 - th - 0.15 - 0.35;
  const bpt = Math.min(...bodies.filter(Boolean).map((x) => fitSize(ctx, x, tw, bodyRoom, rows > 1 ? 15 : 17, 10.5, { spacing: 1.3 })), 17);
  const bh = Math.min(bodyRoom, Math.max(0, ...bodies.map((x) => (x ? textHeight(x, tw, bpt, false, 1.3) : 0))) + 0.1);
  const ch = Math.min(avail, Math.max(padTop + d + 0.25 + th + 0.15 + bh + 0.4, avail * 0.5));
  const y0 = balance(top, rows * ch + (rows - 1) * gap);
  cards.forEach((c, i) => {
    const col = i % cols;
    const row = Math.floor(i / cols);
    const x = M + col * (cw + gap);
    const y = y0 + row * (ch + gap);
    const color = ctx.t.chart[i % ctx.t.chart.length];
    roundBox(ctx, slide, x, y, cw, ch, t.surface, { radius: 0.12 });
    slide.addShape(ctx.pptx.ShapeType.rect, { x: x + 0.35, y, w: 0.9, h: 0.07, fill: { color }, line: { type: 'none' } });
    let yy = y + padTop;
    badge(ctx, slide, x + 0.35, yy, d, asText(c.icon) || pad2(i + 1), color, t.dark ? '0B1020' : 'FFFFFF');
    yy += d + 0.25;
    slide.addText(titles[i], { x: x + 0.35, y: yy, w: tw, h: th, fontFace: ctx.font.head, fontSize: tpt, bold: true, color: t.text, margin: 0, valign: 'top' });
    yy += th + 0.15;
    if (bodies[i] && bh > 0.2) {
      slide.addText(bodies[i], { x: x + 0.35, y: yy, w: tw, h: bh, fontFace: ctx.font.body, fontSize: bpt, color: t.muted, margin: 0, valign: 'top', lineSpacingMultiple: 1.15 });
    }
  });
};

LAYOUTS.stats = (ctx, slide, s) => {
  const t = ctx.t;
  bodyBackground(ctx, slide);
  const top = header(ctx, slide, s);
  const stats = s.stats || s.items || [];
  const n = stats.length || 1;
  const bottom = s.summary ? CONTENT_BOTTOM - 1.2 : CONTENT_BOTTOM;
  const gap = 0.3;
  const cw = (W - 2 * M - (n - 1) * gap) / n;
  const h = Math.min(4.2, bottom - top);
  const y = top + (bottom - top - h) / 2;
  const valPt = Math.min(...stats.map((st) => fitSize({ warn() {} }, asText(st.value), cw - 0.6, 1.3, n <= 3 ? 66 : 54, 30, { bold: true })));
  stats.forEach((st, i) => {
    const x = M + i * (cw + gap);
    roundBox(ctx, slide, x, y, cw, h, t.surface, { radius: 0.12 });
    const color = ctx.t.chart[i % ctx.t.chart.length];
    slide.addText(asText(st.value), { x: x + 0.3, y: y + 0.4, w: cw - 0.6, h: 1.4, fontFace: ctx.font.head, fontSize: valPt, bold: true, color, margin: 0, valign: 'middle' });
    slide.addShape(ctx.pptx.ShapeType.rect, { x: x + 0.32, y: y + 1.95, w: 0.6, h: 0.05, fill: { color }, line: { type: 'none' } });
    const label = asText(st.label);
    slide.addText(label, { x: x + 0.3, y: y + 2.15, w: cw - 0.6, h: 0.7, fontFace: ctx.font.head, fontSize: fitSize(ctx, label, cw - 0.6, 0.7, 18, 12, { bold: true }), bold: true, color: t.text, margin: 0, valign: 'top' });
    const detail = asText(st.detail);
    const dh = y + h - 0.3 - (y + 2.9);
    if (detail && dh > 0.2) slide.addText(detail, { x: x + 0.3, y: y + 2.9, w: cw - 0.6, h: dh, fontFace: ctx.font.body, fontSize: fitSize(ctx, detail, cw - 0.6, dh, 14, 10), color: t.muted, margin: 0, valign: 'top' });
  });
  if (s.summary) {
    const sy = CONTENT_BOTTOM - 0.95;
    roundBox(ctx, slide, M, sy, W - 2 * M, 0.95, t.hero.bg, { radius: 0.1 });
    slide.addText(s.summary, { x: M + 0.4, y: sy, w: W - 2 * M - 0.8, h: 0.95, fontFace: ctx.font.body, fontSize: fitSize(ctx, s.summary, W - 2 * M - 0.8, 0.85, 17, 12), color: t.hero.text, margin: 0, valign: 'middle' });
  }
};

const CHART_KIND = {
  bar: ['bar', { barDir: 'col' }],
  column: ['bar', { barDir: 'col' }],
  'bar-h': ['bar', { barDir: 'bar' }],
  stacked: ['bar', { barDir: 'col', barGrouping: 'stacked' }],
  'stacked-h': ['bar', { barDir: 'bar', barGrouping: 'stacked' }],
  line: ['line', {}],
  area: ['area', {}],
  pie: ['pie', {}],
  doughnut: ['doughnut', {}],
  radar: ['radar', {}],
};

function addChart(ctx, slide, chart, x, y, w, h) {
  const t = ctx.t;
  const kind = CHART_KIND[chart.kind || chart.type || 'bar'];
  if (!kind) throw new Error(`未知图表类型 "${chart.kind}"，可用：${Object.keys(CHART_KIND).join(', ')}`);
  const [ct, extra] = kind;
  const cats = (chart.categories || chart.labels || []).map(asText);
  const series = (chart.series || []).map((se) => ({ name: asText(se.name), labels: cats, values: (se.values || []).map(Number) }));
  if (!series.length) throw new Error('chart.series 不能为空');
  const pie = ct === 'pie' || ct === 'doughnut';
  const axisColor = t.muted;
  const opts = {
    x, y, w, h,
    ...extra,
    chartColors: pie ? t.chart : t.chart.slice(0, Math.max(series.length, 1)),
    showLegend: pie || series.length > 1,
    legendPos: pie ? 'r' : 'b',
    legendFontSize: pie ? 13 : 12,
    legendColor: axisColor,
    legendFontFace: ctx.font.body,
    showTitle: false,
    catAxisLabelColor: axisColor,
    valAxisLabelColor: axisColor,
    catAxisLabelFontSize: 11,
    valAxisLabelFontSize: 10,
    catAxisLabelFontFace: ctx.font.body,
    valAxisLabelFontFace: ctx.font.body,
    catAxisLineShow: true,
    catAxisLineColor: t.line,
    valAxisLineShow: false,
    valGridLine: { color: t.line, style: 'dash', size: 0.5 },
    catGridLine: { style: 'none' },
    dataLabelColor: pie ? 'FFFFFF' : t.text,
    dataLabelFontSize: pie ? 12 : 11,
    dataLabelFontFace: ctx.font.body,
    showValue: chart.showValues ?? (!pie && series.length === 1 && cats.length <= 12),
    showPercent: pie,
    dataLabelPosition: pie ? 'bestFit' : ct === 'bar' && !extra.barGrouping ? 'outEnd' : undefined,
    barGapWidthPct: 60,
    lineSize: 3,
    lineDataSymbol: 'circle',
    lineDataSymbolSize: 7,
    holeSize: 58,
    valAxisHidden: !!chart.hideAxis,
    plotArea: { fill: { color: t.bg, transparency: 100 } },
  };
  if (chart.unit) opts.valAxisLabelFormatCode = `#,##0"${chart.unit}"`;
  if (chart.format) opts.dataLabelFormatCode = chart.format;
  slide.addChart(ctx.pptx.ChartType[ct], series, opts);
}

LAYOUTS.chart = (ctx, slide, s) => {
  const t = ctx.t;
  bodyBackground(ctx, slide);
  const top = header(ctx, slide, s);
  const insights = (s.insights || []).map(asText);
  const h = CONTENT_BOTTOM - top;
  const chartW = insights.length ? 7.9 : W - 2 * M;
  roundBox(ctx, slide, M, top, chartW, h, t.surface, { radius: 0.1 });
  addChart(ctx, slide, s.chart || {}, M + 0.2, top + 0.2, chartW - 0.4, h - 0.4);
  if (insights.length) {
    const x = M + chartW + 0.45;
    const w = W - M - x;
    slide.addText(s.insightsTitle || (ctx.cjk ? '关键洞察' : 'Key insights'), { x, y: top, w, h: 0.45, fontFace: ctx.font.head, fontSize: 16, bold: true, color: t.accent, margin: 0, valign: 'top' });
    const each = (h - 0.6) / insights.length;
    const pt = Math.min(...insights.map((ins) => fitSize(ctx, ins, w - 0.6, each - 0.15, 15, 10.5, { spacing: 1.25 })));
    insights.forEach((ins, i) => {
      const y = top + 0.6 + i * each;
      badge(ctx, slide, x, y + 0.02, 0.38, String(i + 1), t.chart[i % t.chart.length], t.dark ? '0B1020' : 'FFFFFF');
      slide.addText(ins, { x: x + 0.55, y, w: w - 0.55, h: each - 0.1, fontFace: ctx.font.body, fontSize: pt, color: t.text, margin: 0, valign: 'top', lineSpacingMultiple: 1.1 });
    });
  }
};

LAYOUTS.table = (ctx, slide, s) => {
  const t = ctx.t;
  bodyBackground(ctx, slide);
  const top = header(ctx, slide, s);
  const cols = (s.columns || []).map(asText);
  const rows = (s.rows || []).map((r) => r.map(asText));
  const nCols = cols.length || (rows[0] || []).length || 1;
  const w = W - 2 * M;
  const maxH = CONTENT_BOTTOM - top;
  const rowCount = rows.length + 1;
  const rowH = Math.min(0.72, maxH / rowCount);
  const longest = Math.max(...rows.flat().concat(cols).map((c) => textUnits(c)), 1);
  let pt = rowH >= 0.65 ? 16 : rowH >= 0.5 ? 14 : rowH >= 0.4 ? 12 : 10;
  const colW = w / nCols;
  while (pt > 10 && (longest * pt) / 72 > (colW - 0.2) * 2) pt -= 1;
  const hl = s.highlight;
  const cell = (text, opts) => ({ text, options: opts });
  const data = [];
  if (cols.length) data.push(cols.map((c) => cell(c, { bold: true, color: 'FFFFFF', fill: { color: t.primary === '111111' || t.dark ? t.hero.bg : t.primary }, fontSize: pt })));
  rows.forEach((r, i) => {
    data.push(r.map((c, j) => cell(c, {
      color: j === hl ? t.accent : t.text,
      bold: j === hl || j === 0,
      fill: { color: i % 2 ? t.surface : t.bg },
      fontSize: pt,
    })));
  });
  slide.addTable(data, {
    x: M, y: top, w, colW: Array(nCols).fill(colW), rowH,
    fontFace: ctx.font.body, valign: 'middle', align: 'left', margin: [0.04, 0.15, 0.04, 0.15],
    border: { type: 'solid', pt: 0.5, color: t.line },
    autoPage: false,
  });
  if (rows.length * rowH + rowH > maxH + 0.01) ctx.warn('表格行数过多，建议拆成两页');
};

LAYOUTS.timeline = (ctx, slide, s) => {
  const t = ctx.t;
  bodyBackground(ctx, slide);
  const top = header(ctx, slide, s);
  const steps = s.steps || s.items || [];
  const n = Math.max(steps.length, 1);
  if (n > 7) ctx.warn('时间线超过 7 个节点，建议合并或拆页');
  const x0 = M + 0.2;
  const span = W - 2 * M - 0.4;
  const seg = span / n;
  const bw = seg - 0.16;
  const tw = bw - 0.4;
  const titles = steps.map((st) => asText(st.title));
  const texts = steps.map((st) => asText(st.text));
  const room = CONTENT_BOTTOM - top - 1.7;
  const tpt = Math.min(...titles.map((x) => fitSize(ctx, x, tw, 0.8, 19, 11, { bold: true })));
  const th = Math.min(0.85, Math.max(...titles.map((x) => textHeight(x, tw, tpt, true))) + 0.05);
  const bpt = Math.min(...texts.filter(Boolean).map((x) => fitSize(ctx, x, tw, room - th - 0.75, 15, 9.5, { spacing: 1.3 })), 15);
  const bodyH = Math.max(0, ...texts.map((x) => (x ? textHeight(x, tw, bpt, false, 1.3) : 0))) + 0.1;
  const boxH = Math.min(room, Math.max(1.6, 0.3 + th + 0.15 + bodyH + 0.35));
  const lineY = balance(top, 1.7 + boxH) + 1.2;
  slide.addShape(ctx.pptx.ShapeType.line, { x: x0, y: lineY, w: span, h: 0, line: { color: t.line, width: 2.5 } });
  steps.forEach((st, i) => {
    const cx = x0 + seg * i + seg / 2;
    const color = t.chart[i % t.chart.length];
    slide.addShape(ctx.pptx.ShapeType.ellipse, { x: cx - 0.17, y: lineY - 0.17, w: 0.34, h: 0.34, fill: { color }, line: { color: t.bg, width: 3 } });
    const label = asText(st.label || st.time || st.date);
    slide.addText(label, { x: cx - seg / 2 + 0.05, y: lineY - 0.95, w: seg - 0.1, h: 0.6, fontFace: ctx.font.head, fontSize: fitSize(ctx, label, seg - 0.1, 0.6, 24, 12, { bold: true }), bold: true, color, align: 'center', valign: 'bottom', margin: 0 });
    const bx = cx - seg / 2 + 0.08;
    const by = lineY + 0.45;
    roundBox(ctx, slide, bx, by, bw, boxH, t.surface, { radius: 0.1 });
    slide.addShape(ctx.pptx.ShapeType.rect, { x: bx + 0.2, y: by, w: 0.6, h: 0.06, fill: { color }, line: { type: 'none' } });
    slide.addText(titles[i], { x: bx + 0.2, y: by + 0.3, w: tw, h: th, fontFace: ctx.font.head, fontSize: tpt, bold: true, color: t.text, margin: 0, valign: 'top' });
    if (texts[i]) slide.addText(texts[i], { x: bx + 0.2, y: by + 0.3 + th + 0.15, w: tw, h: boxH - th - 0.6, fontFace: ctx.font.body, fontSize: bpt, color: t.muted, margin: 0, valign: 'top', lineSpacingMultiple: 1.12 });
  });
};

LAYOUTS.process = (ctx, slide, s) => {
  const t = ctx.t;
  bodyBackground(ctx, slide);
  const top = header(ctx, slide, s);
  const steps = s.steps || s.items || [];
  const n = Math.max(steps.length, 1);
  const gap = 0.08;
  const cw = (W - 2 * M - (n - 1) * gap) / n;
  const arrowH = 1.15;
  const titles = steps.map((st) => asText(st.title));
  const texts = steps.map((st) => asText(st.text));
  const tw = cw - 0.5;
  const room = CONTENT_BOTTOM - top - arrowH - 1.1;
  const tpt = Math.min(...titles.map((x) => fitSize(ctx, x, cw - 0.6, arrowH - 0.15, 19, 11, { bold: true })));
  const bpt = Math.min(...texts.filter(Boolean).map((x) => fitSize(ctx, x, tw, room, 16, 10, { spacing: 1.3 })), 16);
  const bodyH = Math.min(room, Math.max(0, ...texts.map((x) => (x ? textHeight(x, tw, bpt, false, 1.3) : 0))) + 0.15);
  const y0 = balance(top, arrowH + 1.1 + bodyH);
  steps.forEach((st, i) => {
    const x = M + i * (cw + gap);
    const color = t.chart[i % t.chart.length];
    slide.addShape(i === 0 ? ctx.pptx.ShapeType.homePlate : ctx.pptx.ShapeType.chevron, { x, y: y0, w: cw + (i < n - 1 ? 0.25 : 0), h: arrowH, fill: { color }, line: { type: 'none' } });
    slide.addText(titles[i], { x: x + 0.35, y: y0, w: cw - 0.6, h: arrowH, fontFace: ctx.font.head, fontSize: tpt, bold: true, color: t.dark ? '0B1020' : 'FFFFFF', margin: 0, valign: 'middle', align: 'center' });
    slide.addText(pad2(i + 1), { x, y: y0 + arrowH + 0.3, w: cw, h: 0.55, fontFace: ctx.font.head, fontSize: 26, bold: true, color, margin: 0, align: 'center' });
    if (texts[i]) slide.addText(texts[i], { x: x + 0.25, y: y0 + arrowH + 1.0, w: tw, h: bodyH, fontFace: ctx.font.body, fontSize: bpt, color: t.muted, margin: 0, valign: 'top', align: 'center', lineSpacingMultiple: 1.12 });
  });
};

LAYOUTS.compare = (ctx, slide, s) => {
  const t = ctx.t;
  bodyBackground(ctx, slide);
  const top = header(ctx, slide, s);
  const gap = 0.9;
  const cw = (W - 2 * M - gap) / 2;
  const avail = CONTENT_BOTTOM - top;
  const sides = [s.left || {}, s.right || {}];
  const lists = sides.map((side) => side.points || side.items || []);
  const pt = Math.min(...lists.map((l) => measureBullets(ctx, l, cw - 0.9, avail - 1.75, 20, 11).pt));
  const listH = Math.max(...lists.map((l) => measureBullets({ warn() {} }, l, cw - 0.9, avail - 1.75, pt, pt).h));
  const h = Math.min(avail, Math.max(avail * 0.6, 1.35 + listH + 0.5));
  const y0 = balance(top, h);
  sides.forEach((side, i) => {
    const x = M + i * (cw + gap);
    const strong = i === 1;
    roundBox(ctx, slide, x, y0, cw, h, strong ? t.hero.bg : t.surface, { radius: 0.14 });
    const title = asText(side.title);
    slide.addText(title, { x: x + 0.45, y: y0 + 0.35, w: cw - 0.9, h: 0.6, fontFace: ctx.font.head, fontSize: fitSize(ctx, title, cw - 0.9, 0.6, 24, 14, { bold: true }), bold: true, color: strong ? t.hero.accent : t.text, margin: 0, valign: 'middle' });
    slide.addShape(ctx.pptx.ShapeType.line, { x: x + 0.45, y: y0 + 1.1, w: cw - 0.9, h: 0, line: { color: strong ? t.hero.muted : t.line, width: 0.75, transparency: strong ? 50 : 0 } });
    bulletList(ctx, slide, lists[i], x + 0.45, y0 + 1.35, cw - 0.9, h - 1.6, {
      pt, color: strong ? t.hero.text : t.text, mutedColor: strong ? t.hero.muted : t.muted,
    });
  });
  const d = 0.8;
  badge(ctx, slide, W / 2 - d / 2, y0 + h / 2 - d / 2, d, s.vs || 'VS', t.accent, 'FFFFFF');
};

LAYOUTS.matrix = (ctx, slide, s) => {
  const t = ctx.t;
  bodyBackground(ctx, slide);
  const top = header(ctx, slide, s);
  const q = (s.quadrants || s.items || []).slice(0, 4);
  const gap = 0.25;
  const cw = (W - 2 * M - gap) / 2;
  const ch = (CONTENT_BOTTOM - top - gap) / 2;
  q.forEach((qq, i) => {
    const x = M + (i % 2) * (cw + gap);
    const y = top + Math.floor(i / 2) * (ch + gap);
    const color = t.chart[i % t.chart.length];
    roundBox(ctx, slide, x, y, cw, ch, t.surface, { radius: 0.1 });
    const tag = asText(qq.tag) || asText(qq.title).slice(0, 1);
    badge(ctx, slide, x + 0.3, y + 0.28, 0.62, tag, color, t.dark ? '0B1020' : 'FFFFFF');
    slide.addText(asText(qq.title), { x: x + 1.1, y: y + 0.28, w: cw - 1.4, h: 0.62, fontFace: ctx.font.head, fontSize: 18, bold: true, color: t.text, margin: 0, valign: 'middle', fit: 'shrink' });
    bulletList(ctx, slide, qq.points || qq.items || [], x + 0.35, y + 1.15, cw - 0.7, ch - 1.35, { max: 18, min: 10 });
  });
};

LAYOUTS.funnel = (ctx, slide, s) => {
  const t = ctx.t;
  bodyBackground(ctx, slide);
  const top = header(ctx, slide, s);
  const st = s.stages || s.items || [];
  const n = Math.max(st.length, 1);
  const h = CONTENT_BOTTOM - top;
  const rowH = Math.min(1.15, h / n);
  const maxW = 8.0;
  const minW = 3.0;
  const cx = M + maxW / 2 + 0.2;
  const y0 = balance(top, rowH * n);
  st.forEach((stg, i) => {
    const w = maxW - ((maxW - minW) * i) / Math.max(n - 1, 1);
    const y = y0 + i * rowH;
    const color = t.chart[i % t.chart.length];
    roundBox(ctx, slide, cx - w / 2, y + 0.06, w, rowH - 0.12, color, { radius: 0.08 });
    const label = asText(stg.label || stg.title);
    slide.addText(label, { x: cx - w / 2, y: y + 0.06, w, h: rowH - 0.12, fontFace: ctx.font.head, fontSize: fitSize(ctx, label, w - 0.3, rowH - 0.2, 18, 11, { bold: true }), bold: true, color: t.dark ? '0B1020' : 'FFFFFF', align: 'center', valign: 'middle', margin: 0 });
    const right = [asText(stg.value), asText(stg.text)].filter(Boolean).join('  ');
    if (right) {
      const rx = M + maxW + 0.8;
      slide.addShape(ctx.pptx.ShapeType.line, { x: cx + w / 2 + 0.1, y: y + rowH / 2, w: rx - (cx + w / 2) - 0.2, h: 0, line: { color: t.line, width: 1, dashType: 'dash' } });
      slide.addText([
        { text: asText(stg.value), options: { bold: true, color, fontSize: 26, breakLine: !!stg.text } },
        ...(stg.text ? [{ text: asText(stg.text), options: { color: t.muted, fontSize: 14 } }] : []),
      ], { x: rx, y, w: W - M - rx, h: rowH, fontFace: ctx.font.body, margin: 0, valign: 'middle', fit: 'shrink' });
    }
  });
};

LAYOUTS.quote = (ctx, slide, s) => {
  const t = ctx.t.hero;
  heroBackground(ctx, slide);
  slide.addText('“', { x: M, y: 0.6, w: 2, h: 2, fontFace: 'Georgia', fontSize: 160, bold: true, color: t.accent, margin: 0, valign: 'top' });
  const q = asText(s.quote || s.text || s.title);
  const pt = fitSize(ctx, q, W - 2 * M - 1.2, 3.2, 34, 20, { spacing: 1.3 });
  slide.addText(q, { x: M + 0.6, y: 2.2, w: W - 2 * M - 1.2, h: 3.2, fontFace: ctx.font.head, fontSize: pt, color: t.text, margin: 0, valign: 'middle', lineSpacingMultiple: 1.2 });
  if (s.author) slide.addText(`— ${s.author}`, { x: M + 0.6, y: 5.65, w: W - 2 * M - 1.2, h: 0.5, fontFace: ctx.font.body, fontSize: 16, color: t.muted, margin: 0 });
};

LAYOUTS['image-text'] = (ctx, slide, s) => {
  const t = ctx.t;
  bodyBackground(ctx, slide);
  const img = resolveImage(ctx, s.image);
  const left = s.imageSide === 'left';
  const iw = W * 0.46;
  const ix = left ? 0 : W - iw;
  if (img) slide.addImage({ path: img, x: ix, y: 0, w: iw, h: H, sizing: { type: 'cover', w: iw, h: H } });
  else {
    // No picture: a hero panel carrying the motif and the kicker keeps the half page designed.
    slide.addImage({ data: ctx.bg('hero', t.hero.bg, t.glow), x: ix, y: 0, w: iw, h: H });
    rings(ctx, slide, ix + iw / 2, H / 2, t.hero.accent, 0.95);
    const label = asText(s.kicker || s.label);
    if (label) slide.addText(label, { x: ix + 0.6, y: H / 2 - 0.7, w: iw - 1.2, h: 1.4, fontFace: ctx.font.head, fontSize: fitSize(ctx, label, iw - 1.2, 1.4, 40, 20, { bold: true }), bold: true, color: t.hero.text, align: 'center', valign: 'middle', margin: 0 });
  }
  const tx = left ? iw + 0.7 : M;
  const tw = W - iw - 0.7 - M;
  const title = s.title || '';
  const tpt = fitSize(ctx, title, tw, 1.4, 32, 22, { bold: true });
  const th = Math.min(1.45, textHeight(title, tw, tpt, true) + 0.1);
  const kh = s.kicker ? 0.45 : 0;
  const room = 6.8 - 0.9 - kh - th - 0.3;
  const textH = s.text ? (s.bullets ? 1.4 : room) : 0;
  const textPt = s.text ? fitSize(ctx, s.text, tw, textH, 20, 11, { spacing: 1.35 }) : 0;
  const usedText = s.text ? Math.min(textH, textHeight(s.text, tw, textPt, false, 1.35) + 0.1) : 0;
  const list = s.bullets ? measureBullets(ctx, s.bullets, tw, room - usedText - 0.15, 20, 11) : null;
  const block = kh + th + 0.3 + usedText + (list ? list.h + 0.15 : 0);
  // Centre the text column vertically against the full-height picture.
  let y = Math.max(0.9, (H - block) * 0.45);
  if (s.kicker) {
    slide.addText(s.kicker, { x: tx, y, w: tw, h: 0.35, fontFace: ctx.font.body, fontSize: 12, bold: true, color: t.accent, charSpacing: 2, margin: 0 });
    y += kh;
  }
  slide.addText(title, { x: tx, y, w: tw, h: th, fontFace: ctx.font.head, fontSize: tpt, bold: true, color: t.text, margin: 0, valign: 'top' });
  y += th + 0.3;
  if (s.text) {
    slide.addText(s.text, { x: tx, y, w: tw, h: usedText, fontFace: ctx.font.body, fontSize: textPt, color: t.muted, margin: 0, valign: 'top', lineSpacingMultiple: 1.2 });
    y += usedText + 0.15;
  }
  if (list) bulletList(ctx, slide, s.bullets, tx, y, tw, list.h + 0.05, { pt: list.pt });
};

LAYOUTS.image = (ctx, slide, s) => {
  const t = ctx.t;
  const img = resolveImage(ctx, s.image);
  slide.background = { color: t.hero.bg };
  if (img) slide.addImage({ path: img, x: 0, y: 0, w: W, h: H, sizing: { type: 'cover', w: W, h: H } });
  if (s.title || s.caption) {
    slide.addShape(ctx.pptx.ShapeType.rect, { x: 0, y: H - 2.0, w: W, h: 2.0, fill: { color: '000000', transparency: 30 }, line: { type: 'none' } });
    if (s.title) slide.addText(s.title, { x: M, y: H - 1.8, w: W - 2 * M, h: 0.9, fontFace: ctx.font.head, fontSize: 30, bold: true, color: 'FFFFFF', margin: 0, valign: 'bottom', fit: 'shrink' });
    if (s.caption) slide.addText(s.caption, { x: M, y: H - 0.85, w: W - 2 * M, h: 0.5, fontFace: ctx.font.body, fontSize: 15, color: 'E5E7EB', margin: 0, valign: 'top', fit: 'shrink' });
  }
};

LAYOUTS.closing = (ctx, slide, s) => {
  const t = ctx.t.hero;
  heroBackground(ctx, slide);
  const title = s.title || (ctx.cjk ? '谢谢' : 'Thank you');
  rings(ctx, slide, W / 2, 3.15, t.accent, 1.15);
  slide.addText(title, { x: M, y: 2.3, w: W - 2 * M, h: 1.5, fontFace: ctx.font.head, fontSize: fitSize(ctx, title, W - 2 * M, 1.5, 60, 32, { bold: true }), bold: true, color: t.text, align: 'center', valign: 'middle', margin: 0 });
  slide.addShape(ctx.pptx.ShapeType.rect, { x: W / 2 - 0.55, y: 3.95, w: 1.1, h: 0.06, fill: { color: t.accent }, line: { type: 'none' } });
  if (s.subtitle) slide.addText(s.subtitle, { x: M, y: 4.25, w: W - 2 * M, h: 0.7, fontFace: ctx.font.body, fontSize: 20, color: t.muted, align: 'center', margin: 0, fit: 'shrink' });
  if (s.contact) slide.addText(s.contact, { x: M, y: 5.6, w: W - 2 * M, h: 0.5, fontFace: ctx.font.body, fontSize: 14, color: t.muted, align: 'center', margin: 0 });
};

const ALIASES = { 'two-column': 'compare', swot: 'matrix', kpi: 'stats', 'image-left': 'image-text', 'text-image': 'image-text', end: 'closing', thanks: 'closing', toc: 'agenda', list: 'bullets', features: 'cards', steps: 'process', roadmap: 'timeline' };
// Main content field(s) per layout; a slide with none of them renders empty, so say so.
const CONTENT_FIELDS = {
  agenda: ['items'], bullets: ['bullets', 'items', 'points'], cards: ['cards', 'items'], stats: ['stats', 'items'],
  chart: ['chart'], table: ['rows'], timeline: ['steps', 'items'], process: ['steps', 'items'], compare: ['left', 'right'],
  matrix: ['quadrants', 'items'], funnel: ['stages', 'items'], quote: ['quote', 'text', 'title'], image: ['image'],
};
const NO_FOOTER = new Set(['cover', 'section', 'closing', 'quote', 'image', 'agenda', 'image-text']);

// ---------------------------------------------------------------- build

async function build(specPath, outPath) {
  const deck = JSON.parse(fs.readFileSync(specPath, 'utf8').replace(/^﻿/, ''));
  if (!Array.isArray(deck.slides) || !deck.slides.length) throw new Error('deck.slides 必须是非空数组');
  const themeName = deck.theme || 'business-blue';
  const t = THEMES[themeName];
  if (!t) throw new Error(`未知主题 "${themeName}"，可用：${Object.keys(THEMES).join(', ')}`);
  const cjk = CJK_RE.test(JSON.stringify(deck.slides).slice(0, 4000));
  const pptx = new PptxGenJS();
  pptx.layout = 'LAYOUT_WIDE';
  pptx.title = deck.title || '';
  pptx.author = deck.author || 'One Work';
  pptx.company = deck.company || '';
  const warnings = [];
  const bgCache = new Map();
  const ctx = {
    deck, t, pptx, cjk,
    font: cjk ? CJK_FONT : t.font,
    baseDir: path.dirname(path.resolve(specPath)),
    page: 0,
    sectionNo: 0,
    warn: (m) => warnings.push(`第 ${ctx.page} 页：${m}`),
    bg(key, base, glow) {
      if (!bgCache.has(key)) bgCache.set(key, gradientPng(base, glow || []));
      return bgCache.get(key);
    },
  };
  for (const raw of deck.slides) {
    ctx.page += 1;
    const type = ALIASES[raw.type] || raw.type || 'bullets';
    const fn = LAYOUTS[type];
    if (!fn) throw new Error(`第 ${ctx.page} 页：未知版式 "${raw.type}"，可用：${Object.keys(LAYOUTS).join(', ')}`);
    if (type === 'section') ctx.sectionNo += 1;
    const need = CONTENT_FIELDS[type];
    if (need && !need.some((k) => raw[k] != null && !(Array.isArray(raw[k]) && !raw[k].length))) {
      ctx.warn(`版式 ${type} 缺少内容字段 ${need.map((k) => `"${k}"`).join(' 或 ')}，这一页会是空的`);
    }
    const slide = pptx.addSlide();
    if (cjk) {
      // Tag text as zh-CN so PowerPoint applies CJK line-breaking rules (no '，' at line start).
      const addText = slide.addText.bind(slide);
      slide.addText = (text, opts) => addText(
        Array.isArray(text) ? text.map((r) => ({ ...r, options: { lang: 'zh-CN', ...(r.options || {}) } })) : text,
        { lang: 'zh-CN', ...opts },
      );
    }
    fn(ctx, slide, raw);
    if (!NO_FOOTER.has(type)) footer(ctx, slide);
    if (raw.notes) slide.addNotes(asText(raw.notes));
  }
  const out = outPath || specPath.replace(/\.json$/i, '') + '.pptx';
  await pptx.writeFile({ fileName: out });
  return { out: path.resolve(out), pages: deck.slides.length, theme: themeName, warnings };
}

async function main() {
  const args = process.argv.slice(2);
  if (args.includes('--themes')) {
    for (const [k, v] of Object.entries(THEMES)) console.log(`${k.padEnd(16)} ${v.label}`);
    return;
  }
  if (args.includes('--layouts')) {
    console.log(Object.keys(LAYOUTS).join(', '));
    return;
  }
  const spec = args.find((a) => !a.startsWith('-'));
  const oi = args.indexOf('-o');
  const out = oi >= 0 ? args[oi + 1] : undefined;
  if (!spec) {
    console.error('用法: node build-deck.cjs <deck.json> [-o out.pptx] | --themes | --layouts');
    process.exit(2);
  }
  const started = Date.now();
  const r = await build(spec, out);
  console.log(`OK ${r.pages} 页 · 主题 ${r.theme} · ${Date.now() - started}ms`);
  console.log(`输出: ${r.out}`);
  if (r.warnings.length) {
    console.log(`警告 ${r.warnings.length} 条（修改 deck.json 后重跑）：`);
    for (const w of r.warnings) console.log(`  - ${w}`);
  }
}

main().catch((e) => {
  console.error(`错误: ${e.message}`);
  process.exit(1);
});
