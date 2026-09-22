# -*- coding: utf-8 -*-
"""
公众号 HTML 版 · 本号唯一真相源（克隆 qclaw_clone build_html_7yue.py + header_card/footer_card 而来）

原则（见 AGENTS.md）：克隆 canonical、不重写散文。后续 DAG 只填内容块 + 改配色参数，
禁止每次从散文重新推导尺寸与结构。

两种用法：
  1) 生成（未来 DAG）： build_html.py gen <blocks.json> <out.html> --deep #XXX --accent #XXX --light #XXX --line #XXX --ink #XXX --series "..." --title "..."
  2) 回炉（把已漂的旧 HTML 重排成 canonical 卡片式）： build_html.py refit <old.html> <out.html> [--deep #XXX --accent #XXX ...]
     refit 会自动侦测该篇自身配色（保留每篇独特色调），只重排结构。

卡片式强制结构（对齐 qclaw_clone 带头尾引导.html + header_card.html + footer_card.html）：
  - 外容器 line-height:1.85; font-size:15px; 暖墨色
  - 整块深底栏目头条（深主色底 + 左栏目名 + 右浅系列标签，flex space-between）
  - 大标题 27px 深主色 letter-spacing:1px
  - 小节标题 21px 深主色 + 2px 下边框（非左侧红竖条）
  - 卡片块 浅底 + 1px 细描边 + 左侧 4px 强调色条（克制，非全金边框）
  - 表格 表头沉主色底白字，隔行浅底逐 td 内联
  - 尾四要素 footer_card（深主色底 + 左"觉得有用就动动手" + 右四图标白字）
  - 硬约束：无 <div>/<h1-h3>/<style>/class/max-width/head/body/script
"""
import sys, re, json

FONT_STACK = "'LXGW WenKai','LXGWWenKai','Microsoft YaHei',sans-serif"

BLACK = (43, 43, 43)
WHITE = (255, 255, 255)

def hex2rgb(h):
    h = h.lstrip('#')
    return tuple(int(h[i:i+2], 16) for i in (0, 2, 4))

def _lum(rgb):
    def f(c):
        c = c/255.0
        return c/12.92 if c <= 0.03928 else ((c+0.055)/1.055)**2.4
    r, g, b = [f(x) for x in rgb]
    return 0.2126*r + 0.7152*g + 0.0722*b

def _contrast(a, b):
    la, lb = _lum(a), _lum(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)

def _is_near(c, ref, tol=18):
    return all(abs(c[i]-ref[i]) <= tol for i in range(3))


def render_header_card(deep, accent, series, column):
    right = '#F3E6D2' if _contrast(hex2rgb(deep), WHITE) >= 4.5 else '#F3D9C2'
    return (f'<section style="background:{deep};border-radius:12px;padding:12px 16px;'
            f'margin:10px 0;display:flex;align-items:center;justify-content:space-between;">\n'
            f'<span style="color:{accent};font-size:14px;white-space:nowrap;flex-shrink:0;">'
            f'{column}</span>\n'
            f'<span style="color:{right};font-size:13px;white-space:nowrap;">{series}</span>\n'
            f'</section>')

def render_headline(title, deep):
    return (f'<p style="font-size:27px;color:{deep};letter-spacing:1px;margin:0 0 6px;">'
            f'{title}</p>')

def render_h2(text, deep, line):
    return (f'<p style="font-size:21px;color:{deep};margin:34px 0 14px;'
            f'padding-bottom:6px;border-bottom:2px solid {line};">{text}</p>')

def render_p(text, ink, deep, accent):
    return (f'<p style="font-size:15px;color:{ink};line-height:1.85;margin:12px 0;">'
            f'{_inline(text, deep, accent)}</p>')

def render_card(text, light, line, accent, ink, deep):
    return (f'<section style="background:{light};border:1px solid {line};'
            f'border-left:4px solid {accent};color:{ink};font-size:15px;'
            f'border-radius:8px;margin:16px 0;padding:12px 16px;">\n'
            f'<p style="margin:0;line-height:1.85;">{_inline(text, deep, accent)}</p>\n'
            f'</section>')

def render_table(headers, rows, deep, line, light, ink, accent):
    th = ''.join(
        f'<th style="border:1px solid {line};padding:8px 10px;text-align:left;'
        f'vertical-align:top;background:{deep};color:#fff;font-weight:700;">{h}</th>'
        for h in headers)
    body_rows = []
    for i, row in enumerate(rows):
        bg = light if i % 2 == 1 else '#FFFFFF'
        tds = ''.join(
            f'<td style="border:1px solid {line};padding:8px 10px;text-align:left;'
            f'vertical-align:top;background:{bg};">{_inline(c, deep, accent)}</td>'
            for c in row)
        body_rows.append(f'<tr>{tds}</tr>')
    return (f'<table style="width:100%;border-collapse:collapse;margin:16px 0;font-size:14px;">\n'
            f'<thead><tr>{th}</tr></thead>\n<tbody>{"".join(body_rows)}</tbody>\n</table>')

