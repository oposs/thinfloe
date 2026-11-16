# "What's new" on the Upgrade screen — design

Date: 2026-11-16. Size: small change. Approved by the owner 2026-11-16.

## Intent

Before an upgrade or a fresh install, the user can read what changed
between the version on the chosen stick and the release that will be
written. The owner chose a button over notes shown automatically.

## Goals

- A "What's new" button on the Upgrade screen, next to the release line,
  opens a scrollable panel with the release notes of every version after
  the chosen stick's version up to the target release.
- The notes come from the same tag the write is pinned to.

## Non-goals

- Notes for versions before 4.2.0 (they are not in `CHANGES.md`).
- A Markdown renderer.
- Notes for a source entered by hand.

## Anti-goals

- A seventh release asset, or a version in an asset name.
- Any use of the GitHub API (60 requests/h without a token).
- Mounting a stick's `ramroot.sqsh` to read its version.

## Approaches

Stick version (owner picked 1):

1. **`version` file next to the kernel on the ESP**, written by every image
   build and every write. Cheap to read; sticks written before this change
   have none and fall back to "all notes up to the target". **Picked.**
2. Loop-mount the stick's `ramroot.sqsh` and read `/etc/thinfloe/version`.
   Works for old sticks, but costs a mount per drive listing and adds
   failure paths to a screen that must stay robust.
3. Know only the boot stick's version. Simplest, weakest.

Notes source (proposed earlier, accepted): `CHANGES.md` at
`https://raw.githubusercontent.com/oposs/thinfloe/v<target>/CHANGES.md`. One
file, pinned to the tag, no API limit, keeps the six-asset rule. The
alternative, the GitHub release bodies, needs the API.

## Design

- **Stick version.** `scripts/make-bootable-image.sh` copies
  `artifacts/rootfs/version` to `::tl/version`. `thinfloe-upgrade` writes
  the target's `version` into `tl/` (fresh install, via the image) and
  `tl.new/` (staged upgrade; it rotates with the folder in
  `tf_finalize_upgrade`). The `drives` JSON gets a `version` field:
  `tl.new/version` when an upgrade is pending, else `tl/version`, else
  empty; for the boot stick without the file, `/etc/thinfloe/version`. A
  value that does not match the version pattern is empty.
- **Notes command.** `thinfloe-upgrade notes --to X.Y.Z [--from A.B.C]`
  (batch, root via the existing `sudo -n`) fetches the pinned `CHANGES.md`
  with a size cap, cuts the `## X.Y.Z - date` sections with
  `from < version <= to` (dev versions `X.Y.Z-dev+sha` compare as `X.Y.Z`;
  `[Unreleased]` never counts) and prints them. Exit non-zero with a
  one-line reason on stderr when the fetch or the cut fails.
- **Range.** Upgrade with a known stick version: from that version. Fresh
  install: the target only. Upgrade with no known version: every release up
  to the target, with a line saying the stick's version is unknown. A stick
  already at or above the target: "No changes" text. Custom source: no
  button.
- **Panel.** Headings ("4.4.0 · New") and bullets as plain wrapped text,
  backticks removed. "Loading…" while the job runs; on failure "The release
  notes could not be fetched" and the reason. Close returns to the screen
  unchanged. Theme colours and `brand.rs` sizes only.

## Left to the builder

- The exact size cap, timeout and where the button sits in `upgrade_ui.rs`.
- Whether the menu parses sections itself from the tool's output or the
  tool prints a simple line format; the tool owns the cut either way.
- Caching the notes per (from, to) within one menu run.
- `CHANGES.md` entry under `[Unreleased]` New.
