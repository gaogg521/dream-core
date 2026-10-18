#!/usr/bin/env node
/**
 * Verified calendar data for one year: every day's weekday, lunar month/day,
 * the 24 solar terms and the traditional festivals.
 *
 * Source of truth is the Hong Kong Observatory's official Gregorian–lunar
 * conversion table (same UTC+8 as Beijing). The JavaScript/ICU Chinese
 * calendar is NOT used as truth: it puts 2027's 正月初一 on Feb 7, while the
 * correct date is Feb 6. It is only a fallback when the table cannot be
 * fetched, and the output then says so (`verified: false`).
 *
 * Usage: node calendar-data.cjs <year> [-o out.json]
 */
'use strict';

const fs = require('fs');

const year = Number(process.argv[2]);
if (!Number.isInteger(year) || year < 1901 || year > 2100) {
  console.error('用法: node calendar-data.cjs <年份 1901-2100> [-o 输出.json]');
  process.exit(2);
}
const outIdx = process.argv.indexOf('-o');
const outPath = outIdx > 0 ? process.argv[outIdx + 1] : null;

const WEEK = ['日', '一', '二', '三', '四', '五', '六'];
const T2S = {
  驚蟄: '惊蛰', 穀雨: '谷雨', 小滿: '小满', 芒種: '芒种', 處暑: '处暑',
  閏: '闰', 臘: '腊',
};
const toSimplified = (s) => s.replace(/驚蟄|穀雨|小滿|芒種|處暑|閏|臘/g, (m) => T2S[m]);
const BRANCH_ANIMAL = { 子: '鼠', 丑: '牛', 寅: '虎', 卯: '兔', 辰: '龙', 巳: '蛇', 午: '马', 未: '羊', 申: '猴', 酉: '鸡', 戌: '狗', 亥: '猪' };
const STEMS = '甲乙丙丁戊己庚辛壬癸';
const BRANCHES = '子丑寅卯辰巳午未申酉戌亥';
const ganzhiOf = (y) => STEMS[(y - 4) % 10] + BRANCHES[(y - 4) % 12];
const DAY_NAMES = ['初一', '初二', '初三', '初四', '初五', '初六', '初七', '初八', '初九', '初十',
  '十一', '十二', '十三', '十四', '十五', '十六', '十七', '十八', '十九', '二十',
  '廿一', '廿二', '廿三', '廿四', '廿五', '廿六', '廿七', '廿八', '廿九', '三十'];

/** Fetch and parse one HKO year table → [{ date, lunarLabel, term }]. */
async function fetchHko(y) {
  const url = `https://www.hko.gov.hk/tc/gts/time/calendar/text/files/T${y}c.txt`;
  const res = await fetch(url, { signal: AbortSignal.timeout(15000) });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const text = await res.text();
  const rows = [];
  const seen = new Set();
  for (const line of text.split(/\r?\n/)) {
    const m = line.match(/^(\d{4})年(\d{1,2})月(\d{1,2})日\s+(\S+)\s+星期\S\s*(\S*)/);
    if (!m) continue;
    const date = `${m[1]}-${m[2].padStart(2, '0')}-${m[3].padStart(2, '0')}`;
    if (seen.has(date)) continue; // the table repeats term rows in a summary section
    seen.add(date);
    rows.push({ date, lunarLabel: toSimplified(m[4]), term: m[5] ? toSimplified(m[5]) : null });
  }
  if (rows.length < 365) throw new Error(`table for ${y} has only ${rows.length} days`);
  return rows;
}

/** Turn the label column (a month name on 初一, a day name otherwise) into month + day. */
function resolveLunar(rows, startMonth) {
  let month = startMonth;
  let day = 0;
  return rows.map((r) => {
    if (/月$/.test(r.lunarLabel)) {
      month = r.lunarLabel;
      day = 1;
    } else {
      const idx = DAY_NAMES.indexOf(r.lunarLabel);
      day = idx >= 0 ? idx + 1 : day + 1;
    }
    return { ...r, lunarMonth: month, lunarDay: day };
  });
}

function lastMonthName(rows) {
  for (let i = rows.length - 1; i >= 0; i--) if (/月$/.test(rows[i].lunarLabel)) return rows[i].lunarLabel;
  return null;
}

async function fromHko() {
  const [prev, cur] = await Promise.all([fetchHko(year - 1), fetchHko(year)]);
  const startMonth = lastMonthName(prev);
  const days = resolveLunar(cur, startMonth).filter((d) => d.date.startsWith(`${year}-`));
  return { source: 'Hong Kong Observatory 公历与农历日期对照表', verified: true, days };
}