def render_footer(deep, accent):
    return (f'<section style="background:{deep};border-radius:12px;padding:14px 16px;'
            f'margin:10px 0;display:flex;align-items:center;justify-content:space-between;">\n'
            f'<span style="color:{accent};font-size:14px;white-space:nowrap;flex-shrink:0;">'
            f'觉得有用就动动手</span>\n'
            f'<span style="display:flex;align-items:center;">\n'
            f'<span style="text-align:center;margin-left:14px;"><span style="font-size:20px;display:block;">👍</span>'
            f'<span style="font-size:11px;color:#FFFFFF;display:block;">赞</span></span>\n'
            f'<span style="text-align:center;margin-left:14px;"><span style="font-size:20px;display:block;">🔄</span>'
            f'<span style="font-size:11px;color:#FFFFFF;display:block;">分享</span></span>\n'
            f'<span style="text-align:center;margin-left:14px;"><span style="font-size:20px;display:block;">❤️</span>'
            f'<span style="font-size:11px;color:#FFFFFF;display:block;">推荐</span></span>\n'
            f'<span style="text-align:center;margin-left:14px;"><span style="font-size:20px;display:block;">✏️</span>'
            f'<span style="font-size:11px;color:#FFFFFF;display:block;">写留言</span></span>\n'
            f'</span>\n</section>')

def render_ai_note(ink):
    # 对齐《推荐运营规范》7.4：AIGC 辅助创作须声明；仅写通用已核实表述，不暴露不确定性
    return (f'<p style="font-size:12px;color:{ink};text-align:center;margin:14px 0 4px;'
            f'opacity:0.7;line-height:1.6;">'
            f'本文由 AI 辅助创作，政策要点以各地官方公告原文为准。'
            f'</p>')

def _inline(text, deep, accent):
    text = text.replace('&', '&amp;').replace('<', '&lt;').replace('>', '&gt;')
    text = re.sub(r'\*\*(.+?)\*\*', lambda m: f'<strong style="color:{accent};font-weight:700;">{m.group(1)}</strong>', text)
    text = re.sub(r'(?<!\*)\*(?!\*)(.+?)\*(?!\*)', lambda m: f'<strong style="color:{deep};font-weight:700;">{m.group(1)}</strong>', text)
    return text

def render_outro(text, light, line, accent, ink, deep):
    return render_card(text, light, line, accent, ink, deep)


def assemble(blocks, palette, meta):
    deep, accent, light, line, ink = (palette['deep'], palette['accent'],
                                       palette['light'], palette['line'], palette['ink'])
    series, column, title = meta['series'], meta['column'], meta['title']
    out = []
    out.append(f'<section style="padding:4px 0;font-family:{FONT_STACK};color:{ink};line-height:1.85;font-size:15px;">')
    out.append(render_header_card(deep, accent, series, column))
    out.append(render_headline(title, deep))
    for b in blocks:
        kind = b[0]
        if kind == 'lead':
            out.append(render_p(b[1], ink, deep, accent))
        elif kind == 'h2':
            out.append(render_h2(b[1], deep, line))
        elif kind == 'p':
            out.append(render_p(b[1], ink, deep, accent))
        elif kind == 'card':
            out.append(render_card(b[1], light, line, accent, ink, deep))
        elif kind == 'outro':
            out.append(render_outro(b[1], light, line, accent, ink, deep))
        elif kind == 'table':
            out.append(render_table(b[1], b[2], deep, line, light, ink, accent))
        elif kind == 'tailnote':
            out.append(render_tailnote(b[1], line, ink))
    out.append(render_footer(deep, accent))
    out.append(render_ai_note(ink))
    out.append('</section>')
    return '\n'.join(out)

def render_tailnote(text, line, ink):
    return (f'<p style="font-size:13px;color:{ink};margin:18px 0 8px;'
            f'border-top:1px solid {line};padding-top:10px;line-height:1.7;">{text}</p>')


