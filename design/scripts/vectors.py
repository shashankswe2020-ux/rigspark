"""RigSpark vector kit: logo mark, outlined wordmark, lockups, icons, badges, illustrations, favicon.
Text is outlined via fontTools so SVGs never depend on installed fonts.
"""
import io, json, pathlib, brotli
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen

ROOT = pathlib.Path("/home/claude/kit/site/brand")
FS = pathlib.Path("/home/claude/fonts/node_modules/@fontsource")
C = dict(paper="#F5F5F7", paper50="#FFFFFF", ink="#1D1D1F", ink7="#424245", ink5="#6E6E73", peach="#F9B08B",
         spark="#FFAD00", spark6="#F08C00", yes="#2E9A4C", slow="#C98400", no="#E0503A", gold="#FFCB4A", teal="#1A959B", coral="#FF6B55")

def font(fam, w):
    return TTFont(io.BytesIO(open(FS / fam / "files" / f"{fam}-latin-{w}-normal.woff2", "rb").read()))

class R1(SVGPathPen):
    """SVG path pen with coordinates rounded to 1 decimal (keeps wordmarks tiny)."""
    def _r(self, v): return f"{round(v, 1):g}"
    def _moveTo(self, p): self._commands.append(f"M{self._r(p[0])} {self._r(p[1])}")
    def _lineTo(self, p): self._commands.append(f"L{self._r(p[0])} {self._r(p[1])}")
    def _curveToOne(self, a, b, c): self._commands.append("C" + " ".join(f"{self._r(x)} {self._r(y)}" for x, y in (a, b, c)))
    def _qCurveToOne(self, a, b): self._commands.append("Q" + " ".join(f"{self._r(x)} {self._r(y)}" for x, y in (a, b)))

def text_path(txt, fam="inter", w=700, size=64, x=0, y=0, tracking=0.0):
    f = font(fam, w); gs = f.getGlyphSet(); cmap = f.getBestCmap(); upm = f["head"].unitsPerEm
    s = size / upm; pen = R1(gs); cx = 0.0
    for ch in txt:
        g = cmap[ord(ch)]
        gs[g].draw(TransformPen(pen, (s, 0, 0, -s, x + cx, y)))
        cx += gs[g].width * s + tracking * size
    return pen.getCommands(), cx - tracking * size

def svg(w, h, body, vb=None):
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="{vb or f"0 0 {w} {h}"}" fill="none">{body}</svg>\n'

def save(rel, s):
    p = ROOT / rel; p.parent.mkdir(parents=True, exist_ok=True); p.write_text(s); return p

# ---------------- logo mark ----------------
# Sparky: the RigSpark mascot (graphite tower bot, amber visor, bolt) reduced to a 64px face mark.
BODY_D = "M32 5C45.5 5 54 13.5 54 27V52C54 56.4 50.4 60 46 60H18C13.6 60 10 56.4 10 52V27C10 13.5 18.5 5 32 5Z"
BOLT_D = "M46.2 21.5 42 27.6H44.6L43.2 32.4 47.6 26.1H45L46.2 21.5Z"
def mark_body(ink=C["ink"], spark=C["spark"], bg=None, face=None):
    face = face or ink
    return (f'<path d="{BODY_D}" fill="{ink}"/>'
            f'<rect x="14" y="21" width="36" height="19" rx="8.5" fill="{spark}"/>'
            f'<ellipse cx="24" cy="30.2" rx="2.7" ry="3.6" fill="{face}"/><ellipse cx="36.5" cy="30.6" rx="2.7" ry="3.5" fill="{face}"/>'
            f'<path d="M28 34.4q2.2 2.4 4.4 0" stroke="{face}" stroke-width="1.9" stroke-linecap="round"/>'
            f'<path d="{BOLT_D}" fill="{face}"/>')

