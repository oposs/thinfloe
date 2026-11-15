# The ThinFloe web page

`index.html` is the whole page; `.github/workflows/pages.yml` publishes this
directory to GitHub Pages and writes the number from `RELEASE` into
`<span data-version>`.

Look at it locally by opening `index.html` in a browser.

- Colours are the menu's (`src/thinfloe-menu/src/brand.rs`) and the boot
  screen's (`brand/boot-glow.svg`, `brand/boot-bar-*.svg`); the logo paths
  are those of `brand/logo-wordmark.svg`.
- `fonts/geo-oblique.woff2` is `src/thinfloe-menu/assets/Geo-Oblique.ttf`,
  `fonts/ubuntu-light.woff2` and `fonts/ubuntu-mono.woff2` are Ubuntu
  (weight 300) and Ubuntu Mono (weight 400), each cut down with `pyftsubset`
  to Latin-1 and a few punctuation marks. Their licences sit beside them.
- The tile icons are Lucide (ISC), as in `src/thinfloe-menu/icons/`.
- `screenshot-menu.png` is a copy of `docs/screenshot-menu.png`.
