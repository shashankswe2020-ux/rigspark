# RigSpark × Sparky: Apple-style landing redesign kit (v3)

This is a redesign of [rigspark.si](https://www.rigspark.si/) in the **apple.com product-page style**: lots of white space, big centred type, a single accent colour and studio product shots. The product being shot is the mascot. The B1 robot from `~/rigspark-mascot` (graphite body, amber visor, lightning bolt) is rebuilt in 3D as **"Sparky"** and photographed the way Apple shoots hardware.

> **Placeholder name:** "Sparky" is a working name. Rename it with a find/replace in `design/redesign/landing.src.html`, `site/brand/**/sparky-*` and the Figma components.

- **Figma:** https://www.figma.com/design/FrHXEqYrQlnAK0q1MfZjzG. The pages are **Landing — Apple × Sparky** (current), Foundations (variables and styles re-themed for v3), Assets, and *Landing v2 (sticker, superseded)*.
- **Prototype:** open `design/redesign/landing.html`. It works offline with self-hosted fonts.
- **Preview:** `design/redesign/preview-full.jpg`

---

## 1. Competitive audit (rival: Ollama, plus LM Studio)

| Rival does X | RigSpark today | Redesign move |
|---|---|---|
| Ollama has a minimal page, and its llama appears only as a logo | Busy, neo-brutalist page; the mascot appears once | **Sparky is the product hero.** A scroll-scrubbed 360° turntable, Apple "colours" lineup and studio shots make the mascot the brand |
| Ollama leads with scale proof (installs, logo wall) | Facts only | "Small robot. Big numbers.": **69 / 4 / 0 / MIT**. All real; no invented counts or logos |
| LM Studio sells "the agent for your computer" with one large screenshot | Screens spread across many sections | Apple homepage-style **half-width tiles** (Terminal UI · Local image & video · Workspace · AI Hardware Score) with "Learn more ›" links |
| Both claim privacy | Trust section is buried | Black **"Your hardware. Your data. Your call."** section in the style of Apple's privacy pages, with Sparky in a spotlight and four pillars |
| No interactive demo | Static verdict copy | **"Ask Sparky" configurator** in the style of Apple's Buy pages: pick memory and model, and a 3D Sparky crossfades between happy, sleepy and X-eyed |
| Ollama is "the runtime" | RigSpark vs Ollama table | **"Which is right for you?"** with two columns: *A runtime alone* vs *Runtime + RigSpark*. RigSpark is positioned as the layer above all four runtimes |
| Footer: legal + socials | Docs links | Apple-style footer with a fine-print disclaimer, breadcrumb and a **Compare** column for SEO pages |

Roadmap items (terminal chat, lifecycle checklist) are marked **Next**. Figures come from rigspark.si: the Q4_K_M sizes and published tok/s ranges.

## 2. Design language

- **Palette:** white `#FFFFFF`, tile grey `#F5F5F7`, hairline `#E8E8ED`, ink `#1D1D1F`, secondary text `#6E6E73`. Amber `#FFAD00` is Sparky's visor colour and the only accent. It's used on the primary pill, and darker amber `#B35F00` on "Learn more ›" links. Verdict colours are `#248A3D` (yes), amber (slow) and `#D70015` (no). Night sections use black with `#F5F5F7`/`#A1A1A6` text.
- **Type:** Inter for everything, self-hosted in `site/brand/fonts/`. Display text is 700 weight with −0.035em tracking (`clamp(48px, 8vw, 96px)`). The CSS stack falls back to SF Pro on Apple devices.
- **Shape and motion:** 28px tiles, 980px pills, no borders. Sections fade up as they scroll in (`cubic-bezier(.28,.11,.32,1)`). The spin is pinned with sticky positioning and caption swaps, and `prefers-reduced-motion` turns the motion off.
- **Files:** `site/brand/tokens/tokens.css` and `tokens.json`. Figma variables and text styles carry the same values.

## 3. Asset inventory

| Path | What | Source |
|---|---|---|
| `site/brand/3d/turntable/sparky-00…35.webp` | 36-frame 360° turntable, 799×629, ~1 MB total, transparent | Blender 5.2 Cycles, `scene.py turn` with `ANGLE` env |
| `site/brand/3d/sparky-lineup.{png,webp}` | Yes / slow / no "colours" lineup | `scene.py lineup` |
| `site/brand/3d/sparky-{yes,slow,no}-3d.{png,webp}` | Matching single-expression renders for the configurator crossfade | `scene.py turn` with `EXPR=yes\|slow\|no` |
| `site/brand/3d/sparky-studio.{png,webp}` | Studio hero shot (privacy section, OG) | `scene.py studio` |
| `site/brand/3d/sparky-{cheer,chip,wave,hero,peek}.*` | Pose renders (CTA, Hardware Score tile, extras) | earlier passes, same rig |
| `site/brand/photo/desk-sparky.{jpg,webp}` | Sparky vinyl toy on a desk with a laptop | Runway Nano Banana Pro |
| `site/brand/video/sparky-loop.*` | Peach-background waving loop | Runway Seedance 2 Fast. *Not used by v3*, which uses the turntable; kept for social posts |
| `site/brand/logo/*` | Sparky face mark (light/dark/mono), Inter wordmark and lockups, favicons, app icons | `vectors.py`, `render_social.py` |
| `site/brand/illustrations/sparky-*.svg` | Flat Sparky expressions (yes/slow/no/wow/wink) | `vectors.py` |
| `site/brand/icons/*.svg` | 30 icons, 24px, 1.75 stroke, `currentColor` | `vectors.py` |
| `site/brand/buttons/*` | "Checked with RigSpark" badges and buttons, text outlined | `vectors.py` |
| `site/brand/social/og-image.png` | 1200×630 OG card: white, studio Sparky | `social.html` → `render_social.py` |
| `site/brand/screens/*.webp` | WebP copies of your product screenshots | converted |
| `design/blender/*.blend` | hero, cheer, chip, lineup and studio scenes | Blender |

## 4. Using it on the site

```html
<link rel="icon" href="/brand/logo/favicon.svg" type="image/svg+xml">
<meta property="og:image" content="https://rigspark.si/brand/social/og-image.png">
<link rel="stylesheet" href="/brand/fonts/fonts.css">
<link rel="stylesheet" href="/brand/tokens/tokens.css">
```

**Scroll-scrubbed turntable:** this is the core of the effect, and the full version is in `landing.src.html`.

```html
<section class="spin" style="height:260vh"><div class="stick" style="position:sticky;top:52px;height:calc(100vh - 52px)">
  <img id="spinimg" src="/brand/3d/turntable/sparky-00.webp" width="799" height="629" alt="Sparky">
</div></section>
<script>
const N=36, F=[...Array(N)].map((_,i)=>`/brand/3d/turntable/sparky-${String(i).padStart(2,'0')}.webp`);
F.forEach(s=>{new Image().src=s});
addEventListener('scroll',()=>{const s=document.querySelector('.spin'),r=s.getBoundingClientRect();
  const p=Math.min(1,Math.max(0,-r.top/(s.offsetHeight-innerHeight)));
  spinimg.src=F[Math.min(N-1,Math.floor(p*N))]},{passive:true});
</script>
```

If you move to Next.js, put `site/brand` under `public/brand` and use `<Image src="/brand/3d/sparky-studio.webp" width={1400} height={1115} alt="Sparky" priority />`.

The "Ask Sparky" logic uses the published Q4_K_M sizes, treats 75% of memory as usable, and marks dense models of 27B and up as `slow`. Swap in real `rigspark can-run --json` output when you can.

## 5. Regenerating

```bash
pip install bpy fonttools brotli pillow playwright
npm i @fontsource/inter @fontsource/jetbrains-mono          # adjust FS path in vectors.py

python3 design/blender/scene.py studio out.png 1400 40     # shots: studio | lineup | turn | hero | wave | cheer | chip | peek
EXPR=slow ANGLE=-12 python3 design/blender/scene.py turn out.png 1000 32
for i in $(seq 0 35); do ANGLE=$((-25+i*10)) python3 design/blender/scene.py turn f$(printf %02d $i).png 800 16; done   # ~19 s/frame on 2 CPUs
python3 design/scripts/postprocess_render.py out.png site/brand/3d/sparky-studio   # alpha floor, trim, webp
python3 design/scripts/softshadow.py site/brand/3d/sparky-studio.png               # fade floor shadow edges
python3 design/scripts/tokens.py && python3 design/scripts/vectors.py
python3 design/scripts/build_landing.py                    # landing.src.html -> landing.html (inlines icons + data)
python3 design/scripts/render_social.py                    # OG + app icon PNGs
python3 design/scripts/qa_screenshot.py 1440 full.png      # full-page QA (also 390)
```

Scripts use absolute paths from the build box (`/home/claude/...`), so update the `ROOT`/`FS` constants before running them locally.

## 6. Caveats

- **Placeholders:** the name "Sparky" and the "Next" features. No stats, quotes or logos are invented.
- **Apple influence:** the layout and rhythm are borrowed from Apple. There are no Apple assets, fonts, wording or trademarks, so the page stays clearly RigSpark.
- **Figma images:** raster images aren't embedded because uploads to Figma were blocked from the build environment. Every slot is a labelled frame (`IMAGE · site/brand/...`) to drag files into.
- **3D renders:** made in the cloud with Blender 5.2 because the Blender add-on on your Mac wasn't running. The `.blend` files are included.
- **Unused fonts:** v2 used Bricolage Grotesque, and v3 is Inter-only. Delete any stale `site/brand/fonts/bricolage-*` files.
- **Prototype:** `landing.html` is the design prototype. The production page is `site/index.html` with `site/styles.css` and `site/main.js`: same design, but CSP-safe (no inline styles or scripts), with the tested install and FAQ content kept and a lighter mobile layout. "Ask Sparky" there uses the engine's real fit rule (memory − 2 GiB OS reserve, 15% headroom) instead of the prototype's 75% rule.