def mark_mono():
    return ('<defs><mask id="v"><rect width="64" height="64" fill="#fff"/><rect x="14" y="21" width="36" height="19" rx="8.5" fill="#000"/>'
            '<ellipse cx="24" cy="30.2" rx="2.7" ry="3.6" fill="#fff"/><ellipse cx="36.5" cy="30.6" rx="2.7" ry="3.5" fill="#fff"/>'
            '<path d="M28 34.4q2.2 2.4 4.4 0" stroke="#fff" stroke-width="1.9" stroke-linecap="round" fill="none"/>'
            f'<path d="{BOLT_D}" fill="#fff"/></mask></defs><path d="{BODY_D}" fill="currentColor" mask="url(#v)"/>')

save("logo/mark.svg", svg(64, 64, mark_body()))
save("logo/mark-dark.svg", svg(64, 64, mark_body(ink=C["paper"], face=C["ink"])))
save("logo/mark-mono.svg", svg(64, 64, mark_mono()))

# ---------------- wordmark + lockups ----------------
def wordmark(color_rig, color_spark, size=48):
    d1, w1 = text_path("Rig", size=size, y=size * 0.78, tracking=-0.02)
    d2, w2 = text_path("Spark", size=size, x=w1 - size * 0.02 + 0.5, y=size * 0.78, tracking=-0.02)
    return f'<path d="{d1}" fill="{color_rig}"/><path d="{d2}" fill="{color_spark}"/>', w1 + w2, size

wm, ww, wh = wordmark(C["ink"], C["ink"])
save("logo/wordmark.svg", svg(round(ww + 2), round(wh), wm))
wm2, _, _ = wordmark(C["paper"], C["paper"])
save("logo/wordmark-dark.svg", svg(round(ww + 2), round(wh), wm2))

def lockup(ink, spark, name, bg=None):
    s = 64; gap = 14; wm, ww, wh = wordmark(ink, ink, 46)
    W = s + gap + ww + 4; H = s
    rect = f'<rect width="{W:.0f}" height="{H}" fill="{bg}"/>' if bg else ""
    body = rect + mark_body(ink, spark, face=C["ink"]) + f'<g transform="translate({s+gap} {(H-wh)/2+1:.1f})">{wm}</g>'
    save(f"logo/{name}.svg", svg(round(W), H, body))
lockup(C["ink"], C["spark"], "lockup")
lockup(C["paper"], C["spark"], "lockup-dark")  # body turns cream on dark

# favicon + app icon (svg)
save("logo/favicon.svg", svg(32, 32, mark_body(), vb="2 2 60 60"))
save("logo/app-icon.svg", svg(1024, 1024,
     f'<defs><radialGradient id="g" cx=".5" cy=".35" r=".8"><stop offset="0" stop-color="#FCC6A8"/><stop offset="1" stop-color="{C["peach"]}"/></radialGradient></defs>'
     f'<rect width="1024" height="1024" rx="228" fill="url(#g)"/>'
     f'<ellipse cx="512" cy="900" rx="300" ry="34" fill="{C["ink"]}" opacity=".14"/>'
     f'<g transform="translate(96 84) scale(13)">{mark_body()}</g>'))

