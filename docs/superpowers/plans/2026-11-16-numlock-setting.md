# Num Lock at start Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Keyboard tab switch "Num Lock on at start" that sets labwc's `<numlock>` for the next session start.

**Architecture:** `[keyboard] numlock` in `config.toml` -> `THINFLOE_NUMLOCK=on|off` in `generated/environment` -> the full compose writes an `rc.xml` copy with `<numlock>off</numlock>` and links it.

**Tech Stack:** Rust (thinfloe-menu, egui/shadcn-egui), POSIX sh + awk (composer), bash tests.

**Spec:** `docs/superpowers/specs/2026-11-16-numlock-setting-design.md`

## Global Constraints

- Default on; a home without the key behaves as today.
- Key name `THINFLOE_NUMLOCK`, values exactly `on` / `off`.
- `--env-only` never touches `rc.xml` (labwc-scale owns it in the session).
- UI: colours from the theme, text sizes from `brand.rs` (`ui_code_has_no_style_literals`).
- Machine: `cargo test -j4 -- --test-threads=4` under `systemd-run --user --scope -q -p MemoryMax=4G --`; `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests/numlock` for shell tests.

## Review Focus

- System `rc.xml` without a `<numlock>` line: the copy must still say off (insert into `<keyboard>`); test in Task 2.
- `THINFLOE_NUMLOCK=off` in the user's `environment`: ignored (managed key); test in Task 2.
- labwc-scale after an `off` compose: its scaled copy keeps `off`; test in Task 2.
- A changed Num Lock with an otherwise live keymap apply: the note says it follows at the next start; test in Task 1.
- Rebuild from `generated/` with a junk value: note, stays on; test in Task 1.

---

### Task 1: menu side (config, generated, rebuild, tab)

**Files:**
- Modify: `src/thinfloe-menu/src/config.rs` (`Keyboard` + default `numlock: true`)
- Modify: `src/thinfloe-menu/src/generated.rs` (`render("environment")`)
- Modify: `src/thinfloe-menu/src/convert.rs` (`keyboard_from_env`, test helper `kb`)
- Modify: `src/thinfloe-menu/src/keyboard.rs` (`KbEvent::NumLock(bool)`, note in `done`)
- Modify: `src/thinfloe-menu/src/keyboardtab_ui.rs` (switch row after CapsLock)
- Modify: `overlay/etc/thinfloe/menu-managed-keys` (`THINFLOE_NUMLOCK`)
- Modify: other struct literals the compiler names (`migrate.rs`, tests)

**Interfaces:**
- Produces: `Keyboard.numlock: bool`; `generated/environment` line `THINFLOE_NUMLOCK=on|off` after the XKB lines; const `NUMLOCK_NEXT_START = "Num Lock follows at the next start."` in `keyboard.rs`.

- [ ] Tests first: `environment_has_all_three_keys` extended to expect `THINFLOE_NUMLOCK=on` (default) and `=off`; `convert` test `numlock_from_environment` (`off` -> false, missing -> true, `maybe` -> true + note naming `THINFLOE_NUMLOCK`); `keyboard.rs` test `a_numlock_change_says_next_start` (event `NumLock(false)` returns a Keyboard with `numlock == false`; `done(.., Ok(Live))` sets note `NUMLOCK_NEXT_START`; a later layout-only change with `Live` clears the note); config test: TOML without `numlock` parses to true; managed-keys list test (config.rs:947) expects the new key.
- [ ] Run them, see them fail.
- [ ] Implement; switch label "Num Lock on at start", help line under the card is not needed (the note covers next start).
- [ ] `cargo fmt`, `cargo clippy --all-targets -D warnings`, `cargo test` green.
- [ ] Commit.

### Task 2: composer, scale helper, shell tests, docs

**Files:**
- Modify: `overlay/usr/bin/thinfloe-compose-labwc-config` (full mode only)
- Test: `test/integration/test-labwc-config-merge.sh`, `test/integration/test-labwc-scale.sh`, `test/integration/test-session-packages.sh` (managed keys: five keys)
- Docs: `CHANGES.md` (New), `docs/notes/menu.md` (Keyboard tab), `docs/notes/home-persistence.md` (generated/environment row), `CLAUDE.md` (environment line: `XKB_DEFAULT_*` and `THINFLOE_NUMLOCK` lines are dropped)

- [ ] Tests first: merge test cases: generated `THINFLOE_NUMLOCK=off` -> `rc.xml` is a link to `$OUT/.numlock/rc.xml`, which holds `<numlock>off</numlock>` and no `on`; fixture `rc.xml` with no `<numlock>` but a `<keyboard>` -> copy gains `<numlock>off</numlock>`; `on`/missing -> `rc.xml` links the system file; `THINFLOE_NUMLOCK=off` only in the user env -> link to system; `--env-only` with `off` leaves `rc.xml` as it was. Scale test: after an `off` compose, the scaled `rc.xml` contains `<numlock>off</numlock>`.
- [ ] Run, see them fail.
- [ ] Implement: after composing `environment`, `env_value THINFLOE_NUMLOCK "$OUT_DIR/environment"`; on `off`, awk the system `rc.xml` (replace `<numlock>…</numlock>`, or insert after `<keyboard>` when absent) into `$OUT_DIR/.numlock/rc.xml` via a temp file, then `ln -sfn`. Failure: log `$me: cannot write the Num Lock rc.xml` to stderr and keep the system link.
- [ ] Run the three shell tests and the menu tests; green.
- [ ] Docs, commit.
