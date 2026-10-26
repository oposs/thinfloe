#!/bin/bash
# Asserts the Wayland session stack shipped and the X11 stack did not.
#
# Usage: test/integration/test-session-packages.sh [path/to/ramroot.sqsh]

set -uo pipefail

SQSH="${1:-artifacts/rootfs/ramroot.sqsh}"

if [[ ! -f "$SQSH" ]]; then
    echo "FAIL: $SQSH not found - run 'make rootfs' first" >&2
    exit 1
fi

LIST=$(mktemp)
trap 'rm -f "$LIST"' EXIT
unsquashfs -l "$SQSH" >"$LIST" 2>/dev/null || { echo "FAIL: cannot list $SQSH" >&2; exit 1; }

fail=0
present() {
    if grep -qE -- "$2" "$LIST"; then
        echo "ok   - $1 present"
    else
        echo "FAIL - $1 missing (no match for: $2)"
        fail=1
    fi
}
absent() {
    if grep -qE -- "$2" "$LIST"; then
        echo "FAIL - $1 should have been removed (matched: $2)"
        fail=1
    else
        echo "ok   - $1 absent"
    fi
}

present "labwc"            'usr/bin/labwc$'
# seatd next to logind made libseat try seatd first and fail; see the
# Dockerfile. Not installed today, never enabled if it comes back.
absent  "enabled seatd"    'etc/systemd/system/[^/]+\.wants/seatd\.service$'
present "Xwayland"         'usr/bin/Xwayland$'
present "foot"             'usr/bin/foot$'
present "grim"             'usr/bin/grim$'
present "slurp"            'usr/bin/slurp$'
present "wl-copy"          'usr/bin/wl-copy$'
absent  "waybar"           'usr/bin/waybar$'
absent  "waybar config"    'etc/thinfloe/waybar/'
absent  "waybar user unit enabled" 'systemd/user/graphical-session\.target\.wants/waybar\.service$'
absent  "GTK 3"            'usr/lib/x86_64-linux-gnu/libgtk-3\.so\.0$'
present "session script"    'usr/bin/thinfloe-session$'
present "save-home"         'usr/bin/thinfloe-save-home$'
# The DHCP time zone: the lease option, the watcher and the script that
# validates and sets it (polkit is missing, so networkd cannot).
present "DHCP time zone option" 'etc/systemd/network/10-netplan-eth\.network\.d/50-timezone\.conf$'
present "DHCP time zone script" 'usr/libexec/thinfloe/dhcp-timezone$'
present "DHCP time zone path unit enabled" 'etc/systemd/system/multi-user\.target\.wants/thinfloe-dhcp-timezone\.path$'
present "DHCP time zone service enabled (boot-time run)" 'etc/systemd/system/multi-user\.target\.wants/thinfloe-dhcp-timezone\.service$'
present "labwc config merge" 'usr/bin/thinfloe-compose-labwc-config$'
present "swayidle"          'usr/bin/swayidle$'
present "wlopm"             'usr/bin/wlopm$'
absent  "restore-home script" 'usr/local/bin/thinfloe-restore-home$'
absent  "old save-home path"  'opt/thinlinc/bin/save-home$'
# ThinLinc and OxuLnk are add-ons: the image carries their tiles, never the
# clients. libXrandr stays: vncviewer dlopens it, and the add-on brings none.
absent  "/opt/thinlinc"    '(^|/)opt/thinlinc(/|$)'
absent  "oxulnk-desktop"   'oxulnk-desktop'
present "libXrandr.so.2"   'libXrandr\.so\.2$'
present "ThinLinc audio shim"    'usr/libexec/thinfloe/tlclient-pulseaudio$'
present "ThinLinc post-install"  'usr/libexec/thinfloe/addon-hooks/10-thinlinc$'
present "labwc rc.xml"     'etc/thinfloe/labwc/rc\.xml$'
present "labwc title-bar theme" 'etc/thinfloe/labwc/themerc-override$'
present "Geo font"         'usr/share/fonts/truetype/thinfloe/Geo-Oblique\.ttf$'
present "Geo licence"      'usr/share/thinfloe/fonts/Geo-OFL\.txt$'
present "tty1 autologin"   'getty@tty1\.service\.d/autologin\.conf$'
present "ssh host key helper" 'usr/bin/thinfloe-ssh-hostkeys$'
present "ssh host key unit"   'thinfloe-ssh-hostkeys\.service$'
absent  "old regenerate unit" 'regenerate_ssh_host_keys\.service$'
present "thinfloe-menu"          'usr/bin/thinfloe-menu$'
present "session verdict"     'usr/libexec/thinfloe/session-verdict$'
for t in 10-thinlinc 20-oxulnk 30-terminal 40-save-home 50-upgrade-image 60-settings 80-reboot 81-poweroff; do
    present "tile $t" "etc/thinfloe/menu/$t\.desktop$"
done
for i in hard-drive-download usb log-out rotate-ccw power settings; do
    present "Lucide icon $i" "usr/share/thinfloe/icons/$i\.svg$"
done
present "Lucide licence"      'usr/share/thinfloe/icons/LICENSE$'
present "Oxanium licence"     'usr/share/thinfloe/fonts/Oxanium-OFL\.txt$'

absent  "Xorg server"      'usr/lib/xorg/Xorg$'
absent  "nodm"             'usr/sbin/nodm$'
absent  "wm2"              'usr/bin/wm2$'
absent  "lxterminal"       'usr/bin/lxterminal$'

