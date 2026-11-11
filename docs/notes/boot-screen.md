This note covers the look of the menu and the boot screen: palette, fonts, shadcn-egui, the Plymouth theme and its step line.

- The look (Part 4): palette, logo and Geo (OFL, `Geo-Oblique.ttf`) live in
  `src/thinfloe-menu/src/brand.rs`; the font is compiled into the menu and also
  installed at `/usr/share/fonts/truetype/thinfloe/` for labwc's title bars
  (`rc.xml` `<theme>`) and for the boot screen: `90thinfloe/module-setup.sh`
  copies it into the initrd, guarded by
  `test/integration/test-initramfs-commands.sh`, so moving the font path breaks
  the boot screen too. Licence at `/usr/share/thinfloe/fonts/Geo-OFL.txt`. Geo
  draws names and titles, small text a plain sans, the log monospace.
  The menu's widgets are shadcn-egui from crates.io, `lyra` style (square in
  every component; owner, 2026-11-06), built once in `brand::theme()`; UI code
  takes colours from the theme and text sizes from named constants in
  `brand.rs` (layout distances are still written in place), and
  `ui_code_has_no_style_literals` fails on a colour or text-size literal.
  The plain sans is egui's Ubuntu Light, set as the theme's body font. Inter
  is linked anyway (shadcn-egui always links it) and sits between Ubuntu
  Light and egui's other fonts, so a glyph Ubuntu Light lacks draws in Inter.
  Its licence ships as
  `/usr/share/thinfloe/fonts/Inter-OFL.txt`, and egui's built-in fonts'
  licences in `/usr/share/thinfloe/fonts/egui/`. Glyphs Geo
  lacks fall back: umlauts and emoji do, but CJK has no font in the image yet
  and draws as boxes (egui's default fonts carry no CJK). The Plymouth theme is
  `overlay/usr/share/plymouth/themes/thinfloe/`, a `script` theme
  (`ModuleName=script` in `thinfloe.plymouth`) whose text label-pango draws,
  finding Geo through fontconfig. Its progress line follows the elapsed time
  (`1 - e^(-t/5)`, `ease()` in `thinfloe.script`), never Plymouth's `progress`:
  that one divides by the boot time kept in `/var/lib/plymouth/boot-duration`,
  which a RAM system never keeps, so it assumed 60 s and ended at 7 to 15 %.
  `test-initramfs-commands.sh` guards it. The logo is never `Image.Scale`d:
  the `plymouth-art` stage renders it at seven widths (320 to 1280 px,
  `logo-<w>.png`) and the script draws the widest one that fits `640 * k`,
  unscaled. Scaling interpolates, and the 640 px logo drawn 1280 px wide on
  4K had edges blurred over 3 px. Plymouth draws all screens from one
  canvas as wide and tall as the largest; a smaller screen shows its centre.
  `layout()` in `thinfloe.script` places everything for that canvas, so the
  largest screen is drawn in full (owner, 2026-10-30), and runs again from
  `Plymouth.SetDisplayHotplugFunction`. Laid out once, the picture made for
  the firmware screen filled the top-left quarter of a 4K monitor after
  i915 bound (seen on the Broadwell box, reproduced in QEMU with
  `virtio-gpu`, which is also left out of the initrd). `DeviceScale=1` in
  `overlay/etc/plymouth/plymouthd.conf` keeps Plymouth from guessing scale 2
  on 4K and doubling the whole screen; the same test guards both. The host name reaches it through
  `plymouth display-message` in `tf_write_hostname`. The version shows in the header, from
  `/etc/thinfloe/version`, which the build writes from `RELEASE` in the
  repository root (one line `X.Y.Z`, else the build fails; a prerequisite
  of `.ubuntu-container`). `test/integration/test-release-version.sh`
  guards the chain, `test-session-packages.sh` the file in the image.
  Above the host name a step line says what the machine does, in English.
  It arrives through `plymouth update --status`: a text that starts with
  `tf:` is shown as it is, anything else is a systemd unit id, which a table
  in `thinfloe.script` (`unit_step`) translates; an id not in the table
  leaves the line as it was. The initramfs sends its steps with `tf_status`
  (`thinfloe-lib.sh`; "Finding the boot stick", "Loading the system",
  "Restoring the home directory"), the running system with
  `/usr/libexec/thinfloe/plymouth-step` (`status`, cut to 64 characters),
  and `thinfloe-addon boot` names each add-on ("Installing ThinLinc").
  The picture stays until the session draws, see
  [the tty1 constraint](history-of-constraints.md#tty1-does-not-wait-for-plymouth-the-session-ends-it).