def detect_palette(html):
    th_bgs = re.findall(r'<th[^>]*background:(#[0-9A-Fa-f]{6})', html)
    deep = max(set(th_bgs), key=th_bgs.count) if th_bgs else '#9E2B25'
    colors = re.findall(r'color:(#[0-9A-Fa-f]{6})', html)
    cand = {}
    for c in colors:
        rgb = hex2rgb(c)
        if c.lower() in ('#2b2b2b', '#ffffff', '#000000'):
            continue
        if _is_near(rgb, hex2rgb(deep)):
            continue
        cand[c] = cand.get(c, 0) + 1
    accent = max(cand, key=cand.get) if cand else '#E8A33D'
    card_bgs = re.findall(r'background:(#[0-9A-Fa-f]{6})', html)
    lights = [c for c in set(card_bgs)
              if not _is_near(hex2rgb(c), hex2rgb(deep))
              and _lum(hex2rgb(c)) > 0.82 and c.lower() != '#ffffff']
    light = lights[0] if lights else '#FBF4EC'
    line = '#E5D5C0'
    ink = '#3A2A24' if _is_near(hex2rgb(deep), (158, 43, 37)) else '#2B2B2B'
    return {'deep': deep, 'accent': accent, 'light': light, 'line': line, 'ink': ink}


def _clean_text(s):
    return re.sub(r'\s+', ' ', re.sub(r'<[^>]+>', ' ', s)).strip()


def refit(html, out_path, palette_override=None):
    m = re.search(r'<p style="font-size:(?:22px|27px)[^>]*>(.*?)</p>', html, re.S)
    title = _clean_text(m.group(1)) if m else '标题'
    series = '招投标专家日报'
    column = '📜 政策法规解读'
    mb = re.search(r'background:#[0-9A-Fa-f]{6};border-radius:12px[^>]*>.*?<span[^>]*>(.*?)</span>.*?<span[^>]*>(.*?)</span>', html, re.S)
    if mb:
        column = _clean_text(mb.group(1))
        series = _clean_text(mb.group(2))

    palette = detect_palette(html)
    if palette_override:
        palette.update(palette_override)

    body = html
    # 头栏目块：深底 + 圆角12 + 内边距（容忍样式字段顺序/空格）
    body = re.sub(r'<section style="[^"]*border-radius:\s*12px[^"]*display:\s*flex;\s*align-items:\s*center;\s*justify-content:\s*space-between[^"]*>.*?</section>', '', body, flags=re.S)
    # 尾部互动块（flex 横排四图标）
    body = re.sub(r'<section style="[^"]*display:\s*flex[^"]*justify-content:\s*space-around[^"]*>.*?</section>', '', body, flags=re.S)
    body = re.sub(r'<section style="[^"]*display:\s*flex;\s*margin-top:24px;\s*background:[^"]*border-radius[^"]*>.*?</section>', '', body, flags=re.S)
    body = re.sub(r'<p style="[^"]*觉得有用.*?</p>', '', body, flags=re.S)
    # 顶部日期小行（如 <p style="font-size:13px;color:#17BEBB...>招投标专家日更</p>）并入正文即可，不影响结构

    blocks = []
    for seg in re.split(r'(<table[\s\S]*?</table>|<section[\s\S]*?</section>)', body):
        seg = seg.strip()
        if not seg:
            continue
        if seg.startswith('<table'):
            ths = re.findall(r'<th[^>]*>(.*?)</th>', seg, re.S)
            headers = [_clean_text(h) for h in ths]
            rows = []
            for tr in re.findall(r'<tr>(.*?)</tr>', seg, re.S)[1:]:
                cells = re.findall(r'<td[^>]*>(.*?)</td>', tr, re.S)
                rows.append([_clean_text(c) for c in cells])
            if headers and rows:
                blocks.append(('table', headers, rows))
            continue
        if seg.startswith('<section'):
            opentag = seg[:seg.index('>')+1] if '>' in seg else seg
            own_border_left = 'border-left' in opentag
            own_radius = 'border-radius' in opentag
            own_fw = 'font-weight:700' in opentag or 'font-weight:bold' in opentag
            # 卡片块（自身含 border-left 强调条 或 圆角浅底）
            if own_border_left or own_radius:
                txt = _clean_text(seg)
                if txt and '觉得有用' not in txt:
                    blocks.append(('card', txt))
                continue
            # 小节标题（自身 font-weight:700 的 section）
            if own_fw:
                txt = _clean_text(seg)
                if txt and '觉得有用' not in txt:
                    blocks.append(('h2', txt))
                continue
            # 纯包裹层 section（无强调条/非标题）-> 取其内部内容递归切分，避免吞掉内部 h2/p
            inner = re.sub(r'^<section[^>]*>', '', seg, count=1)
            inner = re.sub(r'</section>\s*$', '', inner)
            for sub in re.split(r'(<table[\s\S]*?</table>|<section[\s\S]*?</section>|<p\s[^>]*>.*?</p>)', inner):
                sub = sub.strip()
                if not sub:
                    continue
                if sub.startswith('<table'):
                    ths = re.findall(r'<th[^>]*>(.*?)</th>', sub, re.S)
                    headers = [_clean_text(h) for h in ths]
                    rows = []
                    for tr in re.findall(r'<tr>(.*?)</tr>', sub, re.S)[1:]:
                        cells = re.findall(r'<td[^>]*>(.*?)</td>', tr, re.S)
                        rows.append([_clean_text(c) for c in cells])
                    if headers and rows:
                        blocks.append(('table', headers, rows))
                    continue
                if sub.startswith('<section'):
                    sopentag = sub[:sub.index('>')+1] if '>' in sub else sub
                    sub_bl = 'border-left' in sopentag
                    sub_rad = 'border-radius' in sopentag
                    sub_fw = 'font-weight:700' in sopentag or 'font-weight:bold' in sopentag
                    if sub_bl or sub_rad or sub_fw:
                        t2 = _clean_text(sub)
                        if t2 and '觉得有用' not in t2:
                            blocks.append(('card', t2) if (sub_bl or sub_rad) else ('h2', t2))
                    continue
                if sub.startswith('<p'):
                    if re.search(r'font-weight:700', sub) and re.search(r'font-size:(?:17px|18px|19px|21px|22px)', sub):
                        t2 = _clean_text(sub)
                        if t2:
                            blocks.append(('h2', t2))
                        continue
                    t2 = _clean_text(sub)
                    if t2:
                        blocks.append(('p', t2))
            continue
        for chunk in re.split(r'(?=<p\s)', seg):
            chunk = chunk.strip()
            if not chunk.startswith('<p'):
                continue
            if re.search(r'font-weight:700', chunk) and re.search(r'font-size:(?:17px|18px|19px|21px|22px)', chunk):
                txt = _clean_text(chunk)
                if txt:
                    blocks.append(('h2', txt))
                continue
            txt = _clean_text(chunk)
            if txt:
                blocks.append(('p', txt))

    blocks = [b for b in blocks if '觉得有用' not in (b[1] if len(b) > 1 and isinstance(b[1], str) else '')]
    meta = {'series': series, 'column': column, 'title': title}
    out = assemble(blocks, palette, meta)
    with open(out_path, 'w', encoding='utf-8') as f:
        f.write(out)
    return out, palette


