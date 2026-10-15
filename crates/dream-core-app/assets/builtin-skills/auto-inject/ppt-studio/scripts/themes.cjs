'use strict';
// Theme catalog. Every color is a 6-digit hex without '#'.
// `hero` styles cover / section / closing slides; on dark themes it matches the body.
// `glow` blobs paint the procedural gradient behind hero slides (x/y/r are fractions of the canvas).

const THEMES = {
  'business-blue': {
    label: '商务蓝 · 工作汇报/方案/通用',
    dark: false,
    bg: 'FFFFFF', surface: 'F2F5FA', surface2: 'E6ECF5', text: '18233A', muted: '5F6B80', line: 'DDE3EC',
    primary: '1D4ED8', accent: '2563EB', accent2: '0EA5A4', good: '16A34A', bad: 'DC2626',
    hero: { bg: '0B1F4A', text: 'FFFFFF', muted: 'B4C2DC', accent: '60A5FA' },
    glow: [{ c: '2563EB', x: 0.92, y: 0.05, r: 0.75, a: 0.75 }, { c: '0EA5A4', x: 0.05, y: 1.05, r: 0.7, a: 0.45 }],
    chart: ['1D4ED8', '60A5FA', '0EA5A4', 'F59E0B', '94A3B8', 'E11D48'],
    font: { head: 'Segoe UI Semibold', body: 'Segoe UI' },
  },
  'midnight-tech': {
    label: '深空科技 · 发布会/AI/技术分享',
    dark: true,
    bg: '0B1020', surface: '141B2E', surface2: '1C2540', text: 'F1F5FF', muted: '95A0BC', line: '263152',
    primary: '7C9CFF', accent: '22D3EE', accent2: 'A78BFA', good: '34D399', bad: 'F87171',
    hero: { bg: '070B17', text: 'FFFFFF', muted: 'A3AECB', accent: '22D3EE' },
    glow: [{ c: '6366F1', x: 0.85, y: 0.1, r: 0.7, a: 0.7 }, { c: '06B6D4', x: 0.1, y: 0.95, r: 0.65, a: 0.5 }, { c: 'A855F7', x: 0.55, y: 0.55, r: 0.45, a: 0.25 }],
    bodyGlow: [{ c: '4F46E5', x: 1.0, y: 0.0, r: 0.6, a: 0.28 }, { c: '06B6D4', x: 0.0, y: 1.0, r: 0.5, a: 0.16 }],
    chart: ['22D3EE', '7C9CFF', 'A78BFA', '34D399', 'FBBF24', 'F472B6'],
    font: { head: 'Segoe UI Semibold', body: 'Segoe UI' },
  },
  'aurora': {
    label: '极光渐变 · 产品/创意/品牌',
    dark: true,
    bg: '120A2A', surface: '1E1440', surface2: '2A1D55', text: 'FFFFFF', muted: 'B8AEDB', line: '3A2C6B',
    primary: 'C084FC', accent: 'F472B6', accent2: '38BDF8', good: '4ADE80', bad: 'FB7185',
    hero: { bg: '0E0724', text: 'FFFFFF', muted: 'C9C0EA', accent: 'F9A8D4' },
    glow: [{ c: 'EC4899', x: 0.95, y: 0.0, r: 0.75, a: 0.7 }, { c: '8B5CF6', x: 0.35, y: 0.25, r: 0.6, a: 0.45 }, { c: '0EA5E9', x: 0.05, y: 1.05, r: 0.7, a: 0.55 }],
    bodyGlow: [{ c: '8B5CF6', x: 1.0, y: 0.0, r: 0.65, a: 0.32 }, { c: 'EC4899', x: 0.0, y: 1.0, r: 0.5, a: 0.18 }],
    chart: ['F472B6', 'C084FC', '38BDF8', '4ADE80', 'FBBF24', 'FB923C'],
    font: { head: 'Segoe UI Semibold', body: 'Segoe UI' },
  },
  'minimal-mono': {
    label: '极简黑白 · 高级感/咨询/设计',
    dark: false,
    bg: 'FAFAF7', surface: 'F0EFEA', surface2: 'E5E3DC', text: '111111', muted: '6B6B6B', line: 'DCDAD3',
    primary: '111111', accent: 'E4372B', accent2: '6B6B6B', good: '2F7D4F', bad: 'E4372B',
    hero: { bg: '111111', text: 'FAFAF7', muted: 'A8A8A3', accent: 'FF5A4E' },
    glow: [{ c: '3A3A3A', x: 0.9, y: 0.1, r: 0.6, a: 0.6 }],
    chart: ['111111', 'E4372B', '8C8C8C', 'C9C6BC', '4A4A4A', 'F2A097'],
    font: { head: 'Georgia', body: 'Segoe UI' },
  },
  'warm-terracotta': {
    label: '暖陶土 · 消费/文旅/人文/教育',
    dark: false,
    bg: 'FBF6EE', surface: 'F3E9DA', surface2: 'EADBC4', text: '2B1D14', muted: '7A6555', line: 'E3D3BC',
    primary: 'B4532A', accent: 'D97742', accent2: '2F6B5A', good: '2F6B5A', bad: 'B4332A',
    hero: { bg: '3A2418', text: 'FBF6EE', muted: 'D9C4AE', accent: 'F2A36B' },
    glow: [{ c: 'D97742', x: 0.95, y: 0.05, r: 0.7, a: 0.55 }, { c: '2F6B5A', x: 0.0, y: 1.0, r: 0.6, a: 0.45 }],
    chart: ['B4532A', 'D97742', '2F6B5A', 'E2B04A', '8C7A6B', '6E8B74'],
    font: { head: 'Georgia', body: 'Segoe UI' },
  },
  'forest-green': {
    label: '森林绿 · ESG/金融/医疗/农业',
    dark: false,
    bg: 'FFFFFF', surface: 'EEF6F1', surface2: 'DDEEE4', text: '13261C', muted: '5B6F63', line: 'D5E5DB',
    primary: '0F6B45', accent: '16A34A', accent2: 'CA8A04', good: '16A34A', bad: 'DC2626',
    hero: { bg: '0A2E20', text: 'FFFFFF', muted: 'AFCBBB', accent: '4ADE80' },
    glow: [{ c: '16A34A', x: 0.95, y: 0.05, r: 0.7, a: 0.6 }, { c: 'CA8A04', x: 0.0, y: 1.05, r: 0.6, a: 0.35 }],
    chart: ['0F6B45', '4ADE80', 'CA8A04', '0EA5A4', '94A3B8', 'F97316'],
    font: { head: 'Segoe UI Semibold', body: 'Segoe UI' },
  },
  'china-red': {
    label: '中国红 · 政务/党建/年终总结/庆典',
    dark: false,
    bg: 'FFFFFF', surface: 'FBF1EF', surface2: 'F5E1DC', text: '2A1414', muted: '7A5B57', line: 'EFD9D4',
    primary: 'B91C1C', accent: 'C8102E', accent2: 'B8860B', good: '15803D', bad: 'B91C1C',
    hero: { bg: '8E0E12', text: 'FFFFFF', muted: 'F3C9C2', accent: 'F6C75A' },
    glow: [{ c: 'E11D48', x: 0.95, y: 0.0, r: 0.75, a: 0.55 }, { c: 'B8860B', x: 0.0, y: 1.05, r: 0.65, a: 0.45 }],
    chart: ['B91C1C', 'E8A33D', '7F1D1D', 'F87171', 'A8A29E', 'B8860B'],
    font: { head: 'Segoe UI Semibold', body: 'Segoe UI' },
  },
  'ocean-teal': {
    label: '海洋青 · 医疗/教育/互联网/清新',
    dark: false,
    bg: 'FFFFFF', surface: 'EEF8F8', surface2: 'DAF0F0', text: '0F2A2E', muted: '58717A', line: 'D3E8E9',
    primary: '0E7490', accent: '0891B2', accent2: 'F97316', good: '059669', bad: 'E11D48',
    hero: { bg: '083344', text: 'FFFFFF', muted: 'A5CAD3', accent: '67E8F9' },
    glow: [{ c: '06B6D4', x: 0.95, y: 0.05, r: 0.75, a: 0.6 }, { c: 'F97316', x: 0.0, y: 1.05, r: 0.55, a: 0.3 }],
    chart: ['0E7490', '22D3EE', 'F97316', '059669', '94A3B8', '8B5CF6'],
    font: { head: 'Segoe UI Semibold', body: 'Segoe UI' },
  },
};

// CJK decks use one family that carries both scripts, so mixed lines don't jump fonts.
const CJK_FONT = { head: 'Microsoft YaHei', body: 'Microsoft YaHei' };

module.exports = { THEMES, CJK_FONT };
