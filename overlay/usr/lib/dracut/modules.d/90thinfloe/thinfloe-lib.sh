#!/bin/bash
# thinfloe-lib.sh - Shared library functions

# Guard against multiple sourcing - only define functions and readonly vars once
if [ -z "$_TF_LIB_LOADED" ]; then
_TF_LIB_LOADED=1

type info >/dev/null 2>&1 || . /lib/dracut-lib.sh

# Use info instead of dinfo for compatibility
type dinfo >/dev/null 2>&1 || dinfo() { info "$@"; }

# Also echo to /dev/console for serial output
# Use >&2 to avoid polluting stdout (which is used for return values).
# That includes info: under systemd, dracut's info() echoes to stdout, and a
# log line in tf_find_esp's answer once made finalize-upgrade.sh fail to
# mount the ESP on every boot.
tf_log() {
    info "$@" >&2
    echo "$@" > /dev/console 2>/dev/null || true
    echo "$@" >&2
}

readonly TF_BASE="/run/thinfloe"
readonly TF_STORAGE="$TF_BASE/storage"
readonly TF_LAYERS="$TF_BASE/layers"
readonly TF_STATE="$TF_BASE/state"

# Set once a wait has already exhausted its budget in this boot, so a later
# hook fails fast instead of repeating the whole timeout.
readonly TF_EFI_ABSENT_MARKER="${TF_EFI_ABSENT_MARKER:-/run/thinfloe-efi-absent}"

# tf_esp_candidates
#
# Print the kernel names (vda1, sdb1) of all partitions named EFI-SYSTEM,
# sorted, one per line. Every ThinFloe stick carries that name, so with two
# sticks attached there are two, and /dev/disk/by-partlabel/EFI-SYSTEM is a
# single udev link that points at either one, differently from boot to
# boot. That link is therefore never used to choose.
tf_esp_candidates() {
    for _tec_dir in "${TF_SYS_BLOCK:-/sys/class/block}"/*; do
        [ -r "$_tec_dir/uevent" ] || continue
        while read -r _tec_line; do
            if [ "$_tec_line" = "PARTNAME=EFI-SYSTEM" ]; then
                echo "${_tec_dir##*/}"
                break
            fi
        done <"$_tec_dir/uevent"
    done
}

# tf_read_stick_id <dev>
#
# Print the stick id in config/stick-id on /dev/<dev>, or nothing when the
# partition has none or it is not valid. Mounts read-only and unmounts again.
tf_read_stick_id() {
    _trsi_mnt=/run/thinfloe-probe
    mkdir -p "$_trsi_mnt" 2>/dev/null || return 0
    mount -t vfat -o ro "/dev/$1" "$_trsi_mnt" 2>/dev/null || return 0
    _trsi_id=""
    [ -r "$_trsi_mnt/config/stick-id" ] && \
        read -r _trsi_id <"$_trsi_mnt/config/stick-id"
    umount "$_trsi_mnt" 2>/dev/null || true
    tf_valid_stick_id "$_trsi_id" && echo "$_trsi_id"
    return 0
}

# _tf_esp_pairs <known>
#
# Print "<dev>=<id>" for every current candidate, reading the id only for a
# device not already listed in <known> (the previous output) with an id, so
# a poll does not mount every stick again each second; a pair whose id is
# empty is read again, since the first read may have failed.
_tf_esp_pairs() {
    for _tep_dev in $(tf_esp_candidates); do
        _tep_pair=""
        for _tep_known in $1; do
            [ "${_tep_known%%=*}" = "$_tep_dev" ] && [ -n "${_tep_known#*=}" ] \
                && _tep_pair="$_tep_known"
        done
        [ -n "$_tep_pair" ] || _tep_pair="$_tep_dev=$(tf_read_stick_id "$_tep_dev")"
        echo "$_tep_pair"
    done
}

# _tf_esp_settled <want-id> <pairs...>
#
# Exit 0 when waiting longer cannot improve the choice: a partition carries
# the wanted id, or none is wanted and there is one partition only or one
# has no id.
_tf_esp_settled() {
    _tes_want="$1"
    shift
    [ -z "$_tes_want" ] && [ $# -eq 1 ] && return 0
    for _tes_pair in "$@"; do
        if [ -n "$_tes_want" ]; then
            [ "${_tes_pair#*=}" = "$_tes_want" ] && return 0
        else
            [ -z "${_tes_pair#*=}" ] && return 0
        fi
    done
    return 1
}

# tf_find_esp
#
# Print /dev/<dev> of the ESP of the stick this machine booted from: the
# EFI-SYSTEM partition whose config/stick-id the kernel command line names
# (thinfloe.stick=), chosen by tf_choose_esp. Exit 1 when no EFI-SYSTEM
# partition appears within TF_DEVICE_TIMEOUT. The answer is cached in
# TF_ESP_CACHE, so finalize-upgrade.sh and mount-squashfs.sh get the same.
#
# This polls rather than checking once. `udevadm settle` only drains events
# that are ALREADY queued: if the USB host controller has not yet enumerated
# the stick, that queue is empty and settle returns immediately. The previous
# "settle; sleep 2; test once; give up" sequence therefore granted the device
# barely two seconds. A USB 3 stick on real hardware routinely needs longer,
# while QEMU's virtio-blk appears at once - which is why this failed only on
# hardware and never in the VM.
#
# The bootloader having loaded the kernel from this same partition proves
# nothing about the kernel's view: UEFI firmware used its own USB stack, and
# the kernel re-enumerates the bus from scratch. For the same reason a second
# stick can appear later than the first, so after the first one appears this
# waits up to TF_ESP_MATCH_WAIT more while the wanted one is missing.
tf_find_esp() {
    _tfe_cache="${TF_ESP_CACHE:-/run/thinfloe-esp-device}"
    _tfe_timeout="${TF_DEVICE_TIMEOUT:-60}"
    _tfe_match_wait="${TF_ESP_MATCH_WAIT:-10}"

    if [ -s "$_tfe_cache" ]; then
        read -r _tfe_dev <"$_tfe_cache"
        echo "$_tfe_dev"
        return 0
    fi
    [ -e "$TF_EFI_ABSENT_MARKER" ] && return 1

    _tfe_waited=0
    while [ -z "$(tf_esp_candidates)" ]; do
        if [ "$_tfe_waited" -ge "$_tfe_timeout" ]; then
            : > "$TF_EFI_ABSENT_MARKER" 2>/dev/null || true
            return 1
        fi
        if [ $((_tfe_waited % 5)) -eq 0 ]; then
            tf_log "Waiting for EFI-SYSTEM to appear (${_tfe_waited}/${_tfe_timeout}s)"
        fi
        udevadm settle --timeout=5 >/dev/null 2>&1 || true
        sleep 1
        _tfe_waited=$((_tfe_waited + 1))
    done
    [ "$_tfe_waited" -gt 0 ] && tf_log "EFI-SYSTEM appeared after ${_tfe_waited}s"

    _tfe_want="$(getarg thinfloe.stick=)"
    tf_valid_stick_id "$_tfe_want" || _tfe_want=""

    _tfe_pairs="$(_tf_esp_pairs "")"
    _tfe_waited=0
    # shellcheck disable=SC2086
    while ! _tf_esp_settled "$_tfe_want" $_tfe_pairs \
          && [ "$_tfe_waited" -lt "$_tfe_match_wait" ]; do
        udevadm settle --timeout=5 >/dev/null 2>&1 || true
        sleep 1
        _tfe_waited=$((_tfe_waited + 1))
        _tfe_pairs="$(_tf_esp_pairs "$_tfe_pairs")"
    done

    # shellcheck disable=SC2086
    _tfe_dev="$(tf_choose_esp "$_tfe_want" $_tfe_pairs)" || return 1
    _tfe_id="none"
    for _tfe_pair in $_tfe_pairs; do
        [ "${_tfe_pair%%=*}" = "$_tfe_dev" ] && [ -n "${_tfe_pair#*=}" ] && \
            _tfe_id="${_tfe_pair#*=}"
    done
    tf_log "thinfloe: boot stick /dev/$_tfe_dev (stick id $_tfe_id)"
    echo "/dev/$_tfe_dev" >"$_tfe_cache" 2>/dev/null || true
    echo "/dev/$_tfe_dev"
    return 0
}

# Log what block devices the kernel can actually see. Called before giving up,
# so the operator sees evidence instead of only "not found".
tf_dump_block_devices() {
    tf_log "Block devices visible to the kernel:"
    blkid 2>&1 | while read -r _ddbd_line; do tf_log "  $_ddbd_line"; done
    tf_log "Partition labels present:"
    ls -l /dev/disk/by-partlabel/ 2>&1 | while read -r _ddbd_line; do tf_log "  $_ddbd_line"; done
}

# tf_valid_stick_id <string>
#
# Exit 0 when the string is a stick id: a lowercase UUID as
# /proc/sys/kernel/random/uuid writes it. The value goes into syslinux.cfg
# and a sed expression, so nothing else may pass.
tf_valid_stick_id() {
    _tvsi_h='[0-9a-f]'
    _tvsi_4="$_tvsi_h$_tvsi_h$_tvsi_h$_tvsi_h"
    # shellcheck disable=SC2254
    case "$1" in
        $_tvsi_4$_tvsi_4-$_tvsi_4-$_tvsi_4-$_tvsi_4-$_tvsi_4$_tvsi_4$_tvsi_4) return 0 ;;
    esac
    return 1
}