# Guard the standing constraint: labwc must register no keybindings. Rootless
# Xwayland never requests zwp_keyboard_shortcuts_inhibit_manager_v1 (that was
# this repo's earlier, disproved explanation); the mechanism that matters is
# zwp_xwayland_keyboard_grab_manager_v1, which our labwc now implements but
# which remains unverified on real hardware - so any binding it owns can
# still never reach the remote ThinLinc session. See CLAUDE.md's "labwc must
# have zero keybindings".
RC=$(mktemp -d)
trap 'chmod -R u+rwX "$RC" 2>/dev/null; rm -rf "$LIST" "$RC"' EXIT
unsquashfs -q -f -d "$RC" "$SQSH" 'etc/thinfloe/labwc/rc.xml' 'etc/thinfloe/labwc/themerc-override' >/dev/null 2>&1
# Test for the extracted FILE, not unsquashfs's exit code: extracting a path
# that does not exist in the image still exits 0 (it simply extracts nothing).
# Trusting that exit code made this guard pass vacuously - grep on a missing
# file exits 2, the inner test went false, and the suite printed
# "rc.xml registers no keybindings" for an image that had no rc.xml at all.
if [[ ! -f "$RC/etc/thinfloe/labwc/rc.xml" ]]; then
    echo "FAIL - rc.xml is not present in the image at all"
    fail=1
elif grep -qE '<keybind|<default */>' "$RC/etc/thinfloe/labwc/rc.xml"; then
    echo "FAIL - rc.xml registers keybindings (breaks remote key passthrough)"
    fail=1
else
    echo "ok   - rc.xml registers no keybindings"
fi

# Title bars: the theme override names the ice-blue active border, and rc.xml
# sets the title font to Geo for both window states.
if grep -qiE '^window\.active\.border\.color: *#7cc4ff' "$RC/etc/thinfloe/labwc/themerc-override" 2>/dev/null; then
    echo "ok   - themerc-override gives the active border ice blue"
else
    echo "FAIL - themerc-override missing or without window.active.border.color: #7cc4ff"
    fail=1
fi
if [[ -f "$RC/etc/thinfloe/labwc/rc.xml" ]] &&
   grep -qE '<font place="ActiveWindow" name="Geo"' "$RC/etc/thinfloe/labwc/rc.xml" &&
   grep -qE '<font place="InactiveWindow" name="Geo"' "$RC/etc/thinfloe/labwc/rc.xml"; then
    echo "ok   - rc.xml sets the title font to Geo"
else
    echo "FAIL - rc.xml does not set Geo for ActiveWindow and InactiveWindow"
    fail=1
fi
# No window icon on the left of a title bar: labwc's default layout starts
# with one, and it pushed the title in.
if [[ -f "$RC/etc/thinfloe/labwc/rc.xml" ]] &&
   tr -d '\n' < "$RC/etc/thinfloe/labwc/rc.xml" |
   grep -qE '<titlebar>[[:space:]]*<layout>:iconify,max,close</layout>'; then
    echo "ok   - rc.xml title bars have no window icon"
else
    echo "FAIL - rc.xml title bar layout is not ':iconify,max,close'"
    fail=1
fi

# The menu is the bottom window: a program clicked away must not go behind
# it, and the window list must not offer to minimise it. Matched by app_id.
if [[ -f "$RC/etc/thinfloe/labwc/rc.xml" ]] &&
   tr -d '\n' < "$RC/etc/thinfloe/labwc/rc.xml" |
   grep -qE '<windowRule[^>]*identifier="thinfloe-menu"[^>]*skipTaskbar="yes"[^>]*>[[:space:]]*<action name="ToggleAlwaysOnBottom"'; then
    echo "ok   - rc.xml keeps thinfloe-menu at the bottom and out of the window list"
else
    echo "FAIL - rc.xml has no ToggleAlwaysOnBottom/skipTaskbar rule for thinfloe-menu"
    fail=1
fi

# The labwc in the image must be our local rebuild, not the archive version.
# Guards against the labwc-build stage silently dropping out of the image.
DPKG_TMP=$(mktemp -d)
trap 'chmod -R u+rwX "$RC" "$DPKG_TMP" 2>/dev/null; rm -f "$LIST"; rm -rf "$RC" "$DPKG_TMP"' EXIT
if unsquashfs -no-xattrs -d "$DPKG_TMP/x" "$SQSH" var/lib/dpkg/status >/dev/null 2>&1; then
    labwc_version=$(awk '/^Package: labwc$/{f=1} f&&/^Version:/{print $2; exit}' \
        "$DPKG_TMP/x/var/lib/dpkg/status")
    if [[ "$labwc_version" == *"+thinfloe1" ]]; then
        echo "ok   - labwc is the local rebuild ($labwc_version)"
    else
        echo "FAIL - labwc is '$labwc_version', expected a +thinfloe1 rebuild"
        fail=1
    fi
    # vim-tiny instead of vim (Part 3): vim brought vim-runtime and
    # libpython3.14, 52 MiB. vim-tiny registers vi, view, ex and editor but not
    # vim, so the Dockerfile adds a vim alternative for admins who type vim.
    pkg_status() {
        awk -v p="$1" '$0 == "Package: " p {f=1; next} f && /^Status:/ {print; exit} /^$/ {f=0}' \
            "$DPKG_TMP/x/var/lib/dpkg/status"
    }
    if [[ "$(pkg_status vim-tiny)" == "Status: install ok installed" ]]; then
        echo "ok   - vim-tiny installed"
    else
        echo "FAIL - vim-tiny is not installed"
        fail=1
    fi
    if [[ "$(pkg_status vim)" == "Status: install ok installed" ]]; then
        echo "FAIL - the full vim package is installed (vim-runtime, libpython3.14)"
        fail=1
    else
        echo "ok   - the full vim package is not installed"
    fi
