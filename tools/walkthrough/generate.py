#!/usr/bin/env python3
"""Generate the ikigai code walkthrough PDF from source + an annotation manifest.

The source is sliced live from the committed crates (test modules stripped), so
the document always matches the repository — only the prose lives in
``walkthrough.toml`` as reviewable data. A side-by-side layout puts source on the
left and annotations on the right.

Usage:
    python generate.py                 # build ikigai-core-walkthrough.pdf
    python generate.py --out path.pdf  # build to a chosen path
    python generate.py --check         # verify every annotation anchor still
                                       # resolves against the source (no PDF) —
                                       # run in CI to catch drift. Needs no fonts.

Rendering needs four embeddable TrueType fonts (base-14 PDF fonts render blank in
some viewers, so we embed). Defaults target macOS; override with --sans / --sans-bold
/ --mono / --mono-bold or the IKIGAI_WT_* env vars on other platforms.
"""

import argparse
import os
import re
import sys

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:  # pragma: no cover
    print("error: Python 3.11+ (tomllib) required", file=sys.stderr)
    raise

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))
MANIFEST = os.path.join(HERE, "walkthrough.toml")

# ---- palette ---------------------------------------------------------------
ACCENT = "#1f7199"        # headings / title
KEYWORD = "#0b6e99"       # Rust keywords
TYPE = "#7a3e9d"          # types / CamelCase
COMMENT = "#8a8f98"       # // comments and /// doc comments
STRING = "#2f7d32"        # string literals
ATTR = "#99711f"          # #[attributes]
CODE_FG = "#1a1a1a"
MUTED = "#555555"
RULE = "#dfe2e5"
CODE_BG = "#f6f8fa"

KEYWORDS = {
    "pub", "fn", "struct", "enum", "trait", "impl", "mod", "use", "let", "const",
    "static", "match", "if", "else", "for", "in", "while", "loop", "return",
    "async", "await", "move", "dyn", "ref", "mut", "as", "where", "self", "Self",
    "crate", "super", "type", "unsafe", "extern", "default", "true", "false",
}

# ---- source slicing --------------------------------------------------------

def strip_tests(text):
    """Drop a trailing ``#[cfg(test)] mod tests { ... }`` block (tests live at the
    end of every file in this codebase) so the walkthrough shows only the API."""
    m = re.search(r"\n#\[cfg\(test\)\]\s*\nmod tests", text)
    return text[: m.start()].rstrip() + "\n" if m else text


def slice_cards(text, soft=32, hard=44):
    """Split source into cards that fit a page. Prefer breaking at a blank line
    once past `soft`; force a break at `hard` lines so no card can overflow."""
    lines = text.rstrip("\n").split("\n")
    cards, cur = [], []
    for line in lines:
        cur.append(line)
        at_blank = line.strip() == ""
        if (len(cur) >= soft and at_blank) or len(cur) >= hard:
            cards.append("\n".join(cur).strip("\n"))
            cur = []
    if cur:
        cards.append("\n".join(cur).strip("\n"))
    return [c for c in cards if c.strip()]


# ---- highlighter -----------------------------------------------------------

_TOKEN = re.compile(
    r"""(?P<comment>//[^\n]*)
       |(?P<attr>\#!?\[[^\]]*\])
       |(?P<string>"(?:\\.|[^"\\])*")
       |(?P<word>[A-Za-z_][A-Za-z0-9_]*)
       |(?P<ws>\s+)
       |(?P<other>.)""",
    re.VERBOSE,
)


def _esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def highlight_line(line):
    """Return a reportlab markup string for one source line, with leading
    whitespace preserved and tokens colored."""
    out = []
    for m in _TOKEN.finditer(line):
        kind = m.lastgroup
        tok = m.group()
        if kind == "ws":
            out.append(tok.replace(" ", "&nbsp;").replace("\t", "&nbsp;" * 4))
        elif kind == "comment":
            out.append(f'<font color="{COMMENT}">{_esc(tok)}</font>')
        elif kind == "attr":
            out.append(f'<font color="{ATTR}">{_esc(tok)}</font>')
        elif kind == "string":
            out.append(f'<font color="{STRING}">{_esc(tok)}</font>')
        elif kind == "word":
            if tok in KEYWORDS:
                out.append(f'<font color="{KEYWORD}">{tok}</font>')
            elif tok[0].isupper():
                out.append(f'<font color="{TYPE}">{tok}</font>')
            else:
                out.append(_esc(tok))
        else:
            out.append(_esc(tok))
    return "".join(out) or "&nbsp;"


