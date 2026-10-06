import json, pathlib
T = {
 "color": {
  # Apple-style neutrals: white canvas, #F5F5F7 tiles, near-black ink
  "paper":   {"50":"#FFFFFF","100":"#F5F5F7","200":"#E8E8ED","300":"#D2D2D7"},
  "ink":     {"900":"#1D1D1F","800":"#2C2C2E","700":"#424245","500":"#6E6E73","400":"#86868B"},
  # Sparky's visor amber = the one brand accent
  "spark":   {"50":"#FFF7DF","100":"#FFE9A8","300":"#FFCB4A","500":"#FFAD00","600":"#F08C00","700":"#B35F00"},
  "peach":   {"100":"#FFEDE3","500":"#F7AE8B"},
  "verdict": {"yes":"#248A3D","yesBg":"#E3F5E6","slow":"#B36B00","slowBg":"#FFF3D6","no":"#D70015","noBg":"#FFE5E5"},
  "night":   {"900":"#000000","800":"#161617","700":"#1D1D1F","text":"#F5F5F7","muted":"#A1A1A6"}
 },
 "font": {"display":"'Inter', -apple-system, BlinkMacSystemFont, 'SF Pro Display', system-ui, sans-serif",
          "ui":"'Inter', -apple-system, BlinkMacSystemFont, 'SF Pro Text', system-ui, sans-serif",
          "mono":"'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, monospace"},
 "radius": {"sm":"12px","md":"18px","lg":"28px","pill":"980px"},
 "space": {str(k):f"{v}px" for k,v in [(1,4),(2,8),(3,12),(4,16),(5,24),(6,32),(7,48),(8,64),(9,96),(10,128),(11,160)]},
 "shadow": {"card":"0 4px 24px rgba(0,0,0,.06)", "float":"0 12px 40px rgba(0,0,0,.12)"},
 "motion": {"ease":"cubic-bezier(.28,.11,.32,1)","spring":"cubic-bezier(.34,1.56,.64,1)","base":"600ms"}
}
out = pathlib.Path("/home/claude/kit/site/brand/tokens"); out.mkdir(parents=True, exist_ok=True)
(out/"tokens.json").write_text(json.dumps(T, indent=2))
lines=[":root {"]
for g,d in T["color"].items():
  for k,v in d.items(): lines.append(f"  --{g}-{k}: {v};")
for k,v in T["font"].items(): lines.append(f"  --font-{k}: {v};")
for k,v in T["radius"].items(): lines.append(f"  --radius-{k}: {v};")
for k,v in T["space"].items(): lines.append(f"  --space-{k}: {v};")
for k,v in T["shadow"].items(): lines.append(f"  --shadow-{k}: {v};")
for k,v in T["motion"].items(): lines.append(f"  --motion-{k}: {v};")
lines.append("}")
(out/"tokens.css").write_text("\n".join(lines)+"\n")
print(open(out/"tokens.css").read()[:400])