else
    echo "FAIL - cannot extract var/lib/dpkg/status from $SQSH"
    fail=1
fi
if unsquashfs -no-xattrs -d "$DPKG_TMP/alt" "$SQSH" \
        etc/alternatives/vim usr/bin/vim usr/bin/vim.tiny >/dev/null 2>&1 &&
   [[ "$(readlink "$DPKG_TMP/alt/usr/bin/vim")" == "/etc/alternatives/vim" ]] &&
   [[ "$(readlink "$DPKG_TMP/alt/etc/alternatives/vim")" == "/usr/bin/vim.tiny" ]] &&
   [[ -f "$DPKG_TMP/alt/usr/bin/vim.tiny" ]]; then
    echo "ok   - the command vim starts vim.tiny"
else
    echo "FAIL - /usr/bin/vim does not lead to /usr/bin/vim.tiny through etc/alternatives"
    fail=1
fi

present "fullscreen span enabled in labwc environment" \
        "etc/thinfloe/labwc/environment"
if unsquashfs -no-xattrs -d "$DPKG_TMP/env" "$SQSH" \
        etc/thinfloe/labwc/environment >/dev/null 2>&1 &&
   grep -q '^LABWC_FULLSCREEN_SPAN_OUTPUTS=1' \
        "$DPKG_TMP/env/etc/thinfloe/labwc/environment"; then
    echo "ok   - LABWC_FULLSCREEN_SPAN_OUTPUTS is set"
else
    echo "FAIL - LABWC_FULLSCREEN_SPAN_OUTPUTS not set in the shipped environment"
    fail=1
fi

# Rootless Xwayland forwards X11 keyboard grabs via
# zwp_xwayland_keyboard_grab_manager_v1; without it a compositor never learns
# about an XGrabKeyboard call at all. Confirm the shipped binary implements
# the protocol (a static string check, not proof it works - test-runtime's
# headless rig only proves Xwayland binds the manager global, since it has
# no input devices to generate a real grab; see the comment in
# test/runtime/test-labwc-runtime.sh for why a grab_keyboard assertion isn't
# possible there).
if unsquashfs -no-xattrs -d "$DPKG_TMP/bin" "$SQSH" usr/bin/labwc >/dev/null 2>&1 &&
   grep -aq 'zwp_xwayland_keyboard_grab_manager_v1' "$DPKG_TMP/bin/usr/bin/labwc"; then
    echo "ok   - labwc implements zwp_xwayland_keyboard_grab_manager_v1"
else
    echo "FAIL - shipped labwc has no xwayland keyboard grab support"
    fail=1
fi

# Screen blanking needs both halves of the Wayland split: labwc reports
# idleness (ext_idle_notifier_v1) and can power outputs down
# (zwlr_output_power_manager_v1), but has no timeout of its own. Without a
# policy daemon listening, nothing ever blanks - which is what happened when
# the session moved off X11, where the server did this internally.
for proto in ext_idle_notifier_v1 zwlr_output_power_manager_v1; do
    if grep -aq "$proto" "$DPKG_TMP/bin/usr/bin/labwc" 2>/dev/null; then
        echo "ok   - labwc implements $proto"
    else
        echo "FAIL - shipped labwc has no $proto (screen blanking cannot work)"
        fail=1
    fi
done

# The other half: the session must actually run the idle daemon, and must
# blank via wlopm rather than wlr-randr. 'wlr-randr --off' speaks the output
# *management* protocol and disables the output outright - that changes the
# monitor layout and resizes the fullscreen ThinLinc client. wlopm speaks
# output *power* management, which is real DPMS and leaves the layout alone.
if unsquashfs -no-xattrs -d "$DPKG_TMP/idle" "$SQSH" usr/bin/thinfloe-session >/dev/null 2>&1 &&
   [[ -f "$DPKG_TMP/idle/usr/bin/thinfloe-session" ]]; then
    SESS="$DPKG_TMP/idle/usr/bin/thinfloe-session"
    if grep -q 'swayidle' "$SESS"; then
        echo "ok   - session starts swayidle"
    else
        echo "FAIL - thinfloe-session does not start swayidle (screen never blanks)"
        fail=1
    fi
    if grep -qE 'wlopm --(off|on)' "$SESS"; then
        echo "ok   - session blanks via wlopm"
    else
        echo "FAIL - thinfloe-session does not blank via wlopm"
        fail=1
    fi
    if grep -vE '^[[:space:]]*#' "$SESS" | grep -qE 'wlr-randr[^|]*--off'; then
        echo "FAIL - thinfloe-session blanks with 'wlr-randr --off' (disables the"
        echo "       output and reflows the layout); use wlopm instead"
        fail=1
    else
        echo "ok   - session does not disable outputs to blank"
    fi
else
    echo "FAIL - cannot extract usr/bin/thinfloe-session from $SQSH"
    fail=1
fi

# ssh.socket must stay masked: two entry points into sshd with different
# ordering is what produced the ordering cycle that stopped it starting.
if unsquashfs -no-xattrs -d "$DPKG_TMP/ssh" "$SQSH" \
        etc/systemd/system/ssh.socket >/dev/null 2>&1 &&
   [[ -L "$DPKG_TMP/ssh/etc/systemd/system/ssh.socket" ]] &&
   [[ "$(readlink "$DPKG_TMP/ssh/etc/systemd/system/ssh.socket")" == "/dev/null" ]]; then
    echo "ok   - ssh.socket is masked"
else
    echo "FAIL - ssh.socket is not masked (see CLAUDE.md on the ordering cycle)"
    fail=1
fi