# tf_choose_esp <want-id> <dev>=<id> ...
#
# Pick the boot stick's ESP among the partitions named EFI-SYSTEM, given in
# device-name order, each with its stick id (empty for none). Prints the
# chosen <dev>. Exit 1 when there is no partition.
#
#   1. Exactly one partition carries the wanted id: that one.
#   2. Several carry it (a dd copy of a used stick): the first, logged.
#   3. Otherwise one partition is that one; else the partitions without an
#      id, because a stick with an id always boots with it on the command
#      line, so a boot without a matching one came from a stick that never
#      booted; else the first, logged.
tf_choose_esp() {
    _tce_want="$1"
    shift
    [ $# -gt 0 ] || return 1
    tf_valid_stick_id "$_tce_want" || _tce_want=""

    _tce_first=""; _tce_total=0
    _tce_match=""; _tce_nmatch=0
    _tce_noid=""; _tce_nnoid=0
    for _tce_arg in "$@"; do
        _tce_dev="${_tce_arg%%=*}"
        _tce_id="${_tce_arg#*=}"
        [ -n "$_tce_first" ] || _tce_first="$_tce_dev"
        _tce_total=$((_tce_total + 1))
        if [ -n "$_tce_want" ] && [ "$_tce_id" = "$_tce_want" ]; then
            [ -n "$_tce_match" ] || _tce_match="$_tce_dev"
            _tce_nmatch=$((_tce_nmatch + 1))
        fi
        if [ -z "$_tce_id" ]; then
            [ -n "$_tce_noid" ] || _tce_noid="$_tce_dev"
            _tce_nnoid=$((_tce_nnoid + 1))
        fi
    done

    if [ "$_tce_nmatch" -gt 0 ]; then
        [ "$_tce_nmatch" -gt 1 ] && \
            tf_log "thinfloe: $_tce_nmatch sticks carry stick id $_tce_want, using /dev/$_tce_match"
        echo "$_tce_match"
    elif [ "$_tce_total" -eq 1 ]; then
        echo "$_tce_first"
    elif [ "$_tce_nnoid" -gt 0 ]; then
        [ "$_tce_nnoid" -gt 1 ] && \
            tf_log "thinfloe: $_tce_nnoid sticks have no stick id, using /dev/$_tce_noid"
        echo "$_tce_noid"
    else
        if [ -n "$_tce_want" ]; then
            tf_log "thinfloe: no stick carries stick id $_tce_want, using /dev/$_tce_first"
        else
            tf_log "thinfloe: no stick id on the command line, using /dev/$_tce_first"
        fi
        echo "$_tce_first"
    fi
    return 0
}

tf_init_dirs() {
    dinfo "thinfloe: Creating $TF_BASE"
    mkdir -p "$TF_BASE" || die "Failed to create $TF_BASE"

    dinfo "thinfloe: Creating storage directories"
    mkdir -p "$TF_STORAGE"/efi || die "Failed to create $TF_STORAGE/efi"
    mkdir -p "$TF_STORAGE"/download || die "Failed to create $TF_STORAGE/download"

    dinfo "thinfloe: Creating layer directories"
    mkdir -p "$TF_LAYERS"/lower || die "Failed to create $TF_LAYERS/lower"
    mkdir -p "$TF_LAYERS"/upper || die "Failed to create $TF_LAYERS/upper"

    dinfo "thinfloe: Creating state directory"
    mkdir -p "$TF_STATE" || die "Failed to create state dir"

    info "thinfloe: Directory structure created"
    dinfo "thinfloe:   Base: $TF_BASE"
    dinfo "thinfloe:   EFI mount: $TF_STORAGE/efi"
}

is_remote_url() {
    case "$1" in
        http://*|https://*|ftp://*)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

# tf_home_url <ramroot-url> <mac>
#
# Build the URL the home archive lives at on the boot server, from the same
# ramroot URL the squashfs came from: .../tl/ramroot.sqsh -> .../tl/home.pkg
#
# thinfloe-save-home derives the identical base with a greedy sed over
# /proc/cmdline. Both reduce to "the directory the ramroot URL sits in", and
# they must not be allowed to diverge - a mismatch means the client posts its
# home somewhere it will never look for it again.
tf_home_url() {
    _dhu_ramroot="$1"
    _dhu_mac="$2"

    [ -n "$_dhu_ramroot" ] || return 1
    [ -n "$_dhu_mac" ] || return 1

    echo "${_dhu_ramroot%/*}/home.pkg?mac=${_dhu_mac}"
}

# tf_record_boot_mac <address-file> <state-dir>
#
# Persist the MAC of the interface the initramfs actually brought up, so
# thinfloe-save-home posts the home archive back under the same key the restore
# fetched it with.
#
# Before this, both halves derived the MAC independently from kernel ifindex
# 2 - which agreed only because both ran in the booted system. ifindex
# follows driver registration order, which the initramfs need not share; on
# a machine with two NICs the two could name different devices.
# mount-squashfs.sh passes the interface holding the default route (see
# tf_route_iface). Recording what was actually used makes the two halves agree by
# construction. It is also the better identity: on netboot it is the
# interface that demonstrably worked, having taken a DHCP lease and served
# ramroot.sqsh.
tf_record_boot_mac() {
    _drbm_addr_file="$1"
    _drbm_state_dir="$2"

    if [ ! -r "$_drbm_addr_file" ]; then
        warn "thinfloe: cannot read MAC from $_drbm_addr_file"
        return 1
    fi

    _drbm_mac=$(cat "$_drbm_addr_file" 2>/dev/null | tr -d '\012')
    if [ -z "$_drbm_mac" ]; then
        warn "thinfloe: empty MAC in $_drbm_addr_file"
        return 1
    fi

    mkdir -p "$_drbm_state_dir" 2>/dev/null || true
    if ! echo "$_drbm_mac" > "$_drbm_state_dir/boot-mac" 2>/dev/null; then
        warn "thinfloe: could not write $_drbm_state_dir/boot-mac"
        return 1
    fi

    return 0
}

# tf_route_iface
#
# Read `ip -o route show default` output on stdin and print the device of the
# first default route, which is the one the kernel uses (the list is sorted by
# metric). Prints nothing and returns 1 if there is none.
#
# On netboot this names the interface whose DHCP lease systemd-networkd
# applied, which is what the boot MAC must come from - not the first entry
# of /sys/class/net, which on a machine with two NICs can be the one without
# a cable.
tf_route_iface() {
    local _dri_line _dri_prev _dri_word
    while read -r _dri_line; do
        _dri_prev=""
        for _dri_word in $_dri_line; do
            if [ "$_dri_prev" = dev ]; then
                printf '%s\n' "$_dri_word"
                return 0
            fi
            _dri_prev=$_dri_word
        done
    done
    return 1
}

# tf_restore_home_from_file <archive> <target-home>
#
# Extract a home archive over the target home directory.
#
# Return codes distinguish "nothing to restore" from "something was there
# and it did not restore" - restore_home records this distinction (see
# below) so thinfloe-save-home can refuse to save over a home it never
# actually restored:
#   0 = restored
#   1 = nothing to restore - a missing archive. Normal: first boot, or a
#       machine whose home was never saved. Must leave the shipped default
#       home in place rather than aborting the boot.
#   2 = there WAS something and it did not restore - unusable tar, failed
#       extraction, or the target directory could not even be created.
#   3 = extracted, but the listing failed, so a 3.x home could not be
#       migrated (treated like 2 by restore_home).
tf_restore_home_from_file() {
    _drhf_archive="$1"
    _drhf_home="$2"
    _drhf_renamed="${3:-}"

    if [ ! -f "$_drhf_archive" ]; then
        warn "thinfloe: no home archive at $_drhf_archive"
        return 1
    fi

    if ! mkdir -p "$_drhf_home" 2>/dev/null; then
        warn "thinfloe: cannot create $_drhf_home"
        return 2
    fi

    # A payload that is valid gzip but not a tar decompresses cleanly, lists
    # zero members, and makes GNU tar exit 0 - measured, not assumed. Trusting
    # that exit code would report a successful restore having restored
    # nothing, and thinfloe-ssh-hostkeys would then treat the machine as keyless
    # and generate fresh host keys into that empty home. (It does NOT put the
    # user's good archive at risk on its own - restore_home's "failed" marker
    # is what stops thinfloe-save-home from uploading over it; see restore_home
    # below.) A boot server answering 200 with a gzip-encoded error page is
    # enough, and curl -f does not catch a 200. So require the archive to
    # contain at least one member before trusting it.
    #
    # The listing is kept for tf_migrate_home_names: listing the archive
    # there a second time decompressed it a third time on every boot.
    _drhf_list=$(tar -tzf "$_drhf_archive" 2>/dev/null)
    _drhf_list_rc=$?
    if [ -z "$_drhf_list" ]; then
        warn "thinfloe: $_drhf_archive is not a usable tar archive"
        return 2
    fi

    # tar's default as root is --same-owner, and that is REQUIRED here, not
    # incidental. thinfloe-save-home writes the archive as tfuser, so members
    # carry uid/gid 1001 numerically; preserving them is what makes the
    # restored home belong to tfuser. Do NOT add --no-same-owner.
    #
    # Note this is the exact OPPOSITE of what thinfloe-ssh-hostkeys must do a
    # few files away: host keys come out of this same archive tfuser-owned
    # and have to be forced to root:root before sshd will touch them. Two
    # different problems; do not "make them consistent".
    if ! tar -xzf "$_drhf_archive" -C "$_drhf_home" 2>/dev/null; then
        warn "thinfloe: home archive $_drhf_archive did not extract cleanly"
        return 2
    fi

    # An archive saved by 3.x carries ~/.dbrrg-* names; give them their
    # 4.0 names now, before anything reads them. Never fails the restore.
    # If the listing failed (tar exited non-zero, so it may be incomplete),
    # the migration cannot be decided. The home then holds 3.x names beside
    # image defaults. Return 3: the caller counts it as a failed restore
    # (saving is refused, the archive survives), and no marker is written,
    # so the next boot tries again.
    if [ "$_drhf_list_rc" -ne 0 ] \
       || ! tf_migrate_home_names "$_drhf_home" "$_drhf_list" "$_drhf_renamed"; then
        warn "thinfloe: listing of $_drhf_archive failed, home names not migrated"
        return 3
    fi

    return 0
}

# tf_restore_home_from_url <url> <target-home> [timeout] [tmp-file]
#
# Fetch a home archive from the boot server and extract it. Downloads to a
# temporary file first: piping curl straight into tar would extract a
# half-written archive over the home directory if the transfer died midway.
#
# Same return-code contract as tf_restore_home_from_file (0 restored,
# 1 nothing to restore, 2 something was there and it did not restore) -
# see that function. A 404 (curl -f exit 22) is the ordinary first-netboot
# case and maps to 1; every other curl failure, including a timeout, means
# the server had something and it could not be fetched, so that maps to 2.
tf_restore_home_from_url() {
    _drhu_url="$1"
    _drhu_home="$2"
    _drhu_timeout="${3:-120}"
    _drhu_tmp="${4:-/tmp/thinfloe-home.pkg}"
    _drhu_renamed="${5:-}"

    rm -f "$_drhu_tmp" 2>/dev/null || true

    # -f makes an HTTP 404 a failure rather than a saved error page. A 404
    # is the ordinary first-netboot case for a machine the server has not
    # seen before, so it is warned about and not treated as a fault.
    #
    # --connect-timeout and --max-time are mandatory. The login-time script
    # this replaces waited on `while true; do ping ...; done`; unbounded at
    # login is a session recoverable from a VT, but unbounded here is a
    # machine that never finishes booting.
    #
    # The exit status is captured explicitly into _drhu_curl_rc rather than
    # tested with `if ! curl ...`, which would discard it - and the
    # distinction matters: exit 22 (404) is normal, anything else (a
    # timeout, exit 28, included) means the server had something and it
    # could not be retrieved.
    curl -f -s -S \
            --connect-timeout 10 --max-time "$_drhu_timeout" \
            -o "$_drhu_tmp" "$_drhu_url" 2>/dev/null
    _drhu_curl_rc=$?

    if [ "$_drhu_curl_rc" -ne 0 ]; then
        rm -f "$_drhu_tmp" 2>/dev/null || true
        if [ "$_drhu_curl_rc" -eq 22 ]; then
            warn "thinfloe: no home archive at $_drhu_url"
            return 1
        fi
        warn "thinfloe: could not fetch home archive from $_drhu_url (curl exit $_drhu_curl_rc)"
        return 2
    fi

    tf_restore_home_from_file "$_drhu_tmp" "$_drhu_home" "$_drhu_renamed"
    _drhu_rc=$?
    rm -f "$_drhu_tmp" 2>/dev/null || true
    return $_drhu_rc
}

# Home files that 3.x called ~/.dbrrg-* and 4.0 calls ~/.thinfloe-*.
# One "old:new" pair per line, paths relative to the home.
TF_HOME_RENAMES=".dbrrg-environment:.thinfloe-environment
.dbrrg-sessionrc:.thinfloe-sessionrc
.dbrrg-password:.thinfloe-password
.dbrrg-ssh-host-keys:.thinfloe-ssh-host-keys
.config/dbrrg:.config/thinfloe"

# The marker of a home that has 4.0 names. Every default home ships it
# (overlay/home/tfuser), and the migration writes it, so every home saved by
# 4.0 lists it. An archive that does not is a 3.x home, migrated once.
TF_HOME_VERSION_FILE=".thinfloe-home-version"
TF_HOME_VERSION=1

# _tf_archive_lists <listing> <name>
#
# True when the tar listing holds <name>: exactly as ./name, name, ./name/ or
# name/, or as a member below it (./name/x, name/x). The last case matters
# for a directory that a hand-made archive lists only through its members,
# without an entry of its own. Plain string comparison with awk's index(); no
# regular expression, so a dot in a name matches only a dot. The name goes
# in through the environment, because awk -v would interpret backslashes.
_tf_archive_lists() {
    printf '%s\n' "$1" | _TF_NAME="$2" awk '
        BEGIN { n = ENVIRON["_TF_NAME"]; a = "./" n; b = n }
        $0 == a || $0 == b || index($0, a "/") == 1 || index($0, b "/") == 1 { f = 1 }
        END { exit !f }'
}

# _tf_parent_is_link <home> <name>
#
# True, with a warning, when <name> sits below a directory of the home that
# is a symlink (today only ~/.config, for .config/dbrrg). Renaming, removing
# or linking below it would act on wherever that link points, possibly
# outside the home: rm -rf of a real ~/.config/dbrrg would delete the target's
# directory. Such an entry is skipped.
#
# The rename and the symlink removal both ask, so the warning is given once
# per directory and restore: _TF_LINK_WARNED remembers the path warned about,
# and restore_home empties it. Without that every boot of such a machine
# logged the same line twice.
_TF_LINK_WARNED=""
_tf_parent_is_link() {
    case "$2" in
        */*) ;;
        *) return 1 ;;
    esac
    [ -L "$1/${2%/*}" ] || return 1
    if [ "$_TF_LINK_WARNED" != "$1/${2%/*}" ]; then
        warn "thinfloe: ~/${2%/*} is a symlink, leaving ~/$2 alone"
        _TF_LINK_WARNED="$1/${2%/*}"
    fi
    return 0
}

# _tf_move_over <from> <to>
#
# Move <from> to <to>, replacing a file or a whole directory there. <to> is
# moved aside first and only removed once the move worked; on failure it is
# moved back, so neither name is lost. With both gone, thinfloe-ssh-hostkeys
# would find no keystore and make new keys.
_tf_move_over() {
    # A leftover from an interrupted boot would travel in every save; for the
    # host keys it holds private keys outside IDENTITY_EXCLUDES.
    rm -rf "$2.tf-aside" 2>/dev/null || true
    if [ -e "$2" ] || [ -L "$2" ]; then
        mv "$2" "$2.tf-aside" 2>/dev/null || return 1
        if ! mv "$1" "$2" 2>/dev/null; then
            mv "$2.tf-aside" "$2" 2>/dev/null || true
            return 1
        fi
        rm -rf "$2.tf-aside" 2>/dev/null || true
        return 0
    fi
    mv "$1" "$2" 2>/dev/null
}

# tf_migrate_home_names <home> <listing> [<renamed-file>]
#
# Give the files of a 3.x home archive their 4.0 names, once, after the
# archive has been extracted over the image's default home.
#
# The rule is decided by what is IN THE ARCHIVE, not by what is on disk. The
# default home already ships the marker, .thinfloe-environment and
# .thinfloe-sessionrc, so their presence on disk says nothing.
#   - The archive lists TF_HOME_VERSION_FILE: the home is native and nothing
#     is touched. A .dbrrg-* entry in it was put there by a save on a 3.x
#     fallback boot and means nothing to 4.0.
#   - Otherwise, per name pair: the archive lists the new name, and it stays;
#     else the archive lists the old name and a real (not symlink) old entry
#     is on disk, and it replaces whatever image default sits at the new
#     name. Each name renamed is appended to <renamed-file>, one per line.
#     Then an old name that is a symlink is removed: development builds of
#     4.0 left those. A real old entry beside a listed new name (also only
#     from a development build) is left alone. Last, the marker is written.
# Running it again on the same archive changes nothing, so a marker that
# could not be written only means the next boot migrates again.
# <listing> is the archive's `tar -tzf` output, which the caller already made
# to check the archive; listing it again here would decompress it once more.
# The tools are those the initramfs has: no cut, head, basename, readlink.
#
# Returns 0, except 1 when the listing is empty (the caller must then treat
# the restore as failed). A failed single rename is only warned about.
tf_migrate_home_names() {
    _tmh_home="$1"
    _tmh_list="$2"
    _tmh_renamed="${3:-}"

    [ -n "$_tmh_list" ] || return 1
    _tf_archive_lists "$_tmh_list" "$TF_HOME_VERSION_FILE" && return 0

    # Newline-separated pairs, split with parameter expansion: the shell
    # may be dash, which has no here-strings, and a pipe would run the loop
    # in a subshell.
    _tmh_rest="$TF_HOME_RENAMES"
    while [ -n "$_tmh_rest" ]; do
        case "$_tmh_rest" in
            *'
'*) _tmh_line="${_tmh_rest%%'
'*}"; _tmh_rest="${_tmh_rest#*'
'}" ;;
            *)  _tmh_line="$_tmh_rest"; _tmh_rest="" ;;
        esac
        _tmh_old="${_tmh_line%%:*}"
        _tmh_new="${_tmh_line#*:}"

        _tf_parent_is_link "$_tmh_home" "$_tmh_old" && continue
        if ! _tf_archive_lists "$_tmh_list" "$_tmh_new" \
           && _tf_archive_lists "$_tmh_list" "$_tmh_old" \
           && [ -e "$_tmh_home/$_tmh_old" ] && [ ! -L "$_tmh_home/$_tmh_old" ]; then
            if _tf_move_over "$_tmh_home/$_tmh_old" "$_tmh_home/$_tmh_new"; then
                tf_log "thinfloe: renamed ~/$_tmh_old to ~/$_tmh_new"
                if [ -n "$_tmh_renamed" ]; then
                    echo "$_tmh_old" >> "$_tmh_renamed" 2>/dev/null || true
                fi
            else
                warn "thinfloe: could not rename ~/$_tmh_old to ~/$_tmh_new"
            fi
        fi
        if [ -L "$_tmh_home/$_tmh_old" ]; then
            rm -f "$_tmh_home/$_tmh_old" 2>/dev/null || \
                warn "thinfloe: could not remove the link ~/$_tmh_old"
        fi
    done

    # Into the existing file when there is one, so it keeps the owner the
    # image gave it (tfuser); the initramfs has no chown.
    # 2>/dev/null first: it must be in place before the redirection that fails.
    if ! echo "$TF_HOME_VERSION" 2>/dev/null > "$_tmh_home/$TF_HOME_VERSION_FILE"; then
        warn "thinfloe: could not write ~/$TF_HOME_VERSION_FILE, the next boot migrates again"
    fi

    return 0
}

# restore_home <newroot> <efi-mount> <state-dir> <ramroot>
#
# Populate /home/tfuser in the new root before the pivot.
#
# This used to run at login, from /etc/profile.d/10-thinfloe-session.sh. It runs
# here now so the home directory exists before multi-user.target, which is
# what lets the SSH host keys persist inside it - see
# overlay/usr/bin/thinfloe-ssh-hostkeys.
#
# Moving it EARLIER does not violate the standing rule recorded in
# 10-thinfloe-session.sh. That rule forbids moving the restore back INTO
# thinfloe-session, because ~/.thinfloe-environment must be on disk before labwc
# reads XKB_DEFAULT_* at startup. The initramfs is earlier still, so the
# ordering constraint holds a fortiori.
#
# ALWAYS returns 0. It is called from setup-overlay.sh, where a non-zero
# return aborts the boot. A client that cannot restore its home must still
# come up with the shipped default one.
#
# Because it can never fail the boot, it cannot signal a bad restore through
# its return value - so it writes the outcome to <state-dir>/home-restore
# instead, one word:
#   ok     - restored successfully
#   absent - nothing to restore (first boot, or a home never saved); saving
#            at logout is correct
#   failed - something was there and did not restore (corrupt/truncated
#            archive, fetch failure, or no boot MAC recorded on netboot)
# overlay/usr/bin/thinfloe-save-home reads this marker and refuses to save when
# it says "failed", so a boot that could not retrieve the real home does not
# get uploaded over it, taking the user's data and the machine's persisted
# SSH host keys (staged by thinfloe-ssh-hostkeys into this same default home)
# down with it.
restore_home() {
    _rh_newroot="$1"
    _rh_efi="$2"
    _rh_state="$3"
    _rh_ramroot="$4"

    _rh_home="$_rh_newroot/home/tfuser"
    _TF_LINK_WARNED=""

    if is_remote_url "$_rh_ramroot"; then
        _rh_mac=$(cat "$_rh_state/boot-mac" 2>/dev/null | tr -d '\012')
        if [ -z "$_rh_mac" ]; then
            # Deliberately do not fall back to guessing an interface. A
            # wrong MAC means fetching one machine's home onto another, or
            # saving to a key nothing will ever read back. Unlike a normal
            # "nothing to restore", this is a fault in the boot itself, so
            # it is "failed" rather than "absent" - thinfloe-save-home must not
            # save over whatever this default home ends up holding.
            warn "thinfloe: no boot MAC recorded, skipping home restore"
            echo failed > "$_rh_state/home-restore" 2>/dev/null || true
            return 0
        fi

        _rh_url=$(tf_home_url "$_rh_ramroot" "$_rh_mac")
        if [ -z "$_rh_url" ]; then
            warn "thinfloe: could not build home archive URL, skipping home restore"
            echo failed > "$_rh_state/home-restore" 2>/dev/null || true
            return 0
        fi

        # Record the base the URL was built from, for the same reason the MAC
        # is recorded: so thinfloe-save-home posts to the identical location
        # rather than re-deriving it.
        #
        # save-home derives the base with a greedy sed over the whole
        # /proc/cmdline (s/.*ramroot=\(.*\)\/.*$/\1/). That backtracks to the
        # LAST slash anywhere in the command line, so any kernel parameter
        # appearing after ramroot= that contains a "/" - root=/dev/sda1,
        # init=/sbin/init - silently yields a wrong base. Today's
        # configs/syslinux.cfg happens to have no such parameter, so the bug
        # is latent rather than live; recording the resolved value here
        # removes the whole class instead of relying on that staying true.
        echo "${_rh_ramroot%/*}" > "$_rh_state/boot-home-base" 2>/dev/null || \
            warn "thinfloe: could not record boot-home-base"

        tf_log "thinfloe: restoring home from $_rh_url"
        tf_restore_home_from_url "$_rh_url" "$_rh_home" 120 "" "$_rh_state/home-renamed"
        _rh_rc=$?
    else
        tf_log "thinfloe: restoring home from $_rh_efi/home.tar.gz"
        tf_restore_home_from_file "$_rh_efi/home.tar.gz" "$_rh_home" "$_rh_state/home-renamed"
        _rh_rc=$?
    fi

    case "$_rh_rc" in
        0) echo ok > "$_rh_state/home-restore" 2>/dev/null || true ;;
        1) echo absent > "$_rh_state/home-restore" 2>/dev/null || true ;;
        *) echo failed > "$_rh_state/home-restore" 2>/dev/null || true ;;
    esac

    return 0
}

# install_local_network <newroot>
#
# Copy the per-machine ~/wifi.yaml and ~/wg0.conf out of the restored home
# into the new root's /etc, and enable wg-quick@wg0. /etc is a fresh RAM
# overlay every boot, so nothing else puts them there.
#
# This is in the initramfs, not in the session and not in a unit, because the
# netplan systemd generator emits netplan-wpa-<iface>.service when systemd
# starts in the real root, before any unit runs. A unit that installed
# wifi.yaml would only produce .network files and no WPA unit, so WiFi would
# never associate; a runtime "systemctl enable" would not start wg-quick on
# that boot either. Doing it here also keeps the network independent of the
# graphical session, which is how a machine with a broken session stays
# reachable.
#
# The two halves are independent: a missing wifi.yaml must not stop the VPN.
# Symlinks are refused because in the initramfs an absolute link target
# resolves against the initramfs root, not the new root.
#
# ALWAYS returns 0. It is called from setup-overlay.sh, where a non-zero
# return aborts the boot.
install_local_network() {
    local _iln_root="$1"
    local _iln_home="$_iln_root/home/tfuser"
    local _iln_wants="$_iln_root/etc/systemd/system/multi-user.target.wants"

    if [ -f "$_iln_home/wifi.yaml" ] && [ ! -L "$_iln_home/wifi.yaml" ] && \
       grep -q -F 'device name of your wifi card' "$_iln_home/wifi.yaml"; then
        # The image ships this placeholder; installing it would only make
        # netplan complain about an interface that does not exist.
        tf_log "thinfloe: ~/wifi.yaml is the unedited template, skipping"
    elif [ -f "$_iln_home/wifi.yaml" ] && [ ! -L "$_iln_home/wifi.yaml" ]; then
        tf_log "thinfloe: installing wifi.yaml into /etc/netplan"
        # netplan ignores a file others can read, hence 600.
        if mkdir -p "$_iln_root/etc/netplan" && \
           cp "$_iln_home/wifi.yaml" "$_iln_root/etc/netplan/wifi.yaml" && \
           chmod 600 "$_iln_root/etc/netplan/wifi.yaml"; then
            :
        else
            warn "thinfloe: could not install wifi.yaml"
        fi
    else
        tf_log "thinfloe: no usable ~/wifi.yaml, skipping"
    fi

    if [ -f "$_iln_home/wg0.conf" ] && [ ! -L "$_iln_home/wg0.conf" ]; then
        tf_log "thinfloe: installing wg0.conf into /etc/wireguard"
        if mkdir -p "$_iln_root/etc/wireguard" && \
           chmod 700 "$_iln_root/etc/wireguard" && \
           cp "$_iln_home/wg0.conf" "$_iln_root/etc/wireguard/wg0.conf" && \
           chmod 600 "$_iln_root/etc/wireguard/wg0.conf" && \
           mkdir -p "$_iln_wants" && \
           ln -sf /usr/lib/systemd/system/wg-quick@.service \
               "$_iln_wants/wg-quick@wg0.service"; then
            :
        else
            warn "thinfloe: could not install wg0.conf"
        fi
    else
        tf_log "thinfloe: no usable ~/wg0.conf, skipping"
    fi

    return 0
}

verify_squashfs() {
    local sqsh_path="$1"

    [ -f "$sqsh_path" ] || die "SquashFS not found: $sqsh_path"

    local size=$(stat -c%s "$sqsh_path" 2>/dev/null)
    [ "$size" -gt 52428800 ] || die "SquashFS too small: $size bytes"

    local magic=$(dd if="$sqsh_path" bs=4 count=1 status=none 2>/dev/null | od -An -tx1 | tr -d ' \012')
    [ "$magic" = "68737173" ] || die "Invalid squashfs magic: $magic"

    info "SquashFS verified: $size bytes"
}

create_zram_overlay() {
    local size="${1:-2G}"

    # Log to kmsg/console only (NOT info() which pollutes stdout with systemd)
    echo "<30>dracut: Creating ZRAM device (size: $size)" > /dev/kmsg
    echo "Creating ZRAM device (size: $size)" > /dev/console 2>/dev/null

    modprobe zram 2>/dev/null || die "Failed to load zram module"
    [ -d /sys/class/zram-control ] || die "ZRAM not available"

    local zram_id=$(cat /sys/class/zram-control/hot_add 2>/dev/null)
    [ -n "$zram_id" ] || die "Failed to create zram device"

    local zram_dev="zram${zram_id}"
    local zram_path="/dev/$zram_dev"

    echo zstd > /sys/block/$zram_dev/comp_algorithm 2>/dev/null || true
    echo 4 > /sys/block/$zram_dev/max_comp_streams 2>/dev/null || true
    echo "$size" > /sys/block/$zram_dev/disksize || die "Failed to set ZRAM size"

    mkfs.ext4 -q -O ^has_journal,^metadata_csum,^ext_attr -m 0 -b 4096 "$zram_path" || \
        die "Failed to format ZRAM"

    echo "<30>dracut: ZRAM device ready: $zram_path" > /dev/kmsg
    echo "ZRAM device ready: $zram_path" > /dev/console 2>/dev/null

    # Return ONLY the device path on stdout
    echo "$zram_path"
}

get_machine_id() {
    local efi_mount="$TF_STORAGE/efi"
    local machine_id=""

    if mountpoint -q "$efi_mount" 2>/dev/null; then
        local id_file="$efi_mount/config/machine-id"
        if [ -f "$id_file" ]; then
            machine_id=$(cat "$id_file" 2>/dev/null | tr -d '\012')
            if [ -n "$machine_id" ]; then
                echo "<30>dracut: Loaded machine-id from EFI" > /dev/kmsg
                echo "$machine_id"
                return 0
            fi
        fi
    fi

    machine_id=$(cat /proc/sys/kernel/random/uuid 2>/dev/null | tr -d '-' | tr -d '\012')
    [ -n "$machine_id" ] || die "Failed to generate machine-id"

    echo "<30>dracut: Generated new machine-id" > /dev/kmsg

    if mountpoint -q "$efi_mount" 2>/dev/null; then
        mkdir -p "$efi_mount/config" 2>/dev/null
        echo "$machine_id" > "$efi_mount/config/machine-id" 2>/dev/null || true
    fi

    echo "$machine_id"
}

setup_loop_device() {
    local sqsh_path="$1"
    local readahead="${2:-128}"

    modprobe loop 2>/dev/null || true

    # Log to kmsg and console (NOT using info() which pollutes stdout with systemd)
    echo "<30>dracut: Setting up loop device for $sqsh_path" > /dev/kmsg
    echo "Setting up loop device for $sqsh_path" > /dev/console 2>/dev/null

    local loop_dev=$(losetup --find --show --read-only "$sqsh_path" 2>/dev/null)
    if [ -z "$loop_dev" ]; then
        die "losetup failed to create loop device"
    fi

    echo "<30>dracut: losetup returned: $loop_dev" > /dev/kmsg
    echo "losetup returned: $loop_dev" > /dev/console 2>/dev/null

    # Wait for udev to create the device node
    udevadm settle --timeout=5 2>/dev/null || true

    # Additional wait with retries
    local retries=30
    while [ $retries -gt 0 ]; do
        [ -b "$loop_dev" ] && break
        sleep 0.2
        retries=$((retries - 1))
    done

    [ -b "$loop_dev" ] || die "Loop device not ready: $loop_dev"

    # Set read-ahead (optional, non-fatal)
    command -v blockdev >/dev/null 2>&1 && blockdev --setra "$readahead" "$loop_dev" 2>/dev/null

    echo "<30>dracut: Loop device ready: $loop_dev" > /dev/kmsg
    echo "Loop device ready: $loop_dev" > /dev/console 2>/dev/null

    # Return ONLY the device path on stdout
    echo "$loop_dev"
}

fi  # end of _TF_LIB_LOADED guard

# tf_write_hostname <newroot> <machine_id> <state_dir>
#
# Write thinfloe-<six hex characters> to <newroot>/etc/hostname.
#
# Netboot: <state_dir>/boot-mac holds the MAC of the interface that fetched
# ramroot.sqsh. There is no ESP, so the machine-id is new every boot, while
# the MAC is stable and is already the identity the home archive is keyed on.
# The name is the LAST six hex digits of it; the first six are the vendor OUI,
# identical across a fleet of one hardware model.
# USB boot: no boot-mac is recorded; the first six characters of the
# machine-id (persisted on the ESP) are used.
#
# Only shell builtins, tr and plymouth (guarded) are used: the initramfs ships no cut, basename,
# head or install, and a missing tool here once produced the hostname "thinfloe-".
#
# Never fails the boot: an unusable MAC falls back to the machine-id, an
# unusable machine-id or a failed write is warned about and skipped, and the
# function always returns 0.
tf_write_hostname() {
    local _wh_root="$1"
    local _wh_id="$2"
    local _wh_state="${3:-}"
    local _wh_mac="" _wh_suffix=""

    if [ -n "$_wh_state" ] && [ -r "$_wh_state/boot-mac" ]; then
        read -r _wh_mac < "$_wh_state/boot-mac" 2>/dev/null || true
        if [ -n "$_wh_mac" ]; then
            _wh_mac=$(printf '%s' "$_wh_mac" | tr -d ':' | tr 'A-Z' 'a-z')
            if [ "${#_wh_mac}" -ge 6 ]; then
                _wh_suffix=${_wh_mac#"${_wh_mac%??????}"}
            fi
            case "$_wh_suffix" in
                ''|*[!0-9a-f]*)
                    warn "thinfloe: unusable boot MAC, using the machine-id for the hostname"
                    _wh_suffix="" ;;
            esac
        fi
    fi

    if [ -z "$_wh_suffix" ]; then
        if [ "${#_wh_id}" -ge 6 ]; then
            _wh_suffix=${_wh_id%"${_wh_id#??????}"}
        fi
        case "$_wh_suffix" in
            ''|*[!0-9a-f]*)
                warn "thinfloe: no usable MAC or machine-id for a hostname, skipping"
                return 0 ;;
        esac
    fi

    { printf 'thinfloe-%s\n' "$_wh_suffix" > "$_wh_root/etc/hostname"; } \
        2>/dev/null || warn "thinfloe: could not write /etc/hostname"

    # Show the name at the foot of the boot screen. Only when the plymouth
    # daemon answers (not with plymouth.enable=0, not on a rescue boot), and
    # whatever happens the boot goes on.
    if command -v plymouth >/dev/null 2>&1 && plymouth --ping 2>/dev/null; then
        plymouth display-message --text="thinfloe-$_wh_suffix" 2>/dev/null || true
    fi

    return 0
}

# tf_set_boot_default <efi-mount> <label>
#
# Set the DEFAULT line of syslinux.cfg on the mounted ESP, in both copies the
# image carries: /syslinux.cfg (BIOS) and /efi/boot/syslinux.cfg (EFI). A
# missing copy is skipped. Each copy is written to a .tmp file and renamed
# over the original, so a power cut leaves the old or the new file, never half
# of one. thinfloe-upgrade sets "new" after it stages tl.new; the rotation sets
# "current" again.
tf_set_boot_default() {
    local _sbd_efi="$1" _sbd_label="$2" _sbd_cfg _sbd_rc=0

    for _sbd_cfg in "$_sbd_efi/syslinux.cfg" "$_sbd_efi/efi/boot/syslinux.cfg"; do
        [ -f "$_sbd_cfg" ] || continue
        if sed "s/^DEFAULT .*/DEFAULT $_sbd_label/" "$_sbd_cfg" > "$_sbd_cfg.tmp" \
                && mv "$_sbd_cfg.tmp" "$_sbd_cfg"; then
            :
        else
            tf_log "finalize-upgrade: could not set DEFAULT $_sbd_label in $_sbd_cfg"
            rm -f "$_sbd_cfg.tmp"
            _sbd_rc=1
        fi
    done
    sync
    return $_sbd_rc
}

# tf_stamp_stick <efi-mount> <want-id>
#
# Give the mounted stick its id, so the next boot finds this ESP by it (see
# tf_find_esp). The id is the one in config/stick-id, else <want-id> (the
# command line's thinfloe.stick=) when this is the only EFI-SYSTEM partition
# (with two, this stick may not be the one that booted, and would take its
# id), else a new one from TF_UUID_SOURCE.
# config/stick-id is written when it differs. Each syslinux.cfg copy is
# rewritten only when one of its local APPEND lines (ramroot= not a URL)
# lacks the id at its end: any old thinfloe.stick= word goes, the new one is
# appended, before a trailing CR if the file has CRLF line ends.
# thinfloe-upgrade's stamp_syslinux makes the same edit and
# test-esp-choice.sh compares the two.
#
# Always returns 0: a stick that cannot be stamped boots as before, and a
# failed write leaves the old file in place.
tf_stamp_stick() {
    local _tss_efi="$1" _tss_want="$2" _tss_id="" _tss_new="" _tss_cfg
    local _tss_tab _tss_cr _tss_ws _tss_local _tss_need _tss_dev _tss_n=0

    [ -r "$_tss_efi/config/stick-id" ] && read -r _tss_id <"$_tss_efi/config/stick-id"
    tf_valid_stick_id "$_tss_id" || _tss_id=""
    if [ -z "$_tss_id" ]; then
        for _tss_dev in $(tf_esp_candidates); do _tss_n=$((_tss_n + 1)); done
        if [ "$_tss_n" -le 1 ] && tf_valid_stick_id "$_tss_want"; then
            _tss_id="$_tss_want"
        else
            read -r _tss_id <"${TF_UUID_SOURCE:-/proc/sys/kernel/random/uuid}"
            tf_valid_stick_id "$_tss_id" || {
                tf_log "thinfloe: no usable stick id from ${TF_UUID_SOURCE:-/proc/sys/kernel/random/uuid}"
                return 0
            }
            _tss_new=1
        fi
        if ! { mkdir -p "$_tss_efi/config" \
               && echo "$_tss_id" >"$_tss_efi/config/stick-id.tmp" \
               && sync \
               && mv "$_tss_efi/config/stick-id.tmp" "$_tss_efi/config/stick-id"; } 2>/dev/null; then
            tf_log "thinfloe: could not write config/stick-id, the stick keeps no id"
            rm -f "$_tss_efi/config/stick-id.tmp" 2>/dev/null
            return 0
        fi
        [ -n "$_tss_new" ] && tf_log "thinfloe: this stick is now $_tss_id"
    fi

    # Literal tab and CR: GNU sed reads \t in a bracket expression, but the
    # Python twin in thinfloe-upgrade must match this byte for byte, so
    # nothing is left to escape handling.
    _tss_tab=$(printf '\t')
    _tss_cr=$(printf '\r')
    _tss_ws="[ $_tss_tab]"
    # sed address: an APPEND line whose ramroot= is not a URL (no
    # <scheme>://; an alternation here reads as a command call to
    # test-initramfs-commands.sh's extractor).
    _tss_local="/^$_tss_ws*[Aa][Pp][Pp][Ee][Nn][Dd]$_tss_ws/{/${_tss_ws}ramroot=[a-z]+:\\/\\//b"

    for _tss_cfg in "$_tss_efi/syslinux.cfg" "$_tss_efi/efi/boot/syslinux.cfg"; do
        [ -f "$_tss_cfg" ] || continue
        _tss_need=$(sed -nE "$_tss_local;/${_tss_ws}thinfloe\\.stick=$_tss_id$_tss_cr?\$/b;p}" "$_tss_cfg")
        [ -n "$_tss_need" ] || continue
        if sed -E "$_tss_local;s/$_tss_ws+thinfloe\\.stick=[^ $_tss_tab$_tss_cr]*//g;s/$_tss_ws*($_tss_cr?)\$/ thinfloe.stick=$_tss_id\\1/}" \
                "$_tss_cfg" >"$_tss_cfg.tmp" 2>/dev/null \
                && sync \
                && mv "$_tss_cfg.tmp" "$_tss_cfg" 2>/dev/null; then
            :
        else
            tf_log "thinfloe: could not write thinfloe.stick=$_tss_id into $_tss_cfg"
            rm -f "$_tss_cfg.tmp" 2>/dev/null
        fi
    done
    sync
    return 0
}

# tf_finalize_upgrade <efi-mount> <ramroot> <ramroot-file>
#
# Rotate a pending upgrade on the mounted ESP: tl.old is removed, tl becomes
# tl.old, tl.new becomes tl. Called by finalize-upgrade.sh, which owns the
# mount and unmount.
#
# The rotation runs only when <ramroot> is tl.new/ramroot.sqsh, i.e. when the
# boot loader took this boot's kernel and initramfs from tl.new/ ("LABEL new",
# which thinfloe-upgrade makes the DEFAULT). The squashfs cannot be moved once it
# is mounted, so the rotation has to happen here, before the mount; and only a
# kernel loaded from tl.new/ matches the modules in tl.new/ramroot.sqsh. A
# rotation on a boot from tl/ ran the old kernel on the new squashfs, and every
# module not in the old initramfs failed to load (an i226-V NIC stayed down
# until the next reboot). A tl.new found on a boot from tl/ is left alone.
#
# On success syslinux.cfg is set back to DEFAULT current, before the moves:
# a power cut between them then boots tl/ or fails the same way the rotation
# always could, but never points syslinux at a tl.new/ that is already gone.
# <ramroot-file> (/tmp/thinfloe-ramroot) is rewritten to tl/ramroot.sqsh, so the
# mount hook finds the squashfs where the rotation put it.
#
# Returns 0 when nothing was pending, the upgrade was finalized, or the boot
# should continue regardless (a failed tl.new -> tl move is logged and left
# for mount-squashfs to report). Returns 1 when tl.old or tl could not be
# moved out of the way, which finalize-upgrade.sh has always passed on as its
# exit status.
#
# The syncs are why module-setup.sh installs sync: they order the FAT
# directory updates on the stick, so a power cut between two moves leaves a
# bootable tl/ or tl.old/. The initramfs had no sync until 2026-10, so every
# one of these calls failed with "command not found" while the offline tests
# passed on the dev host.
tf_finalize_upgrade() {
    local _dfu_efi="$1" _dfu_ramroot="$2" _dfu_ramroot_file="$3"
    local _dfu_file

    # Check if upgrade is pending (tl.new exists with required files)
    [ -d "$_dfu_efi/tl.new" ] || return 0

    if [ "$_dfu_ramroot" != "tl.new/ramroot.sqsh" ]; then
        tf_log "finalize-upgrade: tl.new is pending, but this kernel came from ${_dfu_ramroot%/*}/ - not rotating"
        return 0
    fi

    for _dfu_file in vmlinuz initrd.img ramroot.sqsh; do
        if [ ! -f "$_dfu_efi/tl.new/$_dfu_file" ]; then
            tf_log "finalize-upgrade: tl.new/$_dfu_file missing - aborting (keeping tl.new for next attempt)"
            return 0
        fi
    done

    info "finalize-upgrade: Finalizing pending upgrade (tl.new -> tl -> tl.old)"
    tf_log "finalize-upgrade: Starting rotation"

    tf_set_boot_default "$_dfu_efi" current

    if [ -d "$_dfu_efi/tl.old" ]; then
        info "finalize-upgrade: Removing old fallback (tl.old)"
        if ! rm -rf "$_dfu_efi/tl.old"; then
            tf_log "finalize-upgrade: Failed to remove tl.old"
            return 1
        fi
    fi

    if [ -d "$_dfu_efi/tl" ]; then
        info "finalize-upgrade: Moving current to fallback (tl -> tl.old)"
        if ! mv "$_dfu_efi/tl" "$_dfu_efi/tl.old"; then
            tf_log "finalize-upgrade: Failed to move tl to tl.old"
            return 1
        fi
        # Sync after critical move to ensure FAT32 directory entries are written
        sync
    fi

    info "finalize-upgrade: Activating new version (tl.new -> tl)"
    if ! mv "$_dfu_efi/tl.new" "$_dfu_efi/tl"; then
        tf_log "finalize-upgrade: CRITICAL - Failed to move tl.new to tl!"
        # Try to restore - this is critical for bootability
        if [ -d "$_dfu_efi/tl.old" ]; then
            if mv "$_dfu_efi/tl.old" "$_dfu_efi/tl"; then
                tf_log "finalize-upgrade: Recovered by restoring tl.old -> tl"
                sync
            else
                tf_log "finalize-upgrade: FATAL - Recovery failed! System may be unbootable!"
                tf_log "finalize-upgrade: tl.new exists but couldn't be moved, tl.old couldn't be restored"
            fi
        else
            tf_log "finalize-upgrade: FATAL - No tl.old to restore! System may be unbootable!"
        fi
        # Don't fail - let boot continue and possibly fail at mount-squashfs.
        # This gives the user a chance to see the error messages.
        return 0
    fi

    sync
    echo "tl/ramroot.sqsh" > "$_dfu_ramroot_file"

    info "finalize-upgrade: Upgrade finalized successfully"
    tf_log "finalize-upgrade: Complete - now booting from new tl/"
    return 0
}
