#!/usr/bin/env python3
"""Render a `tmux capture-pane -e -p` dump as a fixed-grid HTML page.

Every cell sits at its own column, so glyphs the monospace font lacks (braille
chart dots fall back to a proportional font) cannot push the rest of the row
out of alignment. Prints the page size as `WIDTH,HEIGHT` for the screenshot.

usage: ansi_to_html.py CAPTURE.ansi OUT.html FONT_DIR
"""

import html
import re
import sys
import unicodedata

FONT_SIZE = 14
CELL_WIDTH = FONT_SIZE * 1233 / 2048  # DejaVu Sans Mono advance width
LINE_HEIGHT = 17
PADDING = 16
BACKGROUND = (25, 25, 25)  # btop theme main_bg
FOREGROUND = (204, 204, 204)
BASIC = [
    (0, 0, 0), (205, 49, 49), (13, 188, 121), (229, 229, 16),
    (36, 114, 200), (188, 63, 188), (17, 168, 205), (229, 229, 229),
    (102, 102, 102), (241, 76, 76), (35, 209, 139), (245, 245, 67),
    (59, 142, 234), (214, 112, 214), (41, 184, 219), (255, 255, 255),
]
SGR = re.compile(r"\x1b\[([0-9;:]*)m")


def xterm256(n):
    if n < 16:
        return BASIC[n]
    if n < 232:
        n -= 16
        steps = [0, 95, 135, 175, 215, 255]
        return (steps[n // 36], steps[n // 6 % 6], steps[n % 6])
    level = 8 + (n - 232) * 10
    return (level, level, level)


def apply_sgr(state, params):
    codes = [int(p) if p else 0 for p in re.split(r"[;:]", params)] or [0]
    i = 0
    while i < len(codes):
        code = codes[i]
        if code == 0:
            state.clear()
        elif code in (1, 2, 3, 4, 7):
            state[code] = True
        elif code == 22:
            state.pop(1, None)
            state.pop(2, None)
        elif code in (23, 24, 27):
            state.pop(code - 20, None)
        elif code in (38, 48) and i + 1 < len(codes):
            key = "fg" if code == 38 else "bg"
            if codes[i + 1] == 2 and i + 4 < len(codes):
                state[key] = tuple(codes[i + 2 : i + 5])
                i += 4
            elif codes[i + 1] == 5 and i + 2 < len(codes):
                state[key] = xterm256(codes[i + 2])
                i += 2
        elif code == 39:
            state.pop("fg", None)
        elif code == 49:
            state.pop("bg", None)
        elif 30 <= code <= 37:
            state["fg"] = BASIC[code - 30]
        elif 40 <= code <= 47:
            state["bg"] = BASIC[code - 40]
        elif 90 <= code <= 97:
            state["fg"] = BASIC[code - 82]
        elif 100 <= code <= 107:
            state["bg"] = BASIC[code - 92]
        i += 1


def cells(line):
    """Yield (column, width, char, style) for each printed character."""
    state, column, pos = {}, 0, 0
    for match in list(SGR.finditer(line)) + [None]:
        end = match.start() if match else len(line)
        for char in line[pos:end]:
            width = 2 if unicodedata.east_asian_width(char) in "WF" else 1
            yield column, width, char, tuple(sorted(state.items(), key=str))
            column += width
        if match:
            apply_sgr(state, match.group(1))
            pos = match.end()


def css(style):
    style = dict(style)
    fg, bg = style.get("fg", FOREGROUND), style.get("bg")
    if style.get(7):
        fg, bg = bg or BACKGROUND, fg
    rules = ["color:rgb(%d,%d,%d)" % fg]
    if bg:
        rules.append("background:rgb(%d,%d,%d)" % bg)
    if style.get(1):
        rules.append("font-weight:bold")
    if style.get(2):
        rules.append("opacity:.6")
    if style.get(3):
        rules.append("font-style:italic")
    if style.get(4):
        rules.append("text-decoration:underline")
    return ";".join(rules)


def render(lines):
    out = []
    for row, line in enumerate(lines):
        run = None  # [column, width, text, style]
        for column, width, char, style in cells(line):
            # Plain-ASCII runs share a span; anything else gets its own cell.
            if run and char.isascii() and run[2].isascii() and run[3] == style:
                run[1] += width
                run[2] += char
                continue
            if run:
                out.append(span(row, *run))
            run = [column, width, char, style]
        if run:
            out.append(span(row, *run))
    return "".join(out)


def span(row, column, width, text, style):
    if text.isspace() and "bg" not in dict(style) and not dict(style).get(7):
        return ""
    return '<span style="left:%.2fpx;top:%dpx;width:%.2fpx;%s">%s</span>' % (
        PADDING + column * CELL_WIDTH,
        PADDING + row * LINE_HEIGHT,
        width * CELL_WIDTH + 0.5,
        css(style),
        html.escape(text),
    )


def main():
    source, target, font_dir = sys.argv[1:4]
    with open(source, encoding="utf-8") as f:
        lines = f.read().rstrip("\n").split("\n")
    columns = max(sum(w for _, w, _, _ in cells(line)) for line in lines)
    width = round(2 * PADDING + columns * CELL_WIDTH)
    height = 2 * PADDING + len(lines) * LINE_HEIGHT
    page = """<!doctype html><meta charset="utf-8"><style>
@font-face{font-family:Grid;src:url("file://%(dir)s/DejaVuSansMono.ttf")}
@font-face{font-family:Grid;font-weight:bold;src:url("file://%(dir)s/DejaVuSansMono-Bold.ttf")}
html,body{margin:0;width:%(w)dpx;height:%(h)dpx;background:rgb%(bg)s;overflow:hidden}
span{position:absolute;height:%(lh)dpx;line-height:%(lh)dpx;white-space:pre;
font:%(fs)dpx Grid,"DejaVu Sans",monospace}
</style><body>%(body)s</body>""" % {
        "dir": font_dir,
        "w": width,
        "h": height,
        "bg": str(BACKGROUND),
        "lh": LINE_HEIGHT,
        "fs": FONT_SIZE,
        "body": render(lines),
    }
    with open(target, "w", encoding="utf-8") as f:
        f.write(page)
    print("%d,%d" % (width, height))


if __name__ == "__main__":
    main()