# Masking alone is not enough to prove sshd can still start: Ubuntu's
# ssh.socket declares RequiredBy=ssh.service, so if that requires-symlink
# is still present, ssh.service fails outright with "Unit ssh.socket is
# masked" - a mask-only check would pass on a broken image. ssh.socket must
# be *disabled* (which removes this symlink) before it is masked.
# Anchored to etc/systemd/system deliberately. An unanchored check also
# matches var/lib/systemd/deb-systemd-helper-enabled/ssh.service.requires/
# ssh.socket, which is dpkg's own record of what it once enabled - not live
# systemd config. It survives `systemctl disable` by design and must not
# fail this test. (An earlier version of this check extracted the live path
# with `unsquashfs -d`, which is just as wrong a different way: extracting a
# single nonexistent path still exits 0, so the check always "passed"
# extraction and reported FAIL regardless of whether the file existed.)
#
# What this asserts is the thing that decides whether sshd can start at all:
# ssh.socket declares RequiredBy=ssh.service, so if enabling it left
# etc/systemd/system/ssh.service.requires/ssh.socket behind, masking the
# socket makes ssh.service fail with "Unit ssh.socket is masked" and the
# image has no sshd. Checking only that the mask exists would pass on
# exactly that broken image.
if unsquashfs -l "$SQSH" 2>/dev/null | \
        grep -qE '^squashfs-root/etc/systemd/system/ssh\.service\.requires/ssh\.socket$'; then
    echo "FAIL - etc/systemd/system/ssh.service.requires/ssh.socket survives; ssh.service cannot start"
    fail=1
else
    echo "ok   - ssh.service has no Requires on the masked ssh.socket"
fi

# sshd-keygen.service ships Wants=-enabled from BOTH ssh.socket.wants/ and
# ssh.service.wants/; disabling ssh.socket only removes the first symlink.
# Left unmasked it races thinfloe-ssh-hostkeys.service on a genuine first boot
# and can win, leaving generated keys unstaged and lost on the next reboot.
if unsquashfs -no-xattrs -d "$DPKG_TMP/keygen" "$SQSH" \
        etc/systemd/system/sshd-keygen.service >/dev/null 2>&1 &&
   [[ -L "$DPKG_TMP/keygen/etc/systemd/system/sshd-keygen.service" ]] &&
   [[ "$(readlink "$DPKG_TMP/keygen/etc/systemd/system/sshd-keygen.service")" == "/dev/null" ]]; then
    echo "ok   - sshd-keygen.service is masked"
else
    echo "FAIL - sshd-keygen.service is not masked - it can race thinfloe-ssh-hostkeys.service on first boot"
    fail=1
fi

# The folded-window strip in thinfloe-menu replaced the waybar taskbar.
# thinfloe-session must no longer start waybar, and must still kill its
# helpers (swayidle) when it ends: labwc terminates when its -S command
# returns, and a surviving helper would be orphaned.
if unsquashfs -no-xattrs -d "$DPKG_TMP/sess" "$SQSH" \
        usr/bin/thinfloe-session >/dev/null 2>&1 &&
   ! grep -q 'waybar' "$DPKG_TMP/sess/usr/bin/thinfloe-session" &&
   grep -q 'trap .*kill' "$DPKG_TMP/sess/usr/bin/thinfloe-session"; then
    echo "ok   - thinfloe-session starts no waybar and kills its helpers on exit"
else
    echo "FAIL - thinfloe-session still mentions waybar or lost its helper cleanup"
    fail=1
fi

# un-dockerize.service must not be ordered before a sysinit unit - that made
# systemd delete the hwdb update job on every boot. See CLAUDE.md.
if unsquashfs -no-xattrs -d "$DPKG_TMP/ud" "$SQSH" \
        etc/systemd/system/un-dockerize.service >/dev/null 2>&1; then
    if grep -q '^Before=systemd-hwdb-update' \
            "$DPKG_TMP/ud/etc/systemd/system/un-dockerize.service"; then
        echo "FAIL - un-dockerize.service reintroduces the hwdb ordering cycle"
        fail=1
    else
        echo "ok   - un-dockerize.service has no hwdb ordering cycle"
    fi
else
    echo "FAIL - could not extract un-dockerize.service"
    fail=1
fi

# The account is tfuser, uid and gid 1001, home /home/tfuser. 1001 is fixed:
# 3.x's tluser had it, field home archives carry it, and the restore keeps
# numeric owners. /home/tluser stays as a symlink so a 3.x path in a saved
# file still resolves; tluser is no account.
if unsquashfs -no-xattrs -d "$DPKG_TMP/acct" "$SQSH" \
        etc/passwd etc/shadow etc/group etc/subuid etc/subgid etc/sudoers \
        home/tluser >/dev/null 2>&1; then
    if grep -q '^tfuser:x:1001:1001:[^:]*:/home/tfuser:' "$DPKG_TMP/acct/etc/passwd"; then
        echo "ok   - tfuser exists with uid and gid 1001, home /home/tfuser"
    else
        echo "FAIL - tfuser is not uid 1001, gid 1001 with home /home/tfuser"
        fail=1
    fi
    if grep -q '^tfuser:x:1001:' "$DPKG_TMP/acct/etc/group"; then
        echo "ok   - group tfuser is gid 1001"
    else
        echo "FAIL - group tfuser is not gid 1001"
        fail=1
    fi
    for g in sudo audio; do
        if grep -qE "^$g:[^:]*:[0-9]+:(.*,)?tfuser(,|$)" "$DPKG_TMP/acct/etc/group"; then
            echo "ok   - tfuser is in group $g"
        else
            echo "FAIL - tfuser is not in group $g"
            fail=1
        fi
    done
    # The base image's own account (uid 1000, in group sudo) is removed.
    for f in passwd shadow group subuid subgid; do
        if grep -q '^ubuntu:' "$DPKG_TMP/acct/etc/$f"; then
            echo "FAIL - etc/$f still has the base image's ubuntu entry"
            fail=1
        else
            echo "ok   - no ubuntu entry in etc/$f"
        fi
    done
    if grep -qE '^squashfs-root/home/ubuntu$' "$LIST"; then
        echo "FAIL - /home/ubuntu is still in the image"
        fail=1
    else
        echo "ok   - no /home/ubuntu"
    fi
    if [[ -L "$DPKG_TMP/acct/home/tluser" && "$(readlink "$DPKG_TMP/acct/home/tluser")" == "tfuser" ]]; then
        echo "ok   - /home/tluser is a symlink to tfuser"
    else
        echo "FAIL - /home/tfuser is not a symlink to tfuser"
        fail=1
    fi
    if ! grep -q '^tluser:' "$DPKG_TMP/acct/etc/passwd"; then
        echo "ok   - no tluser in /etc/passwd"
    else
        echo "FAIL - tluser is still in /etc/passwd"
        fail=1
    fi
    if grep -qx 'tfuser ALL=(ALL) NOPASSWD:ALL' "$DPKG_TMP/acct/etc/sudoers"; then
        echo "ok   - sudoers grants tfuser NOPASSWD"
    else
        echo "FAIL - sudoers does not grant tfuser NOPASSWD:ALL"
        fail=1
    fi
