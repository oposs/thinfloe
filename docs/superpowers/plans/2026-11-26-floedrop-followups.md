# FloeDrop follow-ups (PR #57) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close every bullet of PR #57's "Follow-ups" list: fix each defect and missing test, and give each item that stays a line under "Known limits" in `docs/notes/floedrop.md`.

**Architecture:** Small, local changes in the FloeDrop crate (`src/thinfloe-share/`: names, inbox, outbox, daemon, radio, card, osd, bluez) and in the menu (`share_state.rs`, `sharecfg.rs`, `convert.rs`, `osd/draw.rs`, `brand.rs`). No contract changes beyond one new flag word `no-peers` in the existing flags field of the state file's `ready` line (readers already ignore unknown flags).

**Tech Stack:** Rust (tokio, zbus, inotify, Owly 0.4.3 at 21171d8), egui (CPU raster), bash runtime test.

**Spec:** `docs/superpowers/plans/2026-11-26-round-fix-everything.md`, section "Branch B: fix/floedrop-followups", and the "Follow-ups" section of PR #57 (`gh pr view 57 --repo oposs/thinfloe --json body -q .body`).

**Branch:** `fix/floedrop-followups`, cut from `round/2026-11-26b` **after branch A (`fix/floedrop-hardware`, plan `2026-11-26-floedrop-hardware.md`) has joined the round**. Tasks 1 to 6 and 8 to 11 do not depend on A's code; Task 7 (radio) is written against A's result.

## Deviations from the round spec

1. **OSD note width at scale > 1: already fixed; verify and add the runtime check.** The note panel already takes the menu's scale file (`run.rs:469` `read_scale`, passed to `Painter::note_fit`) and the output's logical width (`surface.rs:342` `logical_width` divides by `wl_output` scale); both came with #57's fix wave, after the bullet was written. `draw.rs` tests `a_note_panel_stays_inside_a_narrow_output` (scale 2) and `the_longest_floedrop_notes_show_whole` already pin it. Task 11 adds one unit test (scale 2 is twice scale 1) and the runtime note check, which is cheap because the media-keys case already runs the real OSD and has a region pixel finder.
2. **"One folder per phone": Owly gives no sender ID in a `/Discover` answer**, only `computer_name`, `model_name` and `record` (`ReceiverRecordData`, `owly-airdrop/src/proto/sender.rs:97-101`). The plan keys on a hash of `record` when a phone sends one; a phone in "Everyone" mode may send none, so the address match stays the fallback and the hardware check stays on the owner checklist with a Known-limits line. Matching by name was rejected: two phones both called "iPhone" would share a folder and get each other's files.
3. **"Send needs twice the batch size": a batch that cannot fit is not offered and its files stay.** The spec asks only for the message. Deleting files that were never offered would lose them for no reason; the owner's "declined and failed offers delete the entries" ruling (2026-11-25) is about offers. The batch is tried again at the folder's next change.
4. **The prune race is fixed by a single writer, not by a re-check.** Discovery no longer writes the target map; it hands the target to the outbox thread in `Cmd::Seen`, which inserts and prunes in one thread, in order. A time-stamp re-check still loses a target when the outbox prunes before it reads a `Seen` that was queued earlier.
5. **"The veth tests pass silently without user namespaces": the note says "with CI set they fail instead", but the code never checks `CI`.** Task 6 makes the code do what the note says.

## Global Constraints

- Never more than 4 cores: every cargo call carries `-j 4`, `CARGO_BUILD_JOBS=4` and `--test-threads=4`, under `systemd-run --user --scope -q -p MemoryMax=2G --` (the Makefile's `MEMCAP`). No temp files in `/tmp`: tests use `TMPDIR=$PWD/.tmp` (the Makefile's `TEST_ENV`) or `target/test-tmp`.
- One test, share crate (written `SHARE_T <filter>` below): `cd src/thinfloe-share && TMPDIR=$PWD/../../.tmp CARGO_BUILD_JOBS=4 systemd-run --user --scope -q -p MemoryMax=2G -- cargo test --locked -j 4 <filter> -- --test-threads=4`
- One test, menu crate (`MENU_T <filter>`): the same in `src/thinfloe-menu`.
- Whole crates with fmt and clippy: `make test-share`, `make test-menu`.
- Contracts (CLAUDE.md, FloeDrop): the state file's format does not change; the `ready` line's flags field gains the word `no-peers`. The OSD note datagram stays `note<TAB>tone<TAB>text<LF>`. The daemon reads only `generated/share`; the menu is the only writer of `[share]`.
- `generated/share` stays `enabled=`, `name=` lines; the menu's new reader accepts what the daemon's `config::parse` accepts (`true`, `1`, `yes`).
- UI code takes sizes from named constants in `brand.rs` (`ui_code_has_no_style_literals`).
- English in code and docs; user-facing text in English (the menu is English).
- Copy fixed by this plan (exact):
  - `Could not send to {device}: not enough room in FloeDrop for {n} MB` (n = bytes / 1 000 000, rounded up)
  - `Waiting to send to {device}: a file in its folder is still open`
  - `Could not remove {name} from the folder of {device}; it is not sent again`
  - `{placed} of {total} files from {sender} received; the others could not be saved`
  - unknown sender in notes: `an unnamed phone`
  - `The FloeDrop folder {root} is not mounted.`
  - `The Wi-Fi radio is in use by another program.`
  - band, ready: `While FloeDrop is on, {card} is not used for Wi-Fi.`
  - band, flag `no-peers`: `owlyd did not answer at start; FloeDrop finds phones over mDNS only.`
  - band, unreadable head: `FloeDrop's state file has a line the menu cannot read: {line:?}`

## Overlap with branch A (rebase notes)

