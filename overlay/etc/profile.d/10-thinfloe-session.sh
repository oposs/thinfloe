# Start the Wayland session after autologin on the first VT.
#
# labwc -S runs the given command on startup and terminates when it exits,
# so the compositor's lifetime is exactly the session's lifetime.
#
# -C points labwc at a config directory outside $HOME. $HOME is captured
# wholesale into home.tar.gz by save-home and restored wholesale by
# restore_home() (in the initramfs) on every boot, with no excludes. A
# config file kept under $HOME would therefore be pinned forever on an
# already-deployed machine - including the rc.xml that enforces the
# zero-keybindings constraint - and a corrected file shipped in a future
# image would never reach it.
# TF_SESSION_ATTEMPTED guards against recursion: the failure path below
# leaves a login shell on tty1, and a login shell re-sources this file. The
# marker is exported, so the guard survives into that shell and stops it
# starting the session again in a loop.
if [ -z "${WAYLAND_DISPLAY:-}" ] &&
   [ -z "${TF_SESSION_ATTEMPTED:-}" ] &&
   [ "$(tty)" = "/dev/tty1" ]; then

    export TF_SESSION_ATTEMPTED=1

    # The log MUST live somewhere tfuser can write. /run is root-owned 0755,
    # so redirecting there fails - and a failed redirection means the command
    # is never executed at all. An earlier version of this hook pointed at
    # /run and so prevented the session from starting, while presenting as
    # "labwc exited with status 1". Starting labwc by hand worked, because
    # there was no redirection involved. Do not move this back to /run.
    TF_SESSION_LOG="${XDG_RUNTIME_DIR:-/tmp}/thinfloe-session.log"
    if ! : >>"$TF_SESSION_LOG" 2>/dev/null; then
        TF_SESSION_LOG=/tmp/thinfloe-session.log
    fi

    # thinfloe-session leaves thinfloe-menu's exit status in TF_SESSION_STATUS.
    # labwc always exits 0 whatever its -S command returned, so without this
    # file a failing menu restarts in a loop with nothing on screen.
    # session-verdict counts consecutive failures in TF_SESSION_FAILURES,
    # which must outlive this login; XDG_RUNTIME_DIR does not, /tmp is per
    # boot. thinfloe-session reads both exported names.
    TF_SESSION_STATUS="${XDG_RUNTIME_DIR:-/tmp}/thinfloe-session.status"
    TF_SESSION_FAILURES="${TF_SESSION_FAILURES:-/tmp/thinfloe-session-failures.$(id -u)}"
    TF_SESSION_VERDICT="${TF_SESSION_VERDICT:-/usr/libexec/thinfloe/session-verdict}"
    export TF_SESSION_LOG TF_SESSION_STATUS

    # Put the session environment into OUR environment before launching, so
    # the variables are present in labwc's process environment at exec time.
    #
    # labwc does read this file itself via -C, but relying on that leaves the
    # XKB settings dependent on whether labwc applies the file before or after
    # it initialises the keyboard - and in practice the ctrl:nocaps and compose
    # options were not taking effect. Exporting them here removes the ordering
    # question entirely.
    #
    # It also means a manual `labwc -C ... ...` from the fallback console
    # below inherits the same settings, so hand-debugging matches what the
    # automatic start does. (The failure message further down prints the
    # exact -C argument that was actually used, merged config dir or plain
    # system one - see below.)
    if [ -r /etc/thinfloe/labwc/environment ]; then
        set -a
        . /etc/thinfloe/labwc/environment
        set +a
    fi

    # The home directory is ALREADY restored - the initramfs did it, in
    # restore_home() (thinfloe-lib.sh), before the pivot.
    #
    # Do not add a restore call here or in thinfloe-session. The constraint that
    # forced it out of thinfloe-session still stands: ~/.thinfloe-environment must
    # be on disk before labwc reads XKB_DEFAULT_* at startup, or a user's
    # keyboard change takes effect only on the NEXT boot. The initramfs
    # satisfies that; a restore at this point would merely re-satisfy it,
    # and a restore any later would break it.

    # User overrides, sourced AFTER the system defaults so the user wins.
    # This file is for variables the compositor reads at STARTUP - keyboard
    # layout, cursor theme. Commands that need a running compositor
    # (wlr-randr, kanshi) belong in ~/.thinfloe-sessionrc instead, which runs
    # inside the session.
    #
    # It lives in $HOME, so it is saved by save-home and restored above on
    # every subsequent boot: a user can set the keyboard layout for their own
    # machine without rebuilding the image.
    if [ -r "$HOME/.thinfloe-environment" ]; then
        set -a
        . "$HOME/.thinfloe-environment"
        set +a
    fi

    # The export above puts the user's values in OUR environment, which is
    # enough for thinfloe-session and tlclient - but not for labwc itself.
    # Run with -C, labwc re-reads <config_dir>/environment on its own
    # (session_environment_init(), src/config/session.c) and setenv()s
    # every key it finds there with overwrite=1, ignoring what the shell
    # already exported. Pointing -C straight at /etc/thinfloe/labwc therefore
    # let the system file's value win back over the user's for any key
    # present in both - LABWC_FULLSCREEN_SPAN_OUTPUTS and the XKB_DEFAULT_*/
    # XCURSOR_* keys included. See CLAUDE.md's ".thinfloe-environment" section
    # for the full writeup.
    #
    # thinfloe-compose-labwc-config builds a config directory whose merged
    # environment file has the system defaults first and the user's
    # overrides last, so the file labwc itself parses already has the
    # user's values winning - see that script's header for why later wins.
    # This relies on the home directory already being restored: it is, by
    # the initramfs (restore_home() in thinfloe-lib.sh), before this script
    # ever runs, so ~/.thinfloe-environment already exists here.
    #
    # A broken merge must degrade to today's behaviour (the plain system
    # config, still correct for every machine that has no per-machine
    # override) rather than to no session at all, so any failure here -
    # XDG_RUNTIME_DIR unset/unwritable, or the helper itself failing -
    # falls back to /etc/thinfloe/labwc and just logs it.
    LABWC_CONFIG_DIR=/etc/thinfloe/labwc
    if [ -n "${XDG_RUNTIME_DIR:-}" ] && [ -w "${XDG_RUNTIME_DIR:-}" ]; then
        if /usr/bin/thinfloe-compose-labwc-config \
                /etc/thinfloe/labwc "$HOME/.thinfloe-environment" \
                "$XDG_RUNTIME_DIR/thinfloe-labwc" >>"$TF_SESSION_LOG" 2>&1; then
            LABWC_CONFIG_DIR="$XDG_RUNTIME_DIR/thinfloe-labwc"
        else
            echo "thinfloe: config merge failed, falling back to /etc/thinfloe/labwc" \
                >>"$TF_SESSION_LOG"
        fi
    else
        echo "thinfloe: XDG_RUNTIME_DIR unset or unwritable, falling back to /etc/thinfloe/labwc" \
            >>"$TF_SESSION_LOG"
    fi

    # The compositor's stderr is the only record of why a session failed.
    # Without this redirection it lands on tty1 and is erased when getty
    # restarts the session seconds later.
    tf_run_labwc() {
        rm -f "$TF_SESSION_STATUS"
        labwc -C "$LABWC_CONFIG_DIR" -S /usr/bin/thinfloe-session \
            >>"$TF_SESSION_LOG" 2>&1
    }
    # The boot picture stays on screen until the menu draws (2026-10-27).
    # Plymouth is still running: tty1 runs login -f without agetty, which
    # would quit it, and plymouth-quit.service is masked. Plymouth holds DRM
    # master, so it lets go of the display (deactivate) before labwc starts,
    # and thinfloe-session ends it once labwc is up. The quit after labwc
    # returns covers a labwc that never ran the session; on a later login
    # Plymouth is gone and every call is a no-op. The helper needs root;
    # sudo -n because a prompt here would hang the boot.
    TF_PLYMOUTH_STEP="${TF_PLYMOUTH_STEP:-/usr/libexec/thinfloe/plymouth-step}"
    tf_boot_screen() {
        sudo -n "$TF_PLYMOUTH_STEP" "$@" >/dev/null 2>&1 || true
    }
    tf_boot_screen status "Starting the session"
    tf_boot_screen deactivate
    tf_run_labwc
    TF_SESSION_RC=$?

    # Retry once if labwc itself could not start.
    #
    # labwc always exits 0 whatever its -S command returned, so a non-zero
    # status here means labwc could not come up at all - almost always no
    # usable DRM device. thinfloe-wait-kms.service covers the known i915 coldplug
    # race before login; this covers what it cannot: a late hotplug, a GPU
    # reset, or hardware slower than its 20s budget.
    #
    # Exactly once, guarded by an exported variable, because a loop on a
    # machine with no GPU would spin forever and fill the journal. The retry
    # runs inline: re-sourcing this file would be a no-op, since
    # TF_SESSION_ATTEMPTED is already set. Before the fix that added this,
    # a 273ms timing miss on a NUC7i3BNK cost the entire boot: the failure
    # shell below is held, never exited, and nothing retried.
    if [ "$TF_SESSION_RC" -ne 0 ] && [ -z "${TF_SESSION_RETRIED:-}" ]; then
        echo "thinfloe: the compositor did not start (status $TF_SESSION_RC)."
        echo "thinfloe: waiting for a graphics driver and trying once more..."
        # The boot picture covers the wait. Plymouth must still be running
        # here, so no quit before the retry: it would leave nothing to show.
        tf_boot_screen reactivate
        tf_boot_screen status "Waiting for the graphics driver"
        # Overridable so the tests do not wait on a host that has the file.
        "${TF_WAIT_KMS:-/usr/libexec/thinfloe/wait-kms}" || true
        TF_SESSION_RETRIED=1
        export TF_SESSION_RETRIED
        tf_boot_screen deactivate
        tf_run_labwc
        TF_SESSION_RC=$?
    fi
    # Before the failure screen, which is text on tty1 and must be seen.
    tf_boot_screen quit

    if [ "$TF_SESSION_RC" -eq 0 ]; then
        # labwc came up and went away again. Whether the session inside it
        # ended cleanly is in the status file, not in labwc's status. A
        # logout, or a menu failure below the limit, exits so getty starts
        # a fresh session. A missing helper keeps the old behaviour.
        if [ ! -x "$TF_SESSION_VERDICT" ] ||
           TF_MENU_RC=$("$TF_SESSION_VERDICT" \
                "$TF_SESSION_STATUS" "$TF_SESSION_FAILURES"); then
            exit 0
        fi
        echo "thinfloe: the menu failed several sessions in a row."
        TF_SESSION_RC=$TF_MENU_RC
    fi

    # Failure. Deliberately do NOT exit - getty would restart us and the
    # error would scroll past unread. Hold this shell on tty1 with the log
    # in view so the failure is diagnosable at the machine itself.
    #
    # This is INTENTIONAL PRODUCTION BEHAVIOUR, kept on purpose - do not
    # remove it as leftover debugging. The trade was made knowingly: once the
    # single retry above has also failed, the session shows the operator what
    # went wrong instead of retrying invisibly forever, at the cost of leaving
    # a shell prompt on screen rather than continuing to attempt the client.
    #
    # Consequence worth knowing: because this holds the shell, getty does not
    # restart on failure at all, so the StartLimitIntervalSec/RestartSec
    # settings in getty@tty1.service are belt-and-braces rather than the
    # thing preventing a dead tty1. They still matter if this block is ever
    # changed to exit.
    echo ""
    echo "=============================================================="
    echo " thinfloe: graphical session exited with status $TF_SESSION_RC"
    echo ""
    echo " Last lines of $TF_SESSION_LOG:"
    echo "--------------------------------------------------------------"
    tail -n 25 "$TF_SESSION_LOG" 2>/dev/null
    echo "--------------------------------------------------------------"
    echo " Full log:    $TF_SESSION_LOG"
    echo " Journal:     journalctl -b --no-pager"
    echo " DRM devices: ls -l /dev/dri/"
    echo " Seat/session: loginctl session-status"
    echo " Retry byhand: labwc -C $LABWC_CONFIG_DIR -S /usr/bin/thinfloe-session"
    echo "=============================================================="
    echo ""
fi