else
    echo "FAIL - could not extract passwd, sudoers and home/tluser"
    fail=1
fi
if unsquashfs -no-xattrs -d "$DPKG_TMP/gty" "$SQSH" \
        etc/systemd/system/getty@tty1.service.d/autologin.conf >/dev/null 2>&1 &&
   grep -q -- '--autologin tfuser ' \
        "$DPKG_TMP/gty/etc/systemd/system/getty@tty1.service.d/autologin.conf"; then
    echo "ok   - getty autologin is tfuser"
else
    echo "FAIL - getty autologin is not tfuser"
    fail=1
fi

# No password ships for tfuser. adduser --disabled-password leaves `!` in the
# shadow field; an image that carries a hash there has a login every machine
# in the field shares, and tfuser has NOPASSWD:ALL, so that login is root.
# `passwd -l` is NOT a fix for this: it prefixes the existing hash with `!`
# and leaves it recoverable. The field must hold `!` and nothing else.
if unsquashfs -no-xattrs -d "$DPKG_TMP/shadow" "$SQSH" \
        etc/shadow >/dev/null 2>&1; then
    tl_field=$(sed -n 's/^tfuser:\([^:]*\):.*/\1/p' "$DPKG_TMP/shadow/etc/shadow")
    if [[ "$tl_field" == "!" ]]; then
        echo "ok   - tfuser ships with no password hash"
    else
        echo "FAIL - tfuser ships a password field of '$tl_field'; the image has a shared login"
        fail=1
    fi
    # root's field must be `!`, not empty. Empty plus the `nullok` that
    # Ubuntu's /etc/pam.d/common-auth carries is a VT login by pressing
    # Enter. sshd refuses it today only because PermitEmptyPasswords
    # defaults to no.
    root_field=$(sed -n 's/^root:\([^:]*\):.*/\1/p' "$DPKG_TMP/shadow/etc/shadow")
    if [[ "$root_field" == "!" ]]; then
        echo "ok   - root is locked, not merely password-less"
    else
        echo "FAIL - root's password field is '$root_field'; with nullok that is a VT login"
        fail=1
    fi
else
    echo "FAIL - cannot extract etc/shadow from $SQSH"
    fail=1
fi

# PermitRootLogin must be no. It is inert while root is locked and
# PermitEmptyPasswords is no, which is exactly why it is easy to leave
# wrong: the day anyone sets a root password, root is on the network.
if unsquashfs -no-xattrs -d "$DPKG_TMP/sshd" "$SQSH" \
        etc/ssh/sshd_config.d/01-thinfloe.conf >/dev/null 2>&1; then
    if grep -qE '^[[:space:]]*PermitRootLogin[[:space:]]+no[[:space:]]*$' \
            "$DPKG_TMP/sshd/etc/ssh/sshd_config.d/01-thinfloe.conf"; then
        echo "ok   - PermitRootLogin is no"
    else
        echo "FAIL - PermitRootLogin is not no: $(grep -i permitrootlogin "$DPKG_TMP/sshd/etc/ssh/sshd_config.d/01-thinfloe.conf" | tr -d '\r')"
        fail=1
    fi
    # PasswordAuthentication stays yes on purpose: it cannot succeed while no
    # password is set, and the user needs it once thinfloe-password sets one.
    if grep -qE '^[[:space:]]*PasswordAuthentication[[:space:]]+yes[[:space:]]*$' \
            "$DPKG_TMP/sshd/etc/ssh/sshd_config.d/01-thinfloe.conf"; then
        echo "ok   - PasswordAuthentication stays yes for thinfloe-password"
    else
        echo "FAIL - PasswordAuthentication is not yes; a set password could not be used"
        fail=1
    fi
else
    echo "FAIL - cannot extract etc/ssh/sshd_config.d/01-thinfloe.conf from $SQSH"
    fail=1
fi

# thinfloe-password and its unit must both ship, and the unit must be enabled.
present "thinfloe-password"  'usr/bin/thinfloe-password$'