| File | Branch A | This branch | Expect |
|------|----------|-------------|--------|
| `radio.rs` `hold`, `main`, `gather`, `RadioApi`, test `Fake` | A1 channel after country, A3 country fallback, A4 EBUSY/counters, A6 per-card probe | Task 7 (all radio bullets) | Task 7 is written against A's code; read A's `hold` first |
| `card.rs` | A3, A6 (candidate order), A7, A9 (skip list) | Task 8: `configured()` and a route test only | text conflicts in `tests` only |
| `daemon.rs` `on_receiver`, `Airdrop::start`, `shutdown`, line 491 | A2 receive logs and `receiving` (A's Task 6 turns `sender_of` into `take(&mut self, generation, id) -> Incoming`, unknown sender "iPhone"), A5 mDNS withdraw, A8 text, A11 doc comments | Task 2 (`Received` branch, `sender_of`), Task 5 (targets), Task 6 (`run`, `discovery`, `start`) | merge A2's log lines into Task 2's `Received` branch by hand |
| `state.rs` | A2 `DevState::Receiving` | none | none |
| menu `share_state.rs` `status_lines` and its tests | A2 receiving line, A3 country warning | Task 9 (two lines, unreadable head) | exact-array tests: add A's lines in A's order, this branch's lines after them |

## Ledger: every PR #57 follow-up bullet

| # | Bullet (short) | Decision | Where |
|---|----------------|----------|-------|
| F1 | One folder per phone assumes mDNS addr = link-local of AWDL MAC | fix where Owly gives `record`; else known limit + owner check | Task 5, Task 12 |
| F2 | awdl/mDNS key order not fixed | fix: the `awdl-` key wins | Task 5 |
| F3 | Prune race erases a fresh target | fix: one writer | Task 5 |
| F4 | OSD note width only right at scale 1; no runtime note check | already fixed (Deviation 1); runtime check added | Task 11 |
| F5 | A send needs the batch twice | known limit; refused before sending with the room note | Task 4, Task 12 |
| F6 | Card guard only at radio start | known limit; the band says it | Task 9, Task 12 |
| F7 | `convert.rs` drops `share` | fix | Task 10 |
| F8 | Note says "no default route", code is narrower; shared mode | fix the note | Task 12 |
| N1 | A cut can drop an extension | fix | Task 1 |
| N2 | `unique()` can exceed 64 bytes | fix | Task 1 |
| N3 | Only controls gives "___" | fix: fallback | Task 1 |
| N4 | Test gaps: exact cut, `.bashrc`, "." | fix | Task 1 |
| N5 | The fallback is not sanitised | document + debug assert | Task 1 |
| C1 | `configured()` not scoped to `wifis:` | fix | Task 8 |
| C2 | Malformed ready line becomes Starting silently | fix: shown as an error line | Task 9 |
| C3 | IPv6 route filter has no test | fix | Task 8 |
| I1 | Weak symlink-chmod test | fix | Task 2 |
| I2 | Note > 1024 bytes dropped | fix: cut to fit | Task 2 |
| I3 | No test for a note without an OSD | fix | Task 2 |
| I4 | `place()` loses the partial list | fix | Task 2 |
| O1 | Held batch retries after 1 s; no hold limit | fix: 3 s, note after 30 s; no hard limit (known limit) | Task 3, Task 12 |
| O2 | A file kept open holds with no note | fix (same note) | Task 3 |
| O3 | Outbox root not canonicalised for /proc | fix | Task 3 |
| O4 | No wake on IN_DELETE | fix | Task 3 |
| O5 | A failed delete is offered again | fix | Task 3 |
| O6 | `recently_modified` recursion unbounded | fix: depth and count bounds | Task 3 |
| R1 | A Probe failure stops the radio | fix (or verify A6 did) | Task 7 |
| R2 | Signal handlers only after up; Bluetooth left on | fix | Task 7 |
| R3 | `connect()` before the card choice gives the owlyd error | fix | Task 7 |
| R4 | `radio.toml` written before Busy is known | fix | Task 7 |
| R5 | `posix/`, `right/` zone links give no country | fix | Task 7 |
| R6 | Radio tests leave `target/test-tmp/radio-*` | fix | Task 7 |
| R7 | 10 s BLE timeout can drop `advertise()` after power-on | fix | Task 7 |
| D1 | Discovery is sequential | fix: concurrent | Task 6 |
| D2 | veth tests pass silently without user namespaces | fix: fail under `CI` (Deviation 5) | Task 6 |
| D3 | Missing tmpfs only logged while ready | fix: state `error` | Task 6 |
| D4 | `bus.accept()` in a daemon test has no timeout | fix | Task 6 |
| D5 | The owlyd 5 s fallback is silent | fix: `no-peers` flag, band line | Task 6, Task 9 |
| D6 | An unknown sender is "iPhone" in the note | fix: "an unnamed phone" | Task 2 |
| M1 | Two `default_name` copies untested for agreement | fix: shared fixture | Task 10 |
| M2 | `clean_name` cuts before it trims | fix | Task 10 |
| M3 | File name `deskflow_sup.rs` also supervises FloeDrop | cheap: renamed `daemon_sup.rs` | Task 11 |
| S1 | Two `impl PanelView` blocks | cheap: merged | Task 11 |
| S2 | `OSD_ICON` used for row centring | cheap: `OSD_NOTE_ROW_H` | Task 11 |
| S3 | `unreachable!()` in `look()` | cheap: `look` returns `Option` | Task 11 |

## Review Focus

1. A long received file name (`<200 chars>.HEIC`) must keep `.HEIC` and stay within 64 bytes, also after " (2)" is added. Pinned in Task 1 (`a_long_name_keeps_its_extension`, `a_suffix_never_passes_the_limit`).
2. A phone found under two keys must never get files meant for another phone: matching by name is not allowed; two phones named "iPhone" with different addresses and no record keep two folders. Pinned in Task 5 (`two_phones_of_the_same_name_keep_two_folders`).
3. An outbox entry the daemon cannot delete (read-only subfolder from the remote session) must not be offered in a loop. Pinned in Task 3 (`an_entry_that_cannot_be_deleted_is_not_offered_again`).
4. A batch larger than the free space must leave the user's files where they are. Pinned in Task 4 (`a_batch_that_does_not_fit_is_kept_and_noted`).
5. A broken `config.toml` must not switch FloeDrop off or rename it. Pinned in Task 10 (`a_broken_config_keeps_floedrop_as_it_was`).

---

### Task 1: Safe names keep their extension and their limit

**Files:**
- Modify: `src/thinfloe-share/src/names.rs:36-87` (`safe`, `unique_dir`, `unique`), tests `:89-170`
- Modify: `src/thinfloe-share/src/inbox.rs:104` (file names use `safe_file`)

**Interfaces:**
- Produces: `pub fn safe_file(name: &str, fallback: &str) -> String` (as `safe`, but a cut keeps the extension: the part after the last `.` when the dot is not at 0 and the part is 1 to 16 bytes). `safe`, `unique` and `unique_dir` keep their signatures; all four return at most `MAX_NAME` (64) bytes.

- [ ] **Step 1: Write the failing tests** in `names.rs` `mod tests`:

```rust
#[test]
fn a_long_name_keeps_its_extension() {
    let s = safe_file(&format!("{}.HEIC", "a".repeat(200)), "file");
    assert_eq!(s.len(), MAX_NAME);
    assert!(s.ends_with(".HEIC"), "{s}");
    // Not an extension: too long, or a leading dot.
    assert_eq!(safe_file(&format!("x.{}", "b".repeat(80)), "file").len(), MAX_NAME);
}
#[test]
fn a_suffix_never_passes_the_limit() {
    let long = format!("{}.jpg", "a".repeat(60)); // 64 bytes
    let u = unique(&[long.clone()], &long);
    assert!(u.len() <= MAX_NAME && u.ends_with(" (2).jpg"), "{u}");
    let d = "d".repeat(64);
    let u = unique_dir(&[d.clone()], &d);
    assert!(u.len() <= MAX_NAME && u.ends_with(" (2)"), "{u}");
    assert!(std::str::from_utf8(u.as_bytes()).is_ok());
}
#[test]
fn a_name_of_controls_only_is_the_fallback() {
    assert_eq!(safe("\u{1}\u{2}\u{3}", "iPhone"), "iPhone");
    assert_eq!(safe("\u{202E}\u{200B}", "iPhone"), "iPhone");
    assert_eq!(safe("a\u{1}", "iPhone"), "a_");
}
#[test]
fn the_cut_and_dot_edges() {
    assert_eq!(safe(&"a".repeat(64), "f"), "a".repeat(64));
    assert_eq!(safe(&"a".repeat(65), "f"), "a".repeat(64));
    assert_eq!(safe(".bashrc", "f"), "_bashrc");
    assert_eq!(safe(".", "f"), "f");
}
```

- [ ] **Step 2: Run them to verify they fail:** `SHARE_T names::` — FAIL: `safe_file` not found (after stubbing it, the suffix and controls tests fail).
- [ ] **Step 3: Implement.** In `safe`: count the characters replaced; when every character was replaced, return `fallback`. Document on `safe` and `safe_file`: "`fallback` is returned as it is and must itself be a safe name" and add `debug_assert!(!fallback.is_empty() && fallback.len() <= MAX_NAME && !fallback.contains('/') && !fallback.starts_with('.'))`. `safe_file` cuts the stem, never the extension, at a char boundary. In `unique`/`unique_dir`, cut the stem (at a char boundary) so `stem + " (N)" + ext` fits `MAX_NAME`. In `inbox::place` (`inbox.rs:104`) use `names::safe_file` for plain files and keep `names::safe` for folders.
- [ ] **Step 4: Run** `SHARE_T names::` and `SHARE_T inbox::` — PASS.
- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-share/src/names.rs src/thinfloe-share/src/inbox.rs
git commit -m "fix(share): received names keep their extension and stay within 64 bytes"
```

### Task 2: Inbox placement, notes and the unknown sender

**Files:**
- Modify: `src/thinfloe-share/src/inbox.rs:11-148` (`place`, notes), tests `:257-275`
- Modify: `src/thinfloe-share/src/osd.rs:22-91` (`datagram`, `note`)
- Modify: `src/thinfloe-share/src/daemon.rs:1079-1134` (`sender_of`, the `Received` and `Failed` branches of `on_receiver`)

**Interfaces:**
- Produces:
  - `pub struct Placed { pub files: Vec<PathBuf>, pub error: Option<io::Error> }`
  - `pub fn place(staged: &[PathBuf], staging_root: &Path, inbox: &Path, sender: &str) -> Placed` (what was moved before an error stays in `files`; an error creating the folder gives empty `files`)
  - `pub const UNKNOWN_SENDER: &str = "an unnamed phone";` and `pub fn shown_sender(sender: &str) -> &str` (trimmed empty -> `UNKNOWN_SENDER`)
  - `pub fn partial_note(placed: usize, total: usize, sender: &str) -> String`
  - `osd.rs`: `pub const DATAGRAM_MAX: usize = 1024;` (the OSD's `socket::DATAGRAM_MAX`), `pub fn send_note(path: &Path, tone: Tone, text: &str) -> bool`; `note(tone, text)` calls it.
  - the sender name kept for a request (`Airdrop::sender_of` today, A's `take(…) -> Incoming` after the rebase) is the name as given, empty when unknown, never "iPhone"; the folder still falls back to `iPhone` through `names::safe`.

- [ ] **Step 1: Write the failing tests:**
  - `inbox::tests::place_keeps_what_it_moved_before_an_error`: stage `a.txt`, a directory `d` containing a 0o000 subdirectory (`normalise` fails on `read_dir`), and `z.txt`; expect `placed.files == [inbox/Jane/a.txt]`, `placed.error.is_some()`. Restore modes before the `Dir` drop.
  - `inbox::tests::place_drops_symlinks_without_following_them`: set `secret` to 0o600 before `place`; replace line 273 by `assert_eq!(mode(&secret), 0o600)`.
  - `inbox::tests::notes_name_an_unnamed_phone`: `received_note(&three, shown_sender(""))` == `"3 files received from an unnamed phone"`; `partial_note(2, 5, "Jane")` == `"2 of 5 files from Jane received; the others could not be saved"`.
  - `osd::tests::a_long_note_is_cut_to_one_datagram`: `datagram(Tone::Warn, &"Jäne ".repeat(400))` has `len() <= DATAGRAM_MAX`, starts with `"note\twarn\t"`, ends with `'\n'`, is valid UTF-8.
  - `osd::tests::a_note_without_an_osd_is_lost_at_once`: `send_note(Path::new("target/test-tmp/no-such-osd.sock"), Tone::Info, "x")` returns `false` within 1 s (assert `Instant::elapsed() < 1 s`).
- [ ] **Step 2: Run** `SHARE_T inbox::` and `SHARE_T osd::` — FAIL: `Placed`, `shown_sender`, `partial_note`, `send_note`, `DATAGRAM_MAX` are missing. The strengthened symlink test passes once it compiles (`place` never touches the target); it is the missing guard.
- [ ] **Step 3: Implement** as in Interfaces. `datagram` cuts the cleaned text at a char boundary so the whole datagram fits `DATAGRAM_MAX`. In `daemon.rs` `Received`: `files` not empty and no error -> info `received_note(&files, shown_sender(&sender))`; error -> log `placing files from {sender:?}: {e}`, remove the staged paths not in `files` (`remove_entry`), then a warn note: `partial_note(files.len(), paths.len(), …)` when `files` is not empty, else `failed_note(…)`. `Failed` and the decline note use `shown_sender` too (`answer(&r.ask.offer, free, shown_sender(&sender))`). Keep A2's log lines when rebasing.
- [ ] **Step 4: Run** `SHARE_T inbox::`, `SHARE_T osd::`, `SHARE_T daemon::` — PASS.
- [ ] **Step 5: Commit** — `git commit -am "fix(share): partial receives are noted, long notes are cut, an unnamed sender is not called iPhone"`

### Task 3: Outbox holds, deletes and walks

**Files:**
- Modify: `src/thinfloe-share/src/outbox.rs:17-127` (constants, `SystemProbe::busy`, `recently_modified`), `:129-145` (`Dev`), `:172-188` (`offerable`), `:348-441` (`tick`, `finished`), tests `:720-734`
- Modify: `src/thinfloe-share/src/daemon.rs:649-655` (`watch_mask`)

**Interfaces:**
- Produces: `const HOLD_RETRY: Duration = BATCH_DELAY;` `pub const HOLD_NOTE_AFTER: Duration = Duration::from_secs(30);` `pub const WALK_MAX_DEPTH: usize = 32;` `pub const WALK_MAX_ENTRIES: usize = 10_000;`. `Dev` gains `held_since: Option<Instant>`, `hold_noted: bool`, `stuck: HashSet<PathBuf>`. `fn offerable(dir: &Path, stuck: &HashSet<PathBuf>) -> Vec<PathBuf>`. `pub fn recently_modified(p: &Path, now: SystemTime) -> bool` keeps its signature (bounded inside).

- [ ] **Step 1: Write the failing tests** in `outbox.rs` `mod tests`:
  - `a_file_still_growing_holds_the_batch`: change the wake assertion to `wake > t.at(3) && wake <= t.at(6)`, the second look to `t.tick(6)`, the send to `t.tick(9)`.
  - `a_long_hold_is_noted_once`: busy from 3 s on; `t.tick(3)`…`t.tick(32)` gives no note; `t.tick(33)` gives exactly `[(Tone::Warn, "Waiting to send to Jane's iPhone: a file in its folder is still open")]`; `t.tick(63)` gives none; after `busy=false` the batch goes.
  - `an_entry_that_cannot_be_deleted_is_not_offered_again`: Jane's folder holds `d/sub/f` with `sub` at 0o500; offer, `finished(1, Delivered)`; the notes contain `"Could not remove d from the folder of Jane's iPhone; it is not sent again"`; `t.put(JANE, "b.jpg")`, `changed`, tick: the second call is `[b.jpg]` only. Restore 0o755 before the end.
  - `a_deep_tree_stops_the_walk`: a 40-deep folder chain with a fresh file at the bottom: `recently_modified(top, now)` is false; the same at depth 5 is true.
  - `busy_follows_a_symlinked_root`: `real/x` opened for append through `real/`; `SystemProbe.busy(&[link/x])` is true where `link -> real`.
  - `daemon::tests::the_outbox_wakes_on_delete`: `watch_mask()` contains `WatchMask::DELETE` and `WatchMask::MOVED_FROM`.
- [ ] **Step 2: Run** `SHARE_T outbox::` and `SHARE_T the_outbox_wakes` — FAIL.
- [ ] **Step 3: Implement.** `busy`: canonicalise each entry (`fs::canonicalize`, the raw path when it fails) before the `/proc` match. `recently_modified`: a private walker with a depth counter and a shared entry counter; past either bound it stops and reports not modified (the `/proc` check still holds open files). `tick`: on busy set `held_since` once; past `HOLD_NOTE_AFTER` push the note once (`hold_noted`); a send or an empty folder clears both. `finished`: a failed `delete` puts the path into `stuck` and pushes the note (name = file name of the entry); `offerable` skips `stuck`; `tick` drops from `stuck` the paths that no longer exist. `watch_mask` adds `DELETE | MOVED_FROM`. No hard hold limit: a half-written file must not be sent.
- [ ] **Step 4: Run** `SHARE_T outbox::` and `SHARE_T daemon::` — PASS.
- [ ] **Step 5: Commit** — `git commit -am "fix(share): outbox holds are noted, undeletable entries are not resent, walks are bounded"`

### Task 4: A batch that does not fit is refused with the reason

**Files:**
- Modify: `src/thinfloe-share/src/outbox.rs:44-64` (new trait beside `Probe`), `:147-157`, `:219-253` (`sent_note`, constructors), `:386-405` (`tick`)

**Interfaces:**
- Produces: `pub trait Space { fn free(&mut self) -> Option<u64>; }`; `pub struct SystemSpace(pub PathBuf)` using `crate::inbox::free_bytes`; `Outbox::new(root)` uses `SystemSpace(root.clone())`; `pub fn with_space(self, space: Box<dyn Space>) -> Outbox`; `pub fn batch_bytes(entries: &[PathBuf]) -> u64` (file lengths below the entries, symlinks not followed, the Task 3 walk bounds); `pub fn room_note(device: &str, bytes: u64) -> (Tone, String)`.

- [ ] **Step 1: Write the failing tests:**
  - `a_batch_that_does_not_fit_is_kept_and_noted`: a fake `Space` returning `Some(10)`; Jane's three 1-byte files plus one 20-byte file; tick at 3: no call; notes `[(Tone::Warn, "Could not send to Jane's iPhone: not enough room in FloeDrop for 1 MB")]`; the files are all still there; state `Failed`; after `free = Some(1 << 30)` and a `changed`, the next tick sends.
  - `a_full_spool_says_why`: `sent_note(&[a], "Jane's iPhone", &Outcome::Failed("No space left on device (os error 28)".into()))` gives the room note for the offered bytes; any other reason keeps `"Could not send to Jane's iPhone"`.
  - `room_note_rounds_up`: `room_note("X", 1_000_001).1` ends with `"for 2 MB"`.
- [ ] **Step 2: Run** `SHARE_T outbox::` — FAIL.
- [ ] **Step 3: Implement.** In `tick`, before `t.send`: when `space.free()` is `Some(f)` and `batch_bytes(&entries) > f`, set `pending = None`, state `Failed`, `since = epoch()`, push `room_note`; do not send, do not delete. `sent_note` maps a `Failed` reason containing `"os error 28"` or `"No space left"` to `room_note` (it needs the bytes: pass `batch_bytes(offered)` from `finished` before the delete).
- [ ] **Step 4: Run** `SHARE_T outbox::` — PASS.
- [ ] **Step 5: Commit** — `git commit -am "fix(share): a batch too large for FloeDrop's folder stays and says why"`

### Task 5: One device, one key, one writer of the targets

**Files:**
- Modify: `src/thinfloe-share/src/outbox.rs:129-145` (`Dev.stable`), `:255-337` (`seen`), tests `:551-558` (helper), `:592-616`, `:899-905`
- Modify: `src/thinfloe-share/src/daemon.rs:599-628` (`Cmd::Seen`, `Target`, `forget_targets`), `:659-707` (`outbox_thread`), `:711-811` (`discovery`), `:1223-1236` (test)

**Interfaces:**
- Produces:
  - `Outbox::seen(&mut self, service_id: &str, addr: &str, name: &str, stable: Option<&str>, now: Instant)`; match order: same service ID, then same `stable` (both `Some` and equal), then same `addr`. When the device's key starts with `awdl-` and the new one does not, the device keeps its key and the new key goes to `dropped` (its target is forgotten); otherwise the new key replaces the old, which goes to `dropped`.
  - `Cmd::Seen { key: String, addr: String, name: String, stable: Option<String>, target: Target }`
  - `pub fn stable_id(record: Option<&[u8]>) -> Option<String>` in `daemon.rs`: 16 hex digits of a `std::hash::DefaultHasher` over the bytes; `None` for `None` or empty.
  - `fn record_target(targets: &Targets, key: &str, t: Target)`; only `outbox_thread` calls it and `forget_targets`; `discovery` keeps no clone of `Targets` (its parameter goes).
- Consumes: Task 3/4 `Outbox` (unchanged otherwise).

- [ ] **Step 1: Write the failing tests:**
  - `outbox::tests::the_awdl_key_wins_over_an_mdns_key`: `seen_at("awdl-02:11:22:33:44:55", A, JANE, None, 0)`, then `seen_at("Jane-mdns", A, JANE, None, 1)`, then again `"awdl-…"` at 2 and `"Jane-mdns"` at 3; `take_dropped()` == `["Jane-mdns", "Jane-mdns"]`; the batch at 6 goes to `"awdl-02:11:22:33:44:55"`.
  - Change `an_awdl_peer_and_its_mdns_instance_share_one_folder` to expect the send to the `awdl-` key, and `a_rekey_drops_the_old_service_id` to the reverse order (an mDNS key first, then the `awdl-` key at the same address: the mDNS key is dropped).
  - `outbox::tests::one_record_is_one_phone_at_two_addresses`: `seen_at("awdl-a", "fe80::1", JANE, Some("r1"), 0)`, `seen_at("Jane-mdns", "fe80::99", JANE, Some("r1"), 1)`: one folder, no "(2)".
  - `outbox::tests::two_phones_of_the_same_name_keep_two_folders`: `"iPhone"` at `fe80::1` and `"iPhone"` at `fe80::2`, both `stable None`, both present: folders `iPhone` and `iPhone (2)`.
  - `daemon::tests::a_target_recorded_after_a_drop_stays`: `record_target(k)`, `forget_targets([k])`, `record_target(k)`: `k` is in the map (the old `dropped_service_ids_lose_their_targets` stays).
  - `daemon::tests::stable_ids`: `stable_id(None)` and `stable_id(Some(&[]))` are `None`; the same bytes give the same 16-digit hex twice; other bytes another.
- [ ] **Step 2: Run** `SHARE_T outbox::` and `SHARE_T daemon::tests` — FAIL.
- [ ] **Step 3: Implement.** `discovery` builds the `Target` and sends it in `Cmd::Seen` with `stable: stable_id(r.record.as_deref())`; `outbox_thread` on `Seen` calls `record_target` then `ob.seen(…)`; after `tick` it calls `forget_targets(&targets, &ob.take_dropped())` as today. Update the `T` helper `seen_at(id, addr, name, stable, s)` and `seen(id, name, s)` (stable `None`).
- [ ] **Step 4: Run** `SHARE_T outbox::`, `SHARE_T daemon::`, `SHARE_T --test veth` — PASS.
- [ ] **Step 5: Commit** — `git commit -am "fix(share): a phone keeps one key and one folder; the send targets have one writer"`

### Task 6: Daemon start, discovery and its tests

**Files:**
- Modify: `src/thinfloe-share/src/daemon.rs:256-267` (root), `:417-455` (`run` start), `:463-490` (ready head), `:711-811` (`discovery`), `:849-936` (`Airdrop::start`)
- Modify: `src/thinfloe-share/tests/daemon.rs:190-220` (the test around `bus.accept()` at line 207), `src/thinfloe-share/tests/veth.rs:32-53` (`enter`)

**Interfaces:**
- Produces:
  - `pub fn is_tmpfs(path: &Path) -> std::io::Result<bool>` (`libc::statfs`, `f_type == libc::TMPFS_MAGIC`)
  - `fn root_must_be_tmpfs() -> bool`: true when `TF_SHARE_ROOT` is unset, or `TF_SHARE_REQUIRE_TMPFS=1`
  - `pub fn not_mounted(root: &Path) -> String` = `"The FloeDrop folder {root} is not mounted."`
  - `Airdrop` gains `pub peers: bool` (false when owlyd's link was not used); `run` appends `"no-peers"` to the ready head's `flags` when false. The `ready_text` log line is unchanged.
- [ ] **Step 1: Write the failing tests:**
  - `daemon::tests::proc_is_not_a_tmpfs`: `is_tmpfs(Path::new("/proc")).unwrap()` is false; when `/dev/shm` exists, `is_tmpfs` on it is true.
  - `tests/daemon.rs` `a_missing_tmpfs_is_an_error`: `Env` with `TF_SHARE_REQUIRE_TMPFS=1` (its root lies in `target/`), helper that touches `started`: the daemon exits 1, the state is `"error\tThe FloeDrop folder <root> is not mounted.\n"`, the helper was never started.
  - `tests/daemon.rs` (line 207): `bus.set_nonblocking(true)` and `assert!(wait_for(Duration::from_secs(10), || bus.accept().is_ok()), "the daemon never asked the bus")` (keep the accepted stream alive: store it from the closure).
  - `tests/veth.rs` `enter`: when the probe fails and `std::env::var_os("CI").is_some()`, `panic!("{test}: user namespaces are needed in CI")`. No new test; checked by reading.
  - `daemon::tests::the_ready_head_says_no_peers`: a pure `fn ready_flags(flags: Vec<String>, peers: bool) -> Vec<String>` gives `["no-ble", "no-peers"]` for `(["no-ble"], false)` and the input for `true`.
- [ ] **Step 2: Run** `SHARE_T daemon::` and `SHARE_T --test daemon` — FAIL.
- [ ] **Step 3: Implement.** `run`: after `paths.prepare()` and before the helper starts, when `root_must_be_tmpfs()` and `is_tmpfs(&paths.root)` is not `Ok(true)`, write `Head::Error(not_mounted(&paths.root))`, log it, return 1. `Airdrop::start` sets `peers` from the `Ok`/`Err` arm of the owlyd lookup. `discovery`: collect the due candidates' `(addr, SocketAddrV6)`, run their `timeout(DISCOVER_WAIT, s.discover(sock))` together with `futures_util::future::join_all`, then apply the results to `cands` in one pass (the `Target` and `Cmd::Seen` of Task 5).
- [ ] **Step 4: Run** `SHARE_T daemon::`, `SHARE_T --test daemon`, `SHARE_T --test veth` — PASS.
- [ ] **Step 5: Commit** — `git commit -am "fix(share): no start without FloeDrop's tmpfs; discovery asks receivers together; owlyd's absence is reported"`

### Task 7: Radio helper start order (on top of branch A)

Read A's `hold`, `main`, `gather` and the test `Fake` in `radio.rs` first; the names below are today's. A's plan (Task 2) gives `card::candidates`, replaces `mode_channel` by `RadioApi::probe`, writes `radio.toml` once per candidate before its probe, and keeps a probe *call* error fatal (exit 1). This task changes exactly that last rule and adds the `held()` check before A's candidate loop.

**Files:**
- Modify: `src/thinfloe-share/src/radio.rs` `RadioApi` (today `:31-41`), `hold` (`:144-213`), `zone` (`:410-416`), `ble` (`:453-472`), `stop` (`:474-487`), `main` (`:497-544`), tests `tmp` (`:627-633`)
- Modify: `src/thinfloe-share/src/bluez.rs:138-169` (`BluezAdvertiser::advertise`), `:271-278` (`advertise`)

**Interfaces:**
- Produces:
  - `RadioApi::held(&self) -> bool`: owlyd's `Lease` property is not 0 (after the stale release). `pub const BUSY: &str = "The Wi-Fi radio is in use by another program.";`
  - `pub fn zone_of(link_target: &str) -> Option<String>`: the zone after `zoneinfo/`, without a leading `posix/` or `right/`.
  - `card::candidates(i: &CardInputs) -> Result<Vec<String>, String>` comes from branch A (its Task 2); this task only moves its call before `Owlyd::connect`.
  - `struct Stop { term: Signal, int: Signal }`, `fn install_stop() -> Result<Stop, String>`, `async fn stop(s: Stop)` (stdin EOF, SIGTERM or SIGINT).
  - `bluez::advertise(advert: Advert, wait: Duration) -> io::Result<Option<Registration>>`: `wait` bounds each D-Bus call; a timeout after the adapter was powered on unregisters the advert and powers it off before the error returns.
  - Test helper `struct Tmp(PathBuf)` with `fn toml(&self) -> PathBuf` and a `Drop` that removes the `radio-<name>-<pid>` directory.
- [ ] **Step 1: Write the failing tests** in `radio.rs` `mod tests`:
  - `a_held_radio_writes_nothing`: `Fake { held: true, .. }`: exit 1, `out == format!("error\t{BUSY}\n")`, `radio.toml` not written, no `acquire` in `calls`.
  - `a_probe_error_tries_the_next_card`: the Fake's `probe` (A's) returns `Err("owlyd's probe: …")` for the first candidate and accepts the second: the up line names the second card. When every probe fails: exit 1 and the error line carries the last probe error.
  - `posix_and_right_zones_have_a_country`: `zone_of("../usr/share/zoneinfo/posix/Europe/Zurich")` and `…/right/Europe/Zurich` are `Some("Europe/Zurich")`; `zone_of("/usr/share/zoneinfo/UTC")` is `Some("UTC")`; `zone_of("/etc/foo")` is `None`.
  - `no_free_card_is_said_before_owlyd`: `candidates(&inputs_with_no_free_card)` is `Err(NO_CARD)`; `main` calls it before `Owlyd::connect` (review item; the function is pure).
  - Replace `tmp(name) -> PathBuf` by `Tmp`; `a_tmp_dir_goes_with_its_guard`: the directory is gone after the guard drops.
- [ ] **Step 2: Run** `SHARE_T radio::` — FAIL.
- [ ] **Step 3: Implement.** `main`: `install_stop()` first (before `gather`, so a SIGTERM during start no longer kills the helper with Bluetooth on); then `candidates` (error -> say, exit 1, no owlyd call); then `Owlyd::connect`. `hold`: release a stale lease, then `held()` -> `BUSY` before A's candidate loop writes any `radio.toml`; in A's loop a probe call error on one card is logged (`thinfloe-share: probe of <card>: <e>`) and the next card is tried, like a refusal; with no card accepted the error line is the first refusal, else the last probe error. `gather` uses `zone_of`. `ble` calls `bluez::advertise(advert, BLE_TIMEOUT)` without an outer `tokio::time::timeout`.
- [ ] **Step 4: Run** `SHARE_T radio::` and `SHARE_T card::` — PASS; `ls src/thinfloe-share/target/test-tmp | grep -c '^radio-'` prints 0.
- [ ] **Step 5: Commit** — `git commit -am "fix(share): the radio helper checks the card and the lease before it changes anything, and never leaves Bluetooth on"`

No unit test can drive BlueZ or a signal during start; owner checklist item 8 (Task 12) covers them.

### Task 8: Card guard: netplan scope and the route filter

**Files:**
- Modify: `src/thinfloe-share/src/card.rs:94-111` (`configured`), tests

**Interfaces:**
- Produces: `fn configured(yaml: &str, iface: &str, mac: Option<&str>) -> bool` (same signature) counts only lines inside a `wifis:` mapping: from a line whose key is `wifis` until the next line indented as little or less.

- [ ] **Step 1: Write the failing tests:**
  - `an_ethernet_entry_does_not_hold_a_wifi_card`: `"network:\n  ethernets:\n    wlp3s0:\n      dhcp4: true\n"` -> `choose_station` takes `wlp3s0`; the same under `wifis:` -> `Err(NO_CARD)`; two files joined by `"\n"` (the helper joins them), the second with `wifis:` naming `wlp3s0` -> `Err(NO_CARD)`.
  - `only_link_local_and_multicast_routes_are_free`: routes `[{"dst":"fe80::/64","dev":"wlx00e04c123456"},{"dst":"ff00::/8","dev":"wlx00e04c123456"}]` -> taken; adding `{"dst":"2001:db8::/64","dev":"wlx00e04c123456"}` -> `Err(NO_CARD)`; `{"dst":"169.254.0.0/16","dev":…}` -> `Err(NO_CARD)`; `{"dst":"default","dev":…}` -> `Err(NO_CARD)`.
- [ ] **Step 2: Run** `SHARE_T card::` — the ethernets case FAILS (today it errs safe); the route test passes at once (it is the missing guard).
- [ ] **Step 3: Implement** the scope in `configured` (track the indentation of the `wifis:` line; a line deeper than it is inside).
- [ ] **Step 4: Run** `SHARE_T card::` — PASS.
- [ ] **Step 5: Commit** — `git commit -am "fix(share): only netplan wifis entries hold a card back; the route filter has its own test"`

### Task 9: The FloeDrop band says more

**Files:**
- Modify: `src/thinfloe-menu/src/share_state.rs:76-146` (`parse`, `status_lines`), tests `:272-`

**Interfaces:**
- Produces: `parse` (A changes only `status_lines`): a `ready` line with fewer than 5 fields or a channel that is not a number gives `Head::Error(format!("FloeDrop's state file has a line the menu cannot read: {line:?}"))`; another unknown first word stays `Starting` (a newer daemon's head). `status_lines(&Head::Ready{..})` = the ready line, then `("While FloeDrop is on, {card} is not used for Wi-Fi.", Plain)`, then the `no-ble` line, then A's `no-country` line, then for flag `no-peers` `("owlyd did not answer at start; FloeDrop finds phones over mDNS only.", Plain)`. A's lines (A2, A3) keep A's places; these go after the ready line in this order.

- [ ] **Step 1: Write the failing tests:** `a_broken_ready_line_is_shown` (`parse("ready\tawdl0\texclusive\tsix\t\n")` and `parse("ready\tawdl0\n")` are `Head::Error` starting with `"FloeDrop's state file has a line the menu cannot read: "`; `parse("stopping\n")` is `Starting`); extend `the_band_names_the_card_and_its_driver` with the card line; `the_band_says_when_owlyd_was_missing` (flags `["no-peers"]`).
- [ ] **Step 2: Run** `MENU_T share_state` — FAIL.
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Run** `MENU_T share_state` and `MENU_T ui_code_has_no_style_literals` — PASS.
- [ ] **Step 5: Commit** — `git commit -am "fix(menu): the FloeDrop band shows an unreadable state, the held card and a missing owlyd"`

### Task 10: `[share]` survives a rebuild; names agree

**Files:**
- Modify: `src/thinfloe-menu/src/sharecfg.rs:9-71`
- Modify: `src/thinfloe-menu/src/convert.rs:339-379` (`from_generated`), tests `:683-760`
- Modify: `src/thinfloe-share/src/daemon.rs:1384-1390` (test)
- Create: `test/unit/floedrop-names.tsv`

**Interfaces:**
- Produces: `pub fn parse(body: &str) -> ShareCfg` in `sharecfg.rs` (the rules of `thinfloe-share`'s `config::parse`: `enabled` true for `true`, `1`, `yes`; `name` trimmed; unknown keys and lines without `=` ignored). `clean_name`: drop controls, trim, cut to `MAX_NAME_BYTES` at a char boundary, trim the end again. `test/unit/floedrop-names.tsv`: lines `host<TAB>expected name` (`thinfloe-56db68\tFloeDrop 56db68`, `desk-7\tFloeDrop desk-7`, `thinfloe-\tFloeDrop thinfloe-`, `THINFLOE-ab\tFloeDrop THINFLOE-ab`, `thinfloe-ü1\tFloeDrop ü1`), `#` lines are comments; both crates read it with `include_str!` (menu: `"../../../test/unit/floedrop-names.tsv"` as `addons.rs:377` does; share: `"../../../test/unit/floedrop-names.tsv"` from `src/daemon.rs`).

- [ ] **Step 1: Write the failing tests:**
  - `convert::tests::a_broken_config_keeps_floedrop_as_it_was`: a `generated/` with only `share` (`generated::HEADER` line, then `enabled=true\nname=Front desk\n`): `from_generated` is `Some(cfg)` with `cfg.share == ShareCfg { enabled: true, name: "Front desk".into() }`; extend `from_generated_reads_back_what_render_wrote` with a non-default `share`.
  - `sharecfg::tests::parse_reads_what_render_writes` (round trip; `enabled=yes` is true; `noise` ignored).
  - `sharecfg::tests::blanks_do_not_count_against_the_limit`: `clean_name(&format!("{}{}", " ".repeat(10), "a".repeat(60)))` == `"a".repeat(60)`; `clean_name(&format!("{} b", "a".repeat(63)))` == `"a".repeat(63)`.
  - `sharecfg::tests::default_name_matches_the_daemon` and `daemon::tests::default_name_matches_the_menu`: every row of the table.
- [ ] **Step 2: Run** `MENU_T sharecfg`, `MENU_T convert`, `SHARE_T default_name_matches` — FAIL.
- [ ] **Step 3: Implement.** `from_generated` reads `share`, counts it in the "holds no file" check, and sets `cfg.share = sharecfg::parse(strip_header(t))`. Update the doc comment of `from_generated` (`[status]` and `[notice]` stay default; `[share]` is read).
- [ ] **Step 4: Run** `MENU_T sharecfg`, `MENU_T convert`, `MENU_T store`, `SHARE_T default_name` — PASS.
- [ ] **Step 5: Commit** — `git add test/unit/floedrop-names.tsv && git commit -am "fix(menu): a rebuilt config keeps FloeDrop's switch and name; both name rules are pinned by one table"`

### Task 11: OSD tidy, note checks, supervisor file name

**Files:**
- Modify: `src/thinfloe-menu/src/osd/draw.rs:44-63` (merge the two `impl PanelView`), `:79-135` (`look`), `:197-203` (`panel_ui`), `:295-316` (`note_fit`), tests
- Modify: `src/thinfloe-menu/src/brand.rs:215-227` (new constant)
- Rename: `src/thinfloe-menu/src/deskflow_sup.rs` -> `src/thinfloe-menu/src/daemon_sup.rs`; update `lib.rs`, `app.rs`, `share_state.rs`, `deskflow/mod.rs`, `CLAUDE.md` ("Deskflow server" line of "Where code lives"); module doc: "Supervision of the menu's daemons: Deskflow and FloeDrop."
- Modify: `test/runtime/test-labwc-runtime.sh` (`mk_script` case list `:1326-1489`, checks after `mk_run osd` `:1581-1611`)

**Interfaces:**
- Produces: `pub const OSD_NOTE_ROW_H: f32 = 22.0;` ("one row of a note, as high as a key panel's icon row"); `fn look(v: &PanelView, t: &Theme) -> Option<Look>` (`None` for `Note`). A new `mk_script` case `note`.

- [ ] **Step 1: Write the failing tests:**
  - `osd::draw::tests::a_note_at_scale_2_is_twice_as_wide`: for `"FloeDrop is ready"` on `Some(3840)`: `fit2.size.0` within 2 px of `2 * fit1.size.0`, and `fit2.size.1` within 2 px of `2 * fit1.size.1`.
  - `osd::draw::tests::a_note_has_no_look`: `look(&note(false, "x"), &Theme::default())` is `None` (use the theme the other draw tests use).
  - Runtime case `note` (one 1280x720 output): `start_osd`; send `note\twarn\t<"FloeDrop " x 40, trimmed>\n` with `python3 -c 'import socket,sys; s=socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM); s.sendto(sys.argv[1].encode(), "/tmp/xdg/thinfloe-osd.sock")' "$text"`; after 0.5 s `px note --within 1 left=270,600,300,697#151b22 right=980,600,1010,697#151b22`; then `printf '2\n' >/tmp/xdg/thinfloe-scale`, send it again, after 0.5 s `px note2 --within 1 left2=30,500,60,675#151b22`; `stop_osd`. Checks: `mk_expect "a two-row note at scale 1 is 720 px wide, centred" note left=found right=found` and `mk_expect "at scale 2 the note keeps 44 px to the edges of a 1280 px output" note2 left2=found`. (Scale 1: at most 720 wide, centred: x 280..999; scale 2: `OSD_NOTE_SIDE` 22 x 2 from each edge: x 44..1235.)
- [ ] **Step 2: Run** `MENU_T osd::draw` — FAIL (`look` returns `Look`).
- [ ] **Step 3: Implement.** `panel_ui`: `let Some(l) = look(v, &t) else { return };` after the note branch. `note_fit`: `OSD_NOTE_ROW_H` replaces `OSD_ICON` for the row height. The rename with `git mv`; `rg -n deskflow_sup` in `src/` and `CLAUDE.md` finds nothing afterwards (the old plans in `docs/superpowers/` keep the old name).
- [ ] **Step 4: Run** `make test-menu` — PASS; `make test-runtime` (needs the image) — the two new `ok` lines.
- [ ] **Step 5: Commit** — `git commit -am "refactor(menu): tidy the OSD note code, check notes at runtime, name the daemon supervisor for both daemons"`

### Task 12: CHANGES.md and notes

**Files:**
- Modify: `CHANGES.md` (`## [Unreleased]`, no release header)
- Modify: `docs/notes/floedrop.md`, `docs/notes/media-keys.md`

- [ ] **Step 1: CHANGES.md** under `### Changed`:
  - `- When a batch in a phone's FloeDrop folder is larger than the space left in /media/tfuser/FloeDrop, it is no longer offered and the files stay; a note says "Could not send to … : not enough room in FloeDrop for … MB". A send that runs out of space on the way says the same.`
  - `- When a file in a phone's FloeDrop folder stays open for 30 seconds, a note says "Waiting to send to …: a file in its folder is still open"; the batch goes when the file is closed.`
  - `- Settings → Network says "While FloeDrop is on, … is not used for Wi-Fi." for the card FloeDrop holds, and "owlyd did not answer at start; FloeDrop finds phones over mDNS only." when FloeDrop started without the radio's peer list.`

  Under `### Fixed`:
  - `- After a damaged settings file was rebuilt, FloeDrop was off and its name was gone; both are now kept.`
  - `- Files received from a phone with a long name showed no note, and a phone that gave no name was called "iPhone" in the notes; the notes now appear and say "an unnamed phone".`
  - `- Received files with long names lost their extension (".HEIC"), and a second file of the same long name could get a name longer than 64 bytes; both are fixed.`
  - `- When some files of a delivery could not be saved, no note mentioned the ones that arrived; the note now says "… of … files from … received; the others could not be saved".`
  - `- A file FloeDrop could not remove from a phone's folder was offered to the phone again at every change; it is now sent once and a note says it stays.`
  - `- A phone found again by FloeDrop could get "Could not send to …" for up to 20 seconds, and a phone seen both over the radio and over mDNS could switch between the two at each rediscovery; both are fixed.`
  - `- FloeDrop started without its folder /media/tfuser/FloeDrop mounted and wrote into the root file system; it now stops with "The FloeDrop folder /media/tfuser/FloeDrop is not mounted."`
  - `- On a machine without a free Wi-Fi card, FloeDrop could say "owlyd cannot be reached" instead of "AirDrop needs a Wi-Fi card this machine does not use for its network.", and it changed /etc/owly/radio.toml while another program held the radio; it now says "The Wi-Fi radio is in use by another program." in that case.`
  - `- Switching FloeDrop off while it was starting could leave Bluetooth switched on.`
  - `- A time zone set as posix/… or right/… gave FloeDrop no country, so the radio ran on the slow default channel.`
  - `- A Wi-Fi card named only in the Ethernet part of a netplan file was not used by FloeDrop.`
  - `- A FloeDrop state the menu could not read showed "Starting…" for ever; Settings → Network now shows the line it could not read.`
- [ ] **Step 2: `docs/notes/floedrop.md`:**
  - Card choice: replace "no default route through it" with "no route through it other than IPv6 link-local (fe80::/64) and multicast (ff00::/8)"; add "Owly's shared mode (AWDL beside a connected station) is never used: the guard takes only a card the machine does not use, although the design spec allows shared mode." Add "A netplan file holds a card back only from its `wifis:` part." and "A zone set through `posix/` or `right/` gives the same country."
  - Processes: the helper checks the card choice before it talks to owlyd, stops with "The Wi-Fi radio is in use by another program." when owlyd has a lease, writes `radio.toml` only then, tries the next card when a probe fails, installs its signal handlers first, and bounds each BlueZ call by 10 s (a timeout after power-on turns the adapter off again). The daemon stops with "The FloeDrop folder … is not mounted." when the folder is not a tmpfs, and asks every due receiver at once.
  - Contracts: flags may hold `no-peers` (owlyd's link was not used: no AWDL peer list, mDNS only).
  - Behaviour rulings: a held batch is looked at every 3 s; after 30 s a note; no time limit. A batch larger than the free space is not offered and stays (Deviation 3). An entry that cannot be deleted is not offered again. One phone, one key: the `awdl-` key wins; a phone that sends a receiver record is matched by it across addresses. The notes call a phone without a name "an unnamed phone"; its inbox folder is `iPhone`. Names: a cut keeps an extension of up to 16 bytes; " (N)" never passes 64 bytes; a name of controls only is the fallback.
  - Tests: "with CI set they fail instead" now matches the code; replace the `make qemu-smoke` line if A10 has not already (A owns it).
  - Owner checklist, add: "8. Switch FloeDrop on and, within 2 s, off again, ten times; `bluetoothctl show` then says `Powered` as before, and `iw dev` shows no AWDL interface." and to item 3: "and check `ls /media/tfuser/FloeDrop/outbox`: one folder per phone, no `(2)`."
  - Known limits (replace the list's FloeDrop lines): "A phone that sends no receiver record and whose mDNS address differs from its AWDL MAC's link-local address still gets a second folder `<name> (2)`; check on hardware (owner checklist 3)." / "A send needs the batch's space twice inside the tmpfs (files and Owly's archive in `.spool`); a batch larger than the free space is refused with a note." / "The card guard runs at radio start only: Wi-Fi set up later on the card FloeDrop holds comes up after FloeDrop is switched off; the band says which card is held." / "A file held open holds its folder without a time limit; the note after 30 s says so." / Remove "Sequential discovery", "the Probe failure stops the radio" and "An unknown sender is called iPhone".
- [ ] **Step 3: `docs/notes/media-keys.md`**: in the text-notes paragraph add "`test-labwc-runtime.sh` checks a two-row note's width at scale 1 and 2 (case `note`)."; remove nothing else.
- [ ] **Step 4: Commit** — `git commit -am "docs: FloeDrop follow-ups in CHANGES.md and the notes"`

### Task 13: Gates

CLAUDE.md "Workflow": a feature branch in a round runs the full set before it joins the round branch; after a rebase with no conflict outside `CHANGES.md` only `make test-menu`.

- [ ] **Step 1:** `make test-share` and `make test-menu` (fmt, clippy `-D warnings`, tests) — both pass.
- [ ] **Step 2:** `make test-unit` — pass (Python units, menu and share cargo tests).
- [ ] **Step 3:** `make image`, then `make test` — pass (`test-floedrop-image.sh` included).
- [ ] **Step 4:** `make test-runtime` — pass: this branch changes the menu and the OSD (Task 9, Task 11), with the new `note` case lines.
- [ ] **Step 5:** smoke tests the change touches: `make qemu-smoke` (boot, menu start, the FloeDrop band on a machine without an AWDL card). No other smoke target touches FloeDrop, the radio helper or the OSD notes: `-netboot`, `-upgrade`, `-two-sticks`, `-write`, `-grow`, `-timezone*`, `-addons`, `-automount` are not run.
- [ ] **Step 6:** Record the counts in the round PR body under this branch's heading (ok/not ok of `make test`, passed of the menu and share cargo tests, `make test-runtime` ok count, `qemu-smoke` PASSED), with the delta to `round/2026-11-26b` before this branch.

## Self-review

- Spec coverage: every ruling of the Branch B section has a task: folder key (Task 5, Deviation 2), key order (Task 5), prune race (Task 5, Deviation 4), note width (Task 11, Deviation 1), twice the batch (Task 4, Deviation 3), card guard (Task 9, Task 12), convert.rs (Task 10), note wording (Task 12), names/card/inbox/outbox/radio/daemon/menu/OSD bullets (Ledger), tidy bullets all cheap and done (Task 11), tmpfs recheck (Task 6). Every PR #57 bullet is in the Ledger.
- Types: `Outbox::seen(…, stable: Option<&str>, now)` (Task 5) is used by the helper and the outbox thread only; `Cmd::Seen` carries `Target` (Task 5) and is built in `discovery` (Task 6 keeps it); `Placed` (Task 2) is used only by `on_receiver`; `Space`/`batch_bytes` (Task 4) reuse Task 3's walk bounds.
- Open for the executor: Task 7 depends on A's restructured `hold`; its tests name behaviours, not A's private names.