# ---------------- icons (24px, 1.75 stroke, round, currentColor) ----------------
I = {
 "chip": '<rect x="6" y="6" width="12" height="12" rx="2.5"/><path d="M9.5 2.5v3.5M14.5 2.5v3.5M9.5 18v3.5M14.5 18v3.5M2.5 9.5H6M2.5 14.5H6M18 9.5h3.5M18 14.5h3.5"/><path d="M12 9.2c.3 1.7 1.1 2.5 2.8 2.8-1.7.3-2.5 1.1-2.8 2.8-.3-1.7-1.1-2.5-2.8-2.8 1.7-.3 2.5-1.1 2.8-2.8Z"/>',
 "memory": '<rect x="2.5" y="6" width="19" height="10" rx="1.5"/><path d="M6 9.5v3M10 9.5v3M14 9.5v3M18 9.5v3M5 16v2.5M9 16v2.5M15 16v2.5M19 16v2.5"/>',
 "gauge": '<path d="M4.5 17.5a8.5 8.5 0 1 1 15 0"/><path d="M12 13.5l4-4.5"/><circle cx="12" cy="14" r="1.2"/>',
 "verdict-yes": '<circle cx="12" cy="12" r="9.25"/><path d="m8 12.3 2.7 2.7L16.2 9.5"/>',
 "verdict-slow": '<circle cx="12" cy="12" r="9.25"/><path d="M12 7.5V12l3 2"/>',
 "verdict-no": '<circle cx="12" cy="12" r="9.25"/><path d="m9 9 6 6M15 9l-6 6"/>',
 "terminal": '<rect x="2.5" y="4" width="19" height="16" rx="2.5"/><path d="m6.5 9.5 3 2.5-3 2.5M12 15h5"/>',
 "shield": '<path d="M12 2.8 4.5 5.6v6.1c0 4.6 3.1 8.3 7.5 9.5 4.4-1.2 7.5-4.9 7.5-9.5V5.6L12 2.8Z"/><path d="m8.8 12 2.2 2.2 4.2-4.4"/>',
 "loopback": '<path d="M20 12a8 8 0 1 1-2.35-5.65"/><path d="M20 3.5V7h-3.5"/><circle cx="12" cy="12" r="2"/>',
 "offline": '<path d="M2.5 8.8a14 14 0 0 1 4.3-2.6M10.6 5.6A14 14 0 0 1 21.5 8.8M5.5 12.2a9.5 9.5 0 0 1 3.4-1.9M14.6 10.5a9.5 9.5 0 0 1 3.9 1.7M8.6 15.6a5 5 0 0 1 6.8 0"/><path d="M3 3l18 18"/><circle cx="12" cy="19" r=".6"/>',
 "download": '<path d="M12 3.5v11M7.5 10.5 12 15l4.5-4.5M4 19.5h16"/>',
 "star": '<path d="m12 3.2 2.7 5.5 6 .9-4.35 4.2 1 6-5.35-2.8-5.35 2.8 1-6L3.3 9.6l6-.9L12 3.2Z"/>',
 "layers": '<path d="m12 3 9 4.8-9 4.8-9-4.8L12 3Z"/><path d="m3 12.2 9 4.8 9-4.8M3 16.4l9 4.8 9-4.8"/>',
 "catalog": '<ellipse cx="12" cy="5.5" rx="7.5" ry="2.75"/><path d="M4.5 5.5v13c0 1.5 3.4 2.75 7.5 2.75s7.5-1.25 7.5-2.75v-13M4.5 12c0 1.5 3.4 2.75 7.5 2.75s7.5-1.25 7.5-2.75"/>',
 "chat": '<path d="M20.5 12a8 8 0 0 1-11.7 7.1L3.5 20.5l1.4-5A8 8 0 1 1 20.5 12Z"/><path d="M8.5 11h7M8.5 14h4"/>',
 "image": '<rect x="3" y="4" width="18" height="16" rx="2.5"/><circle cx="9" cy="9.5" r="1.75"/><path d="m21 15.5-5-5L5.5 20"/>',
 "spark": '<path d="M12 2.5c.7 5 2.5 6.8 7.5 7.5-5 .7-6.8 2.5-7.5 7.5-.7-5-2.5-6.8-7.5-7.5 5-.7 6.8-2.5 7.5-7.5Z"/><path d="M19 16.5v5M16.5 19h5"/>',
 "bolt": '<path d="M13.5 2.5 5 13.5h6.5L10.5 21.5 19 10.5h-6.5l1-8Z"/>',
 "copy": '<rect x="8.5" y="8.5" width="12" height="12" rx="2.5"/><path d="M15.5 8.5V6A2.5 2.5 0 0 0 13 3.5H6A2.5 2.5 0 0 0 3.5 6v7A2.5 2.5 0 0 0 6 15.5h2.5"/>',
 "arrow-right": '<path d="M4.5 12h15M13.5 6l6 6-6 6"/>',
 "search": '<circle cx="10.5" cy="10.5" r="6.75"/><path d="m15.5 15.5 5 5"/>',
 "book": '<path d="M4 19.5V5a2 2 0 0 1 2-2h13.5v15H6a2 2 0 0 0-2 2Zm0 0a2 2 0 0 0 2 2h13.5"/>',
 "agent": '<rect x="4.5" y="7.5" width="15" height="12" rx="3"/><path d="M12 7.5V4M9.5 13v1M14.5 13v1M2 13v2M22 13v2"/><circle cx="12" cy="3.5" r="1"/>',
 "plug": '<path d="M9 2.5v5M15 2.5v5M6 7.5h12v3a6 6 0 0 1-12 0v-3ZM12 16.5v5"/>',
 "context": '<path d="M4 6h16M4 12h10M4 18h6"/><path d="m17 15 3 3-3 3"/>',
 "check": '<path d="m5 12.5 4.5 4.5L19 7.5"/>',
 "apple": '<path d="M12 7.5c0-2 1.5-4 3.5-4 0 2-1.5 4-3.5 4Z"/><path d="M16.8 12.6c0-2 1.6-3 1.7-3.1-1-1.4-2.4-1.6-2.9-1.6-1.2-.1-2.4.7-3 .7-.7 0-1.6-.7-2.7-.7-1.4 0-2.7.8-3.4 2.1-1.5 2.5-.4 6.3 1 8.4.7 1 1.5 2.1 2.6 2.1 1 0 1.4-.7 2.7-.7 1.2 0 1.6.7 2.7.7 1.1 0 1.8-1 2.5-2 .8-1.2 1.1-2.3 1.1-2.4 0 0-2.3-.9-2.3-3.5Z"/>',
 "linux": '<path d="M12 3c-2 0-3.2 1.7-3.2 4 0 1.4.3 2.4-.6 3.9C7 13 5.5 15 6.2 18c.4 1.7 2.5 2.9 5.8 2.9s5.4-1.2 5.8-2.9c.7-3-.8-5-2-7.1-.9-1.5-.6-2.5-.6-3.9C15.2 4.7 14 3 12 3Z"/><circle cx="10.5" cy="7.5" r=".5"/><circle cx="13.5" cy="7.5" r=".5"/><path d="M10.8 9.6h2.4l-1.2 1.2-1.2-1.2Z"/>',
 "windows": '<path d="M3.5 5.5 10.5 4.5v7h-7v-6ZM13 4.2 20.5 3v8.5H13V4.2ZM3.5 12.5h7v7l-7-1v-6ZM13 12.5h7.5V21L13 19.8v-7.3Z"/>',
}
sheet = []
for n, b in I.items():
    save(f"icons/{n}.svg", f'<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">{b}</svg>\n')