# The -f test is not redundant: unsquashfs exits 0 for a path it did not
# find, so without it the negative ordering check below greps a file that
# does not exist, returns non-zero, and reports ok on a missing unit.
PWUNIT="$DPKG_TMP/pwunit/etc/systemd/system/thinfloe-password.service"
if unsquashfs -no-xattrs -d "$DPKG_TMP/pwunit" "$SQSH" \
        etc/systemd/system/thinfloe-password.service >/dev/null 2>&1 &&
   [[ -f "$PWUNIT" ]]; then
    if grep -qE '^Before=ssh\.service[[:space:]]*$' "$PWUNIT"; then
        echo "ok   - thinfloe-password.service is ordered before ssh.service"
    else
        echo "FAIL - thinfloe-password.service is not ordered before ssh.service"
        fail=1
    fi
    # A multi-user.target unit is implicitly After=basic.target, so a
    # Before= on anything in sysinit.target orders it both before and after
    # the same target and systemd silently deletes a job on every boot. Two
    # instances of this have shipped in this repo; see CLAUDE.md.
    if grep -qE '^Before=.*(sysinit|local-fs|systemd-hwdb|sockets|udev)' "$PWUNIT"; then
        echo "FAIL - thinfloe-password.service declares Before= on an early-boot unit: $(grep '^Before=' "$PWUNIT" | tr -d '\r')"
        fail=1
    else
        echo "ok   - thinfloe-password.service has no early-boot ordering cycle"
    fi
else
    echo "FAIL - could not extract thinfloe-password.service"
    fail=1
fi

# Greps $LIST, not a fresh `unsquashfs -l | grep -q` pipeline. With pipefail
# set, grep -q exits on the first match, unsquashfs dies of SIGPIPE, and the
# pipeline reports failure although the file is there.
if grep -qE 'etc/systemd/system/multi-user\.target\.wants/thinfloe-password\.service$' "$LIST"; then
    echo "ok   - thinfloe-password.service is enabled"
else
    echo "FAIL - thinfloe-password.service is not enabled; a set password would not survive a reboot"
    fail=1
fi

# Add-ons: thinfloe-addons.service installs the stored copies at boot, the
# timer runs the daily check. Both must be enabled, and the boot unit may be
# ordered before getty@tty1.service only: it is a multi-user.target unit, so
# a Before= on anything earlier is an ordering cycle (see CLAUDE.md).
present "thinfloe-addon"             'usr/bin/thinfloe-addon$'
present "add-on boot unit"           'etc/systemd/system/thinfloe-addons\.service$'
present "add-on check unit"          'etc/systemd/system/thinfloe-addon-check\.service$'
present "add-on check timer"         'etc/systemd/system/thinfloe-addon-check\.timer$'
present "add-on boot check timer"    'etc/systemd/system/thinfloe-addon-boot-check\.timer$'
present "add-on tmpfiles"            'usr/lib/tmpfiles\.d/thinfloe-addons\.conf$'
if grep -qE 'etc/systemd/system/multi-user\.target\.wants/thinfloe-addons\.service$' "$LIST"; then
    echo "ok   - thinfloe-addons.service is enabled"
else
    echo "FAIL - thinfloe-addons.service is not enabled; stored add-ons would not install at boot"
    fail=1
fi
if grep -qE 'etc/systemd/system/timers\.target\.wants/thinfloe-addon-check\.timer$' "$LIST"; then
    echo "ok   - thinfloe-addon-check.timer is enabled"
else
    echo "FAIL - thinfloe-addon-check.timer is not enabled; add-ons would never be checked"
    fail=1
fi
if grep -qE 'etc/systemd/system/timers\.target\.wants/thinfloe-addon-boot-check\.timer$' "$LIST"; then
    echo "ok   - thinfloe-addon-boot-check.timer is enabled"
else
    echo "FAIL - thinfloe-addon-boot-check.timer is not enabled; netboot would not install add-ons"
    fail=1
fi
# systemd applies RandomizedDelaySec to every trigger of a timer, so the
# check after boot (which installs the add-ons on netboot) has a timer of
# its own without one, and the daily timer has no OnBootSec.
TIMERS="$DPKG_TMP/addontimers/etc/systemd/system"
if unsquashfs -no-xattrs -d "$DPKG_TMP/addontimers" "$SQSH" \
        etc/systemd/system/thinfloe-addon-check.timer \
        etc/systemd/system/thinfloe-addon-boot-check.timer >/dev/null 2>&1 &&
   [[ -f "$TIMERS/thinfloe-addon-check.timer" && -f "$TIMERS/thinfloe-addon-boot-check.timer" ]]; then
    if grep -qx 'OnBootSec=1min' "$TIMERS/thinfloe-addon-boot-check.timer" &&
       grep -qx 'Unit=thinfloe-addon-check.service' "$TIMERS/thinfloe-addon-boot-check.timer" &&
       ! grep -q '^RandomizedDelaySec' "$TIMERS/thinfloe-addon-boot-check.timer" &&
       ! grep -q '^OnBootSec' "$TIMERS/thinfloe-addon-check.timer" &&
       grep -qx 'OnUnitActiveSec=1d' "$TIMERS/thinfloe-addon-check.timer"; then
        echo "ok   - the add-on check runs 1 min after boot without random delay, then daily"
    else
        echo "FAIL - the add-on timers do not split the boot check from the daily check"
        fail=1
    fi
else
    echo "FAIL - could not extract the add-on timers"
    fail=1