def gen(blocks_json, out_path, palette, meta):
    blocks = json.load(open(blocks_json, encoding='utf-8'))
    out = assemble(blocks, palette, meta)
    with open(out_path, 'w', encoding='utf-8') as f:
        f.write(out)
    return out


def selfcheck(html):
    bad = {}
    for tok in ['<div', '<h1', '<h2', '<h3', '<style', ' class=', 'max-width',
               '<head', '<body', '<script']:
        n = html.count(tok)
        if n:
            bad[tok] = n
    return bad


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else 'refit'
    if mode == 'refit':
        old = sys.argv[2]
        out = sys.argv[3] if len(sys.argv) > 3 else old
        html = open(old, encoding='utf-8').read()
        ov = {}
        for i, a in enumerate(sys.argv[4:]):
            if a == '--deep': ov['deep'] = sys.argv[4+i+1]
            if a == '--accent': ov['accent'] = sys.argv[4+i+1]
            if a == '--light': ov['light'] = sys.argv[4+i+1]
            if a == '--line': ov['line'] = sys.argv[4+i+1]
            if a == '--ink': ov['ink'] = sys.argv[4+i+1]
        out_html, pal = refit(html, out, ov or None)
        bad = selfcheck(out_html)
        print(f'refit -> {out}')
        print('palette:', pal)
        print('硬约束:', 'PASS' if not bad else f'FAIL {bad}')
    elif mode == 'gen':
        bj = sys.argv[2]; out = sys.argv[3]
        pal = {'deep': '#9E2B25', 'accent': '#E8A33D', 'light': '#FBF4EC',
               'line': '#E5D5C0', 'ink': '#3A2A24'}
        meta = {'series': '招投标专家日报', 'column': '📜 政策法规解读', 'title': '标题'}
        for i, a in enumerate(sys.argv[4:]):
            if a == '--deep': pal['deep'] = sys.argv[4+i+1]
            if a == '--accent': pal['accent'] = sys.argv[4+i+1]
            if a == '--light': pal['light'] = sys.argv[4+i+1]
            if a == '--line': pal['line'] = sys.argv[4+i+1]
            if a == '--ink': pal['ink'] = sys.argv[4+i+1]
            if a == '--series': meta['series'] = sys.argv[4+i+1]
            if a == '--column': meta['column'] = sys.argv[4+i+1]
            if a == '--title': meta['title'] = sys.argv[4+i+1]
        out_html = gen(bj, out, pal, meta)
        bad = selfcheck(out_html)
        print(f'gen -> {out} | 硬约束:', 'PASS' if not bad else f'FAIL {bad}')


if __name__ == '__main__':
    main()