# ---- manifest / anchors ----------------------------------------------------

def load_manifest():
    with open(MANIFEST, "rb") as f:
        return tomllib.load(f)


def read_source(rel_path):
    with open(os.path.join(REPO_ROOT, rel_path), encoding="utf-8") as f:
        return strip_tests(f.read())


def check_anchors(manifest):
    """Verify every note anchor occurs exactly once in its (test-stripped) file.
    Returns a list of human-readable problems (empty == clean)."""
    problems = []
    for section in manifest.get("file", []):
        try:
            src = read_source(section["path"])
        except FileNotFoundError:
            problems.append(f"{section['path']}: file not found")
            continue
        for note in section.get("note", []):
            anchor = note.get("anchor")
            if anchor is None:
                continue
            n = src.count(anchor)
            if n != 1:
                problems.append(
                    f"{section['path']}: anchor {anchor!r} found {n}× (expected 1)"
                )
    return problems


# ---- rendering -------------------------------------------------------------

def render(manifest, out_path, fonts):
    from reportlab.lib.pagesizes import landscape, letter
    from reportlab.lib.styles import ParagraphStyle
    from reportlab.pdfbase import pdfmetrics
    from reportlab.pdfbase.ttfonts import TTFont
    from reportlab.platypus import (
        BaseDocTemplate, Frame, PageTemplate, Paragraph, Spacer, Table,
        TableStyle, KeepTogether, PageBreak,
    )
    from reportlab.lib import colors

    pdfmetrics.registerFont(TTFont("Body", fonts["sans"]))
    pdfmetrics.registerFont(TTFont("Body-B", fonts["sans_bold"]))
    pdfmetrics.registerFont(TTFont("Mono", fonts["mono"]))
    pdfmetrics.registerFont(TTFont("Mono-B", fonts["mono_bold"]))

    doc_meta = manifest["doc"]
    PAGE = landscape(letter)
    MARGIN = 40
    footer_text = doc_meta.get("footer", "ikigai code walkthrough")

    code_st = ParagraphStyle(
        "code", fontName="Mono", fontSize=7.2, leading=9.4, textColor=colors.HexColor(CODE_FG)
    )
    bullet_st = ParagraphStyle(
        "bullet", fontName="Body", fontSize=8.5, leading=11.5, textColor=colors.HexColor("#1a1a1a"),
        leftIndent=10, bulletIndent=0, spaceAfter=5,
    )
    h_st = ParagraphStyle("h", fontName="Body-B", fontSize=13, leading=16, textColor=colors.HexColor(ACCENT))
    sub_st = ParagraphStyle("sub", fontName="Body", fontSize=9, leading=12, textColor=colors.HexColor(MUTED), spaceAfter=6)
    title_st = ParagraphStyle("title", fontName="Body-B", fontSize=34, leading=38, textColor=colors.HexColor(ACCENT))
    tsub_st = ParagraphStyle("tsub", fontName="Body", fontSize=12, leading=16, textColor=colors.HexColor(MUTED), spaceAfter=18)
    intro_st = ParagraphStyle("intro", fontName="Body", fontSize=10.5, leading=15, textColor=colors.HexColor("#1a1a1a"), spaceAfter=8)

    avail_w = PAGE[0] - 2 * MARGIN
    left_w = avail_w * 0.62
    right_w = avail_w - left_w

    def code_flowable(card):
        markup = "<br/>".join(highlight_line(l) for l in card.split("\n"))
        return Paragraph(markup, code_st)

    def bullets_flowable(bullets):
        if not bullets:
            return Paragraph("", bullet_st)
        items = [Paragraph(f"• {b}", bullet_st) for b in bullets]
        return items

    story = []
    # title page
    story.append(Spacer(1, 150))
    story.append(Paragraph(doc_meta.get("title", "ikigai"), title_st))
    story.append(Paragraph(doc_meta.get("subtitle", ""), tsub_st))
    for para in doc_meta.get("intro", []):
        story.append(Paragraph(para, intro_st))
    story.append(PageBreak())

    for section in manifest.get("file", []):
        src = read_source(section["path"])
        cards = slice_cards(src)
        notes = section.get("note", [])
        header = [
            Paragraph(section["title"], h_st),
            Paragraph(section.get("subtitle", ""), sub_st),
        ]
        for i, card in enumerate(cards):
            bullets = []
            for note in notes:
                if note.get("anchor") and note["anchor"] in card:
                    bullets.extend(note.get("bullets", []))
            left = Table([[code_flowable(card)]], colWidths=[left_w])
            left.setStyle(TableStyle([
                ("BACKGROUND", (0, 0), (-1, -1), colors.HexColor(CODE_BG)),
                ("BOX", (0, 0), (-1, -1), 0.5, colors.HexColor(RULE)),
                ("LEFTPADDING", (0, 0), (-1, -1), 8),
                ("RIGHTPADDING", (0, 0), (-1, -1), 8),
                ("TOPPADDING", (0, 0), (-1, -1), 7),
                ("BOTTOMPADDING", (0, 0), (-1, -1), 7),
                ("VALIGN", (0, 0), (-1, -1), "TOP"),
            ]))
            right = bullets_flowable(bullets)
            row = Table([[left, right]], colWidths=[left_w, right_w])
            row.setStyle(TableStyle([
                ("VALIGN", (0, 0), (-1, -1), "TOP"),
                ("LEFTPADDING", (0, 0), (0, 0), 0),
                ("LEFTPADDING", (1, 0), (1, 0), 14),
                ("RIGHTPADDING", (0, 0), (-1, -1), 0),
                ("TOPPADDING", (0, 0), (-1, -1), 0),
                ("BOTTOMPADDING", (0, 0), (-1, -1), 10),
            ]))
            block = (header + [row]) if i == 0 else [row]
            story.append(KeepTogether(block))
        story.append(Spacer(1, 8))

    def footer(canvas, doc):
        canvas.saveState()
        canvas.setFont("Body", 7.5)
        canvas.setFillColor(colors.HexColor(MUTED))
        canvas.drawCentredString(PAGE[0] / 2, 22, f"{footer_text}  ·  page {doc.page}")
        canvas.restoreState()

    base = BaseDocTemplate(out_path, pagesize=PAGE, leftMargin=MARGIN, rightMargin=MARGIN,
                           topMargin=MARGIN, bottomMargin=MARGIN, title="ikigai code walkthrough")
    frame = Frame(MARGIN, MARGIN, PAGE[0] - 2 * MARGIN, PAGE[1] - 2 * MARGIN, id="body")
    base.addPageTemplates([PageTemplate(id="main", frames=[frame], onPage=footer)])
    base.build(story)


