# Deskflow: per-device Compose key and pointer re-entry position

Date: 2026-11-22. Owner approved both approaches (1A, 2A) and the design on 2026-11-22.

## Intent

Two small improvements to ThinFloe's Deskflow server:

1. **Per-device Compose key:** macOS and Windows Deskflow clients ignore `kKeyCompose` (`0xEF20`), so the ThinFloe Compose key does nothing when typing onto them. Resolving `Multi_key` sequences locally on ThinFloe before sending the composed character's `KeyID` makes Compose work on those clients for every character their active keyboard layout can produce (including Option, AltGr and dead-key characters such as `ä`, `ö`, `ü`, `é`, `ß`, `€`, `—`, `…`), while keeping raw forwarding available for Linux clients that handle Compose themselves.
2. **Pointer re-entry position:** moving the pointer back from a client currently re-enters ThinFloe at the position where it left rather than where it moved along the edge on the client. Moving the re-entry point proportionally along the edge matches normal multi-monitor movement.

## Goals

- Per-device `compose` boolean in `[[deskflow.device]]` (default `false`; absent in config reads as `false`), toggled in Settings -> Screen inside the device's block beside "Share the clipboard", and preserved when re-attaching a device with a new fingerprint.
- When `compose = true` on the active device, pressing `Multi_key` (`0xFF20`) starts a local `libxkbcommon` Compose sequence using the system X11 Compose table. Intermediate keys (and modifiers pressed mid-sequence) are held back from the client; on completion, ThinFloe sends `KeyDown` and `KeyUp` for the composed `KeyID` (with non-lock modifiers cleared so the client chooses its own modifiers for the character) and resets the compose state.
- When returning from a client (`Track::Back { along }`), place ThinFloe's pointer proportionally along the attached edge (`along` in `0..1`, vertical for left/right edges, horizontal for top/bottom edges) and update `hover.at` to match.

## Non-goals

- Dead keys (`dead_acute`, `dead_circumflex`, and similar `0xFE50..0xFE5C` keysyms) are not intercepted locally; they continue to travel as Deskflow dead-key IDs.
- Characters the client's active keyboard layout has no key sequence for, or Unicode code points above `U+FFFF`, remain a Deskflow client and protocol limit.
- No wire-protocol or Deskflow client changes.

## Anti-goals

- Stuck compose state or stuck modifiers on the client after an invalid or cancelled sequence, escape chord, disconnect, keymap reload or capture release.
- `Ctrl+Alt+Shift+Escape` blocked mid-compose ("release always wins" stays ahead of compose).
- Pointer bouncing immediately back onto the client after re-entry at the new position on the 1 px strip.

## Approaches

> **As built (2026-11-23):** pointer re-entry ships Approach 2B
> (`zwlr_virtual_pointer_v1` absolute motion after the unlock), not 2A:
> on labwc 0.9.3 the cursor position hint on the 1 px layer strip warps to
> the wrong x. See `docs/notes/deskflow.md`.

### 1. Per-device local Compose (`Multi_key` only)

- **1A (chosen): `libxkbcommon` compose state (`xkb_compose_table_new_from_locale` / `xkb_compose_state`) via `xkbcommon-dl`.** Reuses the already-linked `xkbcommon-dl` crate (no new dependency) and matches ThinFloe's own local X11 Compose sequences 1:1. Falls back to `"en_US.UTF-8"` when `LC_ALL`/`LANG` is unset, `C` or `POSIX`. Requires `/usr/share/X11/locale/en_US.UTF-8/Compose` in the ThinFloe rootfs.
- **1B (rejected): embedded Rust Compose table.** Avoids the rootfs file dependency, but misses sequences users expect from X11 Compose and drifts from local ThinFloe behaviour.

### 2. Pointer re-entry position along the edge

- **2A (chosen): `zwp_locked_pointer_v1::set_cursor_position_hint(sx, sy)` + `surface.commit()` before `lock.destroy()`.** Uses the Wayland pointer-lock protocol already bound by `capture.rs` with no extra privileged protocol or virtual pointer device. Contingent on `labwc 0.9.3` honouring the hint on a layer-shell surface (verified in `test/runtime/test-labwc-runtime.sh`).
- **2B (fallback only): `zwlr_virtual_pointer_v1` absolute warp after unlock.** Works regardless of hint handling on layer-shell surfaces, at the cost of binding a new wlr protocol and creating a virtual pointer device on the seat. Used only if the runtime verification shows `labwc 0.9.3` ignores 2A on layer-shell surfaces.

## Constraints

- No new Cargo dependencies (`xkbcommon-dl 0.4.2` already exposes `xkbcommon_compose_option()`).
- Max 4 cores locally (`BUILD_JOBS=4`, `TEST_THREADS=4`, cargo and build locks).
- Verify `/usr/share/X11/locale/en_US.UTF-8/Compose` exists in the ThinFloe rootfs (with an integration guard).
- Update `docs/notes/deskflow.md` and `CHANGES.md` in house style.

## Left to the builder

- Exact switch label copy in Settings -> Screen (for example `"Handle Compose key on ThinFloe"`).
- Synthetic `button` value for composed `KeyDown`/`KeyUp` checked against Deskflow `KeyState::fakeKeyDown` (`m_serverKeys` tracking) so it never collides with held keys or repeat tracking.
- Runtime test coverage in `test/runtime/test-labwc-runtime.sh` verifying both composed character delivery and `labwc 0.9.3` cursor re-entry position.
