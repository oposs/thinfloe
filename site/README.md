# The ThinFloe web page

`index.html` is the page, `ad.html` one screen composed for a screenshot
(the social-media ad); both use `style.css`. `.github/workflows/pages.yml`
publishes this directory to GitHub Pages without `tools/` and this file, and
writes the number from `RELEASE` into `<span data-version>`.

Look at it locally by opening `index.html` in a browser; `index.html?still`
shows the menu picture in place of the boot movie.

- Colours are the menu's (`src/thinfloe-menu/src/brand.rs`) and the boot
  screen's (`brand/boot-glow.svg`, `brand/boot-bar-*.svg`); the logo paths
  are those of `brand/logo-wordmark.svg`.
- `fonts/geo-oblique.woff2` is `src/thinfloe-menu/assets/Geo-Oblique.ttf`,
  `fonts/ubuntu-light.woff2` and `fonts/ubuntu-mono.woff2` are Ubuntu
  (weight 300) and Ubuntu Mono (weight 400), each cut down with `pyftsubset`
  to Latin-1 and a few punctuation marks. Their licences sit beside them.
- The tile icons are Lucide (ISC), as in `src/thinfloe-menu/icons/`.

## Making the pictures again for a release

Everything is program output of the released image; nothing is drawn by
hand. Work in a scratch directory (`$W` below), never in `artifacts/`. The
VM takes 2 cores and 4 GB.

    gh release download vX.Y.Z -R oposs/thinfloe -p thinfloe-usb.img.zst -D $W
    zstd -d $W/thinfloe-usb.img.zst
    mkdir $W/vm && tools/boot-vm.sh $W/thinfloe-usb.img $W/vm 2880 1920 &
    tools/qmp.py $W/vm/qmp-ctl.sock shot now.ppm         # look at the screen
    tools/qmp.py $W/vm/qmp-ctl.sock click X Y 2880 1920  # click a tile

`boot-vm.sh` boots the stick image under OVMF on a qcow2 layer (the image
itself is not written) and gives the machine the demo model name
`ThinkCentre M70q`. The first boot enlarges the stick and takes longer.

1. **Add-ons.** In Settings, Packages, click Install now for ThinLinc and
   OxuLnk (the VM needs the network), then click Restart so the home is
   saved to the stick. Stop the VM with `qmp.py … quit`.
2. **Still pictures** (`img/menu.webp`, `img/menu.png`, `img/settings-packages.webp`,
   `-keyboard`, `-sound`, `-status`). Boot at 2880x1920, open the Terminal tile and
   run a script as root (under `systemd-run`, so it outlives the session) that
   - stops `thinfloe-peripheral-battery.timer` and writes two demo lines to
     `/run/thinfloe/peripherals/battery` every 45 s (`MX Mechanical`, 87,
     keyboard; `MX Anywhere 3S`, 64, mouse),
   - fetches a file from the host in bursts (`curl --limit-rate`, 3 s each,
     50 kB/s to 5 MB/s) so the traffic graph has a shape,
   - writes the monitor's EDID with a size of 398 x 265 mm to
     `/sys/kernel/debug/dri/*/Virtual-1/edid_override` (bytes 21 and 22, the
     size in the timing descriptors, the checksum). QEMU's EDID says 100
     dpi, which gives menu scale 1; this size gives scale 2, a 1440x960
     layout drawn twice as sharp.

   Then `sudo systemctl restart getty@tty1`: the new session reads the new
   size. For the Eject tile, plug a small USB stick into the VM: a raw file
   with a FAT file system (`truncate -s 256M $W/photos.img`, `mkfs.vfat -n
   PHOTOS $W/photos.img`), added over QMP with `blockdev-add` (driver
   `raw`, node name `photos`) and `device_add` (driver `usb-storage`,
   drive `photos`). In Settings, Sound, switch on "Set the volume on this
   machine" and set the slider to about two thirds; the Keyboard tab then
   shows "Volume keys control this machine" on too. Wait five minutes for
   the graph, park the pointer in the corner
   (`qmp.py … move 2879 1919 2880 1920`) and take the shots. The menu
   picture is the whole screen; the Settings pictures are the crop
   `1872x858+504+432` (for 4.12.0 the Keyboard one has the layout list
   closed, so the key switches show). WebP lossless (`cwebp -lossless -z 9`);
   `menu.png` is the same picture with 256 colours (`convert -dither None
   -colors 256 PNG8:`, then `oxipng`).
3. **Network and Screen tabs** (`img/settings-network.webp`, `-screen`).
   The VM has no WiFi, one monitor and no Deskflow computer, so these two
   come from the menu's shot test with staged data (two monitors and two
   attached computers on the Screen tab), at the same size, scale and crop:

       cd src/thinfloe-menu
       TF_SHOT_DIR=$W cargo test -j2 --lib shot_page -- --ignored

4. **Boot movie** (`boot.webm`, `boot.mp4`, `img/boot-poster.webp`). Boot
   at 1440x960 and record from power-on:

       tools/boot-vm.sh $W/thinfloe-usb.img $W/vm 1440 960 &
       tools/qmp.py $W/vm/qmp-rec.sock record $W/frames 15 80

   Send pointer moves to the corner while the session starts (seconds 44
   to 50), or the pointer sits beside the logo. Look at the frames, then
   give `make-movie.sh` the ranges to keep: the boot screen before and
   after the graphics driver takes over (the frames between are garbage),
   two black frames, the menu. For 4.4.0:

       TF_WEBM_CRF=24 TF_MP4_CRF=19 tools/make-movie.sh $W/frames $W . 170-346 359-638 648-650 669-1000

   The movie runs four times as fast as the boot did, which the page says.
   The poster is the first frame of the boot screen.
5. **Ads** (`ad/thinfloe-1200x630.png`, `-1080x1080`, `-1080x1350`, `-social-1280x640`):
   `TMPDIR=$W tools/screenshots.sh --ads`, then `oxipng -o 4 --strip safe ad/*.png`.
   `tools/screenshots.sh DIR` renders `index.html` for a look at desktop and
   phone width. The link preview (`og:image`) is the 1200x630 one.
   `ad/thinfloe-social-1280x640.png` (from `social.html`) is the repository's
   social preview, uploaded by hand under Settings, General, Social preview
   (GitHub has no API for it). Topic pages show it as a 725x275 card, so its
   content stays in the 1280x485 band in the middle.