def resolve_fonts(args):
    def pick(flag, env, default):
        return flag or os.environ.get(env) or default
    base = "/System/Library/Fonts/Supplemental"
    fonts = {
        "sans": pick(args.sans, "IKIGAI_WT_SANS", f"{base}/Arial.ttf"),
        "sans_bold": pick(args.sans_bold, "IKIGAI_WT_SANS_BOLD", f"{base}/Arial Bold.ttf"),
        "mono": pick(args.mono, "IKIGAI_WT_MONO", f"{base}/Courier New.ttf"),
        "mono_bold": pick(args.mono_bold, "IKIGAI_WT_MONO_BOLD", f"{base}/Courier New Bold.ttf"),
    }
    missing = [f"{k} ({v})" for k, v in fonts.items() if not os.path.exists(v)]
    if missing:
        sys.exit("error: missing fonts (override with --sans/--mono/... or IKIGAI_WT_*):\n  "
                 + "\n  ".join(missing))
    return fonts


def main():
    ap = argparse.ArgumentParser(description="Generate the ikigai code walkthrough PDF.")
    ap.add_argument("--out", default=os.path.join(HERE, "ikigai-core-walkthrough.pdf"))
    ap.add_argument("--check", action="store_true", help="verify anchors only (no PDF, no fonts)")
    ap.add_argument("--sans"); ap.add_argument("--sans-bold", dest="sans_bold")
    ap.add_argument("--mono"); ap.add_argument("--mono-bold", dest="mono_bold")
    args = ap.parse_args()

    manifest = load_manifest()

    problems = check_anchors(manifest)
    if problems:
        print("anchor check FAILED:", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        sys.exit(1)
    print(f"anchor check ok: {sum(len(s.get('note', [])) for s in manifest['file'])} "
          f"anchors across {len(manifest['file'])} files")

    if args.check:
        return
    render(manifest, args.out, resolve_fonts(args))
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