cols = 7; cell = 96; rows = (len(I) + cols - 1) // cols
body = f'<rect width="{cols*cell}" height="{rows*cell+0}" fill="{C["paper50"]}"/>'
for i, (n, b) in enumerate(I.items()):
    x, y = (i % cols) * cell, (i // cols) * cell
    lbl, lw = text_path(n, fam="inter", w=500, size=10)
    body += (f'<g transform="translate({x+30} {y+22}) scale(1.5)" stroke="{C["ink"]}" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">{b}</g>'
             f'<path transform="translate({x+(cell-lw)/2:.1f} {y+80})" d="{lbl}" fill="{C["ink5"]}"/>')
save("icons/_sheet.svg", svg(cols * cell, rows * cell, body))
json.dump(sorted(I), open(ROOT / "icons/index.json", "w"), indent=1)

# ---------------- badges / buttons ----------------
def badge(name, label, value, style):
    H = 32; pad = 12; icon = 16
    lp, lw = text_path(label, fam="inter", w=600, size=12.5)
    vp, vw = text_path(value, fam="jetbrains-mono", w=500, size=12.5)
    L = pad + icon + 8 + lw + pad; W = L + pad + 9 + vw + pad + 2
    st = {"light": (C["paper50"], C["ink"], C["paper"], C["ink"], C["ink"], C["yes"]),
          "dark": (C["ink"], C["paper"], C["ink7"], C["paper"], C["paper"], "#4ADE80"),
          "outline": ("none", C["ink"], "none", C["ink"], C["ink"], C["yes"]),
          "spark": (C["spark"], C["ink"], C["ink"], "#FFFFFF", C["ink"], "#4ADE80")}[style]
    bgL, fgL, bgR, fgR, ink, vcol = st
    stroke = f' stroke="{C["ink"]}" stroke-width="1.5"' if style in ("outline", "light") else ""
    body = (f'<rect x=".75" y=".75" width="{W-1.5:.1f}" height="{H-1.5}" rx="{H/2-1}" fill="{bgL}"/>'
            f'<path d="M{L:.1f} .75H{W-H/2:.1f}A{H/2-.75} {H/2-.75} 0 0 1 {W-H/2:.1f} {H-.75}H{L:.1f}Z" fill="{bgR}"/>'
            + (f'<path d="M{L:.1f} 1V{H-1}" stroke="{C["ink"]}" stroke-width="1.5"/>' if style in ("outline", "light") else "")
            + f'<g transform="translate({pad} 8) scale(.25)">{mark_body(C["ink"] if style!="dark" else C["paper"], C["spark"], face=C["ink"])}</g>'
            f'<path transform="translate({pad+icon+8} 20.5)" d="{lp}" fill="{fgL}"/>'
            f'<circle cx="{L+pad+1}" cy="16" r="3.5" fill="{vcol}"/>'
            f'<path transform="translate({L+pad+9} 20.5)" d="{vp}" fill="{fgR}"/>'
            + (f'<rect x=".75" y=".75" width="{W-1.5:.1f}" height="{H-1.5}" rx="{H/2-1}" fill="none"{stroke}/>' if stroke else ""))
    save(f"buttons/{name}-{style}.svg", svg(round(W), H, body))

for s in ("light", "dark", "outline", "spark"):
    badge("checked-with-rigspark", "Checked with RigSpark", "yes · 16 GB", s)

def button(name, label, icon, bg, fg, border=None):
    lp, lw = text_path(label, fam="inter", w=600, size=15)
    H = 48; W = 20 + 20 + 10 + lw + 22
    b = f' stroke="{border}" stroke-width="1.5"' if border else ""
    body = (f'<rect x=".75" y=".75" width="{W-1.5:.1f}" height="{H-1.5}" rx="12" fill="{bg}"{b}/>'
            f'<g transform="translate(20 14) scale(.8333)" stroke="{fg}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{I[icon]}</g>'
            f'<path transform="translate(50 29.5)" d="{lp}" fill="{fg}"/>')
    save(f"buttons/{name}.svg", svg(round(W), H, body))
button("download-macos", "Download for macOS", "apple", C["ink"], C["paper50"])
button("download-spark", "Download RigSpark", "download", C["spark"], C["ink"], C["ink"])
button("star-github", "Star on GitHub", "star", C["paper50"], C["ink"], C["ink"])
button("read-docs", "Read the docs", "book", "none", C["ink"], C["ink"])

# ---------------- illustrations ----------------
save("illustrations/glow.svg", svg(800, 800, f'<defs><radialGradient id="g"><stop offset="0" stop-color="{C["spark"]}" stop-opacity=".55"/><stop offset=".45" stop-color="#FCC6A8" stop-opacity=".22"/><stop offset="1" stop-color="{C["spark"]}" stop-opacity="0"/></radialGradient></defs><circle cx="400" cy="400" r="400" fill="url(#g)"/>'))
trace = "M0 24H220l16-16h120l16 16h180l16 16h140l16-16h220l16 16h240"
save("illustrations/trace-divider.svg", svg(1200, 48, f'<path d="{trace}" stroke="{C["ink"]}" stroke-opacity=".18" stroke-width="1.5"/>'
     + "".join(f'<circle cx="{x}" cy="{y}" r="4" fill="{c}"/>' for x, y, c in [(236, 8, C["spark"]), (568, 40, C["yes"]), (724, 40, C["slow"]), (960, 24, C["no"])])))
vb = ""
for i, (lbl, c, w) in enumerate([("yes", C["yes"], 0.46), ("slow", C["slow"], 0.30), ("no", C["no"], 0.24)]):
    x = sum([0.46, 0.30, 0.24][:i]) * 600
    vb += f'<rect x="{x+ (2 if i else 0):.0f}" y="0" width="{w*600-4:.0f}" height="14" rx="7" fill="{c}"/>'
save("illustrations/verdict-bar.svg", svg(600, 14, vb))

# ---------------- Sparky expressions (verdict mascots) ----------------
def face(kind, visor):
    ink = C["ink"]
    eyes = {
      "happy": '<ellipse cx="24" cy="30.2" rx="2.7" ry="3.6"/><ellipse cx="36.5" cy="30.6" rx="2.7" ry="3.5"/>',
      "sleepy": f'<path d="M21 31h6M33.5 31h6" stroke="{ink}" stroke-width="2.2" stroke-linecap="round"/>',
      "sad": f'<path d="M21.5 28l5 4.5M26.5 28l-5 4.5M34 28l5 4.5M39 28l-5 4.5" stroke="{ink}" stroke-width="2.1" stroke-linecap="round"/>',
      "wow": '<circle cx="24" cy="29.8" r="3.4"/><circle cx="36.5" cy="30.2" r="3.4"/>',
      "wink": f'<ellipse cx="24" cy="30.2" rx="2.7" ry="3.6"/><path d="M33.5 31q3-2.6 6 0" stroke="{ink}" stroke-width="2.1" stroke-linecap="round" fill="none"/>',
    }[kind]
    mouth = {
      "happy": "M28 34.4q2.2 2.4 4.4 0", "wink": "M27.6 34.2q2.6 2.8 5.2 0",
      "sleepy": "M28.6 35.4h3.4", "sad": "M28 36.4q2.2-2.2 4.4 0", "wow": None}[kind]
    m = f'<path d="{mouth}" stroke="{ink}" stroke-width="1.9" stroke-linecap="round" fill="none"/>' if mouth else f'<ellipse cx="30.2" cy="35.6" rx="1.8" ry="2.1" fill="{ink}"/>'
    return (f'<path d="{BODY_D}" fill="{ink}"/><rect x="14" y="21" width="36" height="19" rx="8.5" fill="{visor}"/>'
            f'<g class="eyes" fill="{ink}" style="transition:transform .15s">{eyes}</g>{m}<path d="{BOLT_D}" fill="{ink}"/>')
for kind, visor, name in [("happy", C["spark"], "sparky-yes"), ("sleepy", "#FFCB4A", "sparky-slow"), ("sad", C["coral"], "sparky-no"),
                          ("wow", C["spark"], "sparky-wow"), ("wink", C["spark"], "sparky-wink")]:
    save(f"illustrations/{name}.svg", svg(64, 64, face(kind, visor)))
save("illustrations/sparky-zzz.svg", svg(40, 40, f'<path d="M6 10h9l-9 10h9M22 4h8l-8 9h8" stroke="{C["ink"]}" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" fill="none"/>'))
save("illustrations/bolt-sticker.svg", svg(48, 48, f'<path d="M28 3 9 27h13l-4 18 21-26H26l2-16Z" fill="{C["spark"]}" stroke="{C["ink"]}" stroke-width="3" stroke-linejoin="round"/>'))
save("illustrations/sparkle.svg", svg(40, 40, f'<path d="M20 2c1.2 9.5 4.5 13 14 14-9.5 1.2-12.8 4.5-14 14-1.2-9.5-4.5-12.8-14-14 9.5-1.2 12.8-4.5 14-14Z" fill="{C["ink"]}"/>'))
wave = "M0 20 " + " ".join(f"Q{30+i*60} {4 if i%2==0 else 36} {60+i*60} 20" for i in range(20))
save("illustrations/squiggle.svg", svg(1200, 40, f'<path d="{wave}" stroke="{C["ink"]}" stroke-width="3" stroke-linecap="round" fill="none"/>'))
save("illustrations/bubble.svg", svg(220, 120, f'<path d="M24 4h172a20 20 0 0 1 20 20v52a20 20 0 0 1-20 20H70l-26 20 4-20H24A20 20 0 0 1 4 76V24A20 20 0 0 1 24 4Z" fill="#fff" stroke="{C["ink"]}" stroke-width="3" stroke-linejoin="round"/>'))

print("vectors ok:", sum(1 for _ in ROOT.rglob("*.svg")), "svgs")