/** ICU fallback — known to be off by a day in some months (2027 正月). */
function fromIcu() {
  const fmt = new Intl.DateTimeFormat('zh-CN-u-ca-chinese', { timeZone: 'Asia/Shanghai', month: 'long', day: 'numeric' });
  const days = [];
  for (let d = new Date(Date.UTC(year, 0, 1)); d.getUTCFullYear() === year; d.setUTCDate(d.getUTCDate() + 1)) {
    const parts = fmt.formatToParts(new Date(d.getTime() + 4 * 3600e3));
    const month = parts.find((p) => p.type === 'month').value;
    const day = Number(parts.find((p) => p.type === 'day').value);
    days.push({ date: d.toISOString().slice(0, 10), lunarMonth: month, lunarDay: day, term: null });
  }
  return { source: 'ICU（未核验，节气缺失）', verified: false, days };
}

const FESTIVALS = [
  ['正月', 1, '春节'], ['正月', 15, '元宵节'], ['二月', 2, '龙抬头'], ['五月', 5, '端午节'],
  ['七月', 7, '七夕'], ['七月', 15, '中元节'], ['八月', 15, '中秋节'], ['九月', 9, '重阳节'], ['腊月', 8, '腊八节'],
];
const SOLAR_FESTIVALS = { '01-01': '元旦', '02-14': '情人节', '03-08': '妇女节', '03-12': '植树节', '04-01': '愚人节',
  '05-01': '劳动节', '05-04': '青年节', '06-01': '儿童节', '07-01': '建党节', '08-01': '建军节', '09-10': '教师节',
  '10-01': '国庆节', '12-25': '圣诞节' };

async function main() {
  let data;
  try {
    data = await fromHko();
  } catch (e) {
    console.error(`警告: 无法获取香港天文台对照表 (${e.message})，改用 ICU 推算——农历日期可能差一天，必须向用户注明未核验。`);
    data = fromIcu();
  }
  const isMonth = (m, name) => m.replace('十二月', '腊月') === name;
  const days = data.days.map((d, i) => {
    const date = new Date(`${d.date}T00:00:00Z`);
    const festivals = [];
    for (const [m, day, name] of FESTIVALS) if (isMonth(d.lunarMonth, m) && d.lunarDay === day && !d.lunarMonth.startsWith('闰')) festivals.push(name);
    const next = data.days[i + 1];
    if (next && isMonth(next.lunarMonth, '正月') && next.lunarDay === 1) festivals.push('除夕');
    if (d.term === '清明') festivals.push('清明节');
    const solar = SOLAR_FESTIVALS[d.date.slice(5)];
    if (solar) festivals.push(solar);
    return {
      date: d.date,
      weekday: WEEK[date.getUTCDay()],
      lunar: `${d.lunarMonth.replace('十二月', '腊月')}${DAY_NAMES[d.lunarDay - 1] ?? d.lunarDay}`,
      lunarMonth: d.lunarMonth.replace('十二月', '腊月'),
      lunarDay: d.lunarDay,
      term: d.term,
      festivals,
    };
  });

  // The lunar year that begins inside this Gregorian year carries the zodiac.
  const gz = ganzhiOf(year);
  const leap = (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
  const months = Array.from({ length: 12 }, (_, m) => ({
    month: m + 1,
    days: new Date(Date.UTC(year, m + 1, 0)).getUTCDate(),
    firstWeekday: WEEK[new Date(Date.UTC(year, m, 1)).getUTCDay()],
  }));
  const result = {
    year,
    source: data.source,
    verified: data.verified,
    leapYear: leap,
    totalDays: leap ? 366 : 365,
    ganzhi: gz,
    zodiac: BRANCH_ANIMAL[gz[1]],
    months,
    solarTerms: days.filter((d) => d.term).map((d) => ({ date: d.date, name: d.term })),
    festivals: days.filter((d) => d.festivals.length).map((d) => ({ date: d.date, weekday: d.weekday, names: d.festivals })),
    days,
  };

  const json = JSON.stringify(result, null, 1);
  if (outPath) fs.writeFileSync(outPath, json, 'utf8');
  // Short summary for the agent to read and to quote to the user.
  const cny = result.festivals.find((f) => f.names.includes('春节'));
  console.log(`${year} ${gz}${result.zodiac}年 · ${leap ? '闰年' : '平年'} ${result.totalDays} 天 · 2 月 ${months[1].days} 天 · 数据源: ${result.source}${result.verified ? '' : ' ⚠未核验'}`);
  if (cny) console.log(`春节 ${cny.date}（周${cny.weekday}）`);
  console.log(`节日: ${result.festivals.map((f) => `${f.date.slice(5)} ${f.names.join('/')}`).join('，')}`);
  if (result.solarTerms.length) console.log(`节气: ${result.solarTerms.map((t) => `${t.date.slice(5)} ${t.name}`).join('，')}`);
  if (outPath) console.log(`完整数据已写入 ${outPath}`);
}

main().catch((e) => {
  console.error(e.stack || String(e));
  process.exit(1);
});