fi
ADDONUNIT="$DPKG_TMP/addonunit/etc/systemd/system/thinfloe-addons.service"
if unsquashfs -no-xattrs -d "$DPKG_TMP/addonunit" "$SQSH" \
        etc/systemd/system/thinfloe-addons.service >/dev/null 2>&1 &&
   [[ -f "$ADDONUNIT" ]]; then
    before=$(grep -E '^Before=' "$ADDONUNIT" | tr -d '\r')
    if [[ "$before" == "Before=getty@tty1.service" ]]; then
        echo "ok   - thinfloe-addons.service is ordered before getty@tty1.service only"
    else
        echo "FAIL - thinfloe-addons.service Before= lines: ${before:-none}"
        fail=1
    fi
    # A hung dpkg must not hold the tty1 autologin forever; thinfloe-addon
    # gives each install 25 s at boot, so the next add-on still gets its turn.
    if grep -qxE 'TimeoutStartSec=120' "$ADDONUNIT"; then
        echo "ok   - thinfloe-addons.service has TimeoutStartSec=120"
    else
        echo "FAIL - thinfloe-addons.service has no TimeoutStartSec=120"
        fail=1
    fi
else
    echo "FAIL - could not extract thinfloe-addons.service"
    fail=1
fi

# The default home must carry the 4.0 home marker. Without it a home saved
# from it lists no marker, and every boot would treat it as a 3.x home.
MARKER="$DPKG_TMP/hv/home/tfuser/.thinfloe-home-version"
if unsquashfs -no-xattrs -d "$DPKG_TMP/hv" "$SQSH" \
        home/tfuser/.thinfloe-home-version >/dev/null 2>&1 &&
   [[ -f "$MARKER" && "$(cat "$MARKER")" == "1" ]]; then
    echo "ok   - the default home carries ~/.thinfloe-home-version 1"
else
    echo "FAIL - the default home has no ~/.thinfloe-home-version holding 1"
    fail=1
fi

# --- the NUC7i3BNK field report, in the built image ---

# 1. The KMS wait script and unit must be present.
present "wait-kms script"           'usr/libexec/thinfloe/wait-kms$'
present "wait-kms unit"             'etc/systemd/system/thinfloe-wait-kms\.service$'
present "save-home exclude list"    'etc/thinfloe/save-home-exclude$'

# 2. The wait-kms unit must be enabled (WantedBy=multi-user.target).
if grep -qE 'etc/systemd/system/multi-user\.target\.wants/thinfloe-wait-kms\.service$' "$LIST"; then
    echo "ok   - wait-kms is enabled in multi-user.target"
else
    echo "FAIL - wait-kms is not enabled (missing from multi-user.target.wants)"
    fail=1
fi

# 3. The rejected thinfloe-local-network design must not return.
absent  "thinfloe-local-network unit" 'etc/systemd/system/thinfloe-local-network\.service$'
absent  "local-network installer"   'usr/libexec/thinfloe/install-local-network$'

# 4. The container ID must not be baked in; hostname is set by initramfs.
absent  "baked-in hostname"         '^squashfs-root/etc/hostname$'

# 5. The netplan config must exist and be mode 600 (see test-field-report.sh
# for the Dockerfile assertion).
if unsquashfs -no-xattrs -d "$DPKG_TMP/netplan" "$SQSH" \
        etc/netplan/ethernet.yaml >/dev/null 2>&1 &&
   [[ -f "$DPKG_TMP/netplan/etc/netplan/ethernet.yaml" ]]; then
    mode=$(stat -c%a "$DPKG_TMP/netplan/etc/netplan/ethernet.yaml")
    if [[ "$mode" == "600" ]]; then
        echo "ok   - ethernet.yaml exists at mode 600"
    else
        echo "FAIL - ethernet.yaml is mode $mode, not 600 (netplan: permissions too open)"
        fail=1
    fi
else
    echo "FAIL - cannot extract etc/netplan/ethernet.yaml from $SQSH"
    fail=1
fi

# 6. The save-home script must avoid the race where the old code would
# overwrite the home archive while it is being read: check for the atomic
# .new file pattern and absence of the legacy by-partlabel path.
if unsquashfs -no-xattrs -d "$DPKG_TMP/saveh" "$SQSH" \
        usr/bin/thinfloe-save-home >/dev/null 2>&1 &&
   [[ -f "$DPKG_TMP/saveh/usr/bin/thinfloe-save-home" ]]; then
    save_home="$DPKG_TMP/saveh/usr/bin/thinfloe-save-home"
    if grep -q 'home\.tar\.gz\.new' "$save_home"; then
        echo "ok   - thinfloe-save-home uses atomic .new file writes"
    else
        echo "FAIL - thinfloe-save-home does not use home.tar.gz.new (atomic write pattern)"
        fail=1
    fi
    if grep -q 'by-partlabel' "$save_home"; then
        echo "FAIL - thinfloe-save-home still references by-partlabel (legacy path)"
        fail=1
    else
        echo "ok   - thinfloe-save-home does not reference by-partlabel"
    fi
else
    echo "FAIL - cannot extract usr/bin/thinfloe-save-home from $SQSH"
    fail=1
fi

# 7. The autologin drop-in must order getty after wait-kms so the race
# window cannot open.
if unsquashfs -no-xattrs -d "$DPKG_TMP/autologin" "$SQSH" \
        etc/systemd/system/getty@tty1.service.d/autologin.conf >/dev/null 2>&1 &&
   [[ -f "$DPKG_TMP/autologin/etc/systemd/system/getty@tty1.service.d/autologin.conf" ]]; then
    if grep -q 'thinfloe-wait-kms' "$DPKG_TMP/autologin/etc/systemd/system/getty@tty1.service.d/autologin.conf"; then
        echo "ok   - autologin drop-in pulls in wait-kms"
    else
        echo "FAIL - autologin drop-in does not reference thinfloe-wait-kms"
        fail=1
    fi
else
    echo "FAIL - cannot extract getty@tty1 autologin drop-in from $SQSH"
    fail=1
fi

