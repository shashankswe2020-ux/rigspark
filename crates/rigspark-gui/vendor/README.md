# Browser Dependencies

Native builds embed these exact browser distributions. No npm install, CDN, or
Node process is needed to build or run the Rust GUI or Tauri app.

| Library | Version | Upstream | Integrity |
| --- | --- | --- | --- |
| Marked | 15.0.12 | https://github.com/markedjs/marked | `3e7e7d7feb3e5d58cb6c804f68ab5c24cc7e5eb6270fd6e5cbb9124739217d0c` |
| DOMPurify | 3.4.13 | https://github.com/cure53/DOMPurify | `9ab3d44d73c3e3947f9ab72e0f0bc15c7f1931d60b365ba261fc85fe59013c56` |
| KaTeX | 0.19.0 | https://github.com/KaTeX/KaTeX | npm SHA-512 `v6Tznz3zJ7u3niRCoDTsumM2+HA2XXcCu+WAacCeHD2z3p9A9Ks987o5FzfTGBN0e8A0vjEgIDvLbICrXpdw/Q==`; JS SHA-256 `103a53763cc033bba8d175bf3f0ba597c3505c9b6747dd3f2c7bc2a6bfcc8ae7`; auto-render SHA-256 `e5372d199bcdae8b4de71d0f7ceba72a4ba12774a27c60a6f1f77d03b3228ee4`; CSS SHA-256 `d4ab5b8ee16989b070cdb0ea24bd6ad48fc8df1b787f280f29d3ae4f959f2c00` |

Files were copied without modification from the existing pinned project packages.
Upstream license notices are retained beside them. Include these notices with
native distributions. Any update requires security review, hash updates, and
browser rendering/sanitization regression checks. These are browser scripts,
not a JavaScript backend or an npm launcher.

## Fonts

`rigspark-gui` embeds these WOFF2 fonts for its workspace UI. They are licensed
under the SIL Open Font License 1.1; the full license and copyright notices ship
as `fonts.OFL.txt` (source: `crates/rigspark-gui/static/fonts/LICENSE-OFL.txt`).

| Font | Version | Upstream | License |
| --- | --- | --- | --- |
| Inter | 4.001 | https://github.com/rsms/inter | OFL-1.1 |
| JetBrains Mono | 2.211 | https://github.com/JetBrains/JetBrainsMono | OFL-1.1 |