# 8. thinfloe-upgrade: the save before finishing and the home copy onto a fresh
# drive. A stale image would keep the old 60 s save whose bare except hid a
# failed save, and would copy no home at all.
if unsquashfs -no-xattrs -d "$DPKG_TMP/upg" "$SQSH" usr/bin/thinfloe-upgrade >/dev/null 2>&1 &&
   [[ -f "$DPKG_TMP/upg/usr/bin/thinfloe-upgrade" ]]; then
    UPG="$DPKG_TMP/upg/usr/bin/thinfloe-upgrade"
    if grep -q 'def save_home_before_finish' "$UPG" && grep -q 'SAVE_HOME_TIMEOUT = 600' "$UPG"; then
        echo "ok   - thinfloe-upgrade saves the home with a 600 s bound and reports failure"
    else
        echo "FAIL - thinfloe-upgrade has no save_home_before_finish with SAVE_HOME_TIMEOUT = 600"
        fail=1
    fi
    if grep -qE 'timeout=60[,)]' "$UPG"; then
        echo "FAIL - thinfloe-upgrade still calls thinfloe-save-home with timeout=60 (kills a netboot upload)"
        fail=1
    else
        echo "ok   - thinfloe-upgrade no longer uses the 60 s save timeout"
    fi
    if grep -q 'def copy_home_to_drive' "$UPG"; then
        echo "ok   - thinfloe-upgrade can copy the home onto a fresh drive"
    else
        echo "FAIL - thinfloe-upgrade has no copy_home_to_drive"
        fail=1
    fi
    # Both identity files must never travel to another machine.
    if grep -q '"./.thinfloe-ssh-host-keys"' "$UPG"; then
        echo "ok   - home copy excludes the SSH host keystore"
    else
        echo "FAIL - home copy does not exclude ./.thinfloe-ssh-host-keys (two machines would share a host key)"
        fail=1
    fi
    if grep -q '"./wg0.conf"' "$UPG"; then
        echo "ok   - home copy excludes wg0.conf"
    else
        echo "FAIL - home copy does not exclude ./wg0.conf (two machines would share a WireGuard identity)"
        fail=1
    fi
else
    echo "FAIL - cannot extract usr/bin/thinfloe-upgrade from $SQSH"
    fail=1
fi
# thinfloe-menu rasterises on the CPU. The images this is built for have no
# Vulkan driver at all, and a GL path would make the menu's start depend on
# EGL context creation. A later switch to a GPU backend must fail here, not
# on a client. Both linking and dlopen are checked: winit and wgpu load
# their libraries at runtime.
MENU_TMP=$(mktemp -d)
if unsquashfs -no-xattrs -d "$MENU_TMP/x" "$SQSH" usr/bin/thinfloe-menu >/dev/null 2>&1 &&
   [[ -f "$MENU_TMP/x/usr/bin/thinfloe-menu" ]]; then
    MB="$MENU_TMP/x/usr/bin/thinfloe-menu"
    if readelf -d "$MB" | grep NEEDED | grep -qiE 'vulkan|libGL|libEGL|GLES'; then
        echo "FAIL - thinfloe-menu links a GPU library"
        fail=1
    elif grep -aqE 'libvulkan\.so|libEGL\.so|libGLESv2\.so|libGL\.so' "$MB"; then
        echo "FAIL - thinfloe-menu names a GPU library it may dlopen"
        fail=1
    else
        echo "ok   - thinfloe-menu uses no GPU library"
    fi
else
    echo "FAIL - cannot extract usr/bin/thinfloe-menu from $SQSH"
    fail=1
fi
chmod -R u+rwX "$MENU_TMP" 2>/dev/null
rm -rf "$MENU_TMP"

# The session body is the menu now; tlclient is one of its tiles.
if [[ -n "${SESS:-}" && -f "$SESS" ]] &&
   grep -q 'TF_MENU:-/usr/bin/thinfloe-menu' "$SESS" &&
   ! grep -v '^[[:space:]]*#' "$SESS" | grep -q '/opt/thinlinc/bin/tlclient'; then
    echo "ok   - thinfloe-session launches thinfloe-menu, not tlclient"
else
    echo "FAIL - thinfloe-session does not launch thinfloe-menu"
    fail=1
fi

# Cargo.lock is committed, so the image builds what was tested, and it
# carries no smithay-clipboard, which segfaults on Wayland.
LOCK=src/thinfloe-menu/Cargo.lock
if git ls-files --error-unmatch "$LOCK" >/dev/null 2>&1 &&
   ! grep -q '^name = "smithay-clipboard"$' "$LOCK"; then
    echo "ok   - $LOCK is committed and has no smithay-clipboard"
else
    echo "FAIL - $LOCK missing from git or pulls smithay-clipboard"
    fail=1
fi

# Every shipped tile is usable and its icon draws, checked by the menu
# itself: --check resolves each Icon= and renders it in the same rlimited,
# time-limited renderer process the session uses, so an icon that is missing
# or that the renderer refuses or cannot finish fails here, not as a letter
# on the screen. An add-on tile's icon is skipped while the add-on is not
# installed, which it never is in the image: test-addon-deps.sh checks
# ThinLinc's thinlinc_128.png after installing the client.
IMAGE="${TF_UBUNTU_IMAGE:-localhost/thinfloe-ubuntu:3.0.0}"
if check_out=$(podman run --rm --network=none "$IMAGE" /usr/bin/thinfloe-menu --check 2>&1); then
    echo "ok   - every shipped tile is usable and its icon renders"
else
    echo "FAIL - thinfloe-menu --check in $IMAGE:"
    echo "$check_out"
    fail=1
fi

if [[ $fail -ne 0 ]]; then
    echo ""
    echo "FAILED - session stack is not as expected"
    exit 1
fi

echo ""
echo "PASSED - Wayland session stack correct"
exit 0
