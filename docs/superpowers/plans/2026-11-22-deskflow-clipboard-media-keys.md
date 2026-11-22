# Deskflow clipboard and media keys (steps 2 and 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Text copied on ThinFloe can be pasted on an attached Deskflow device and back (step 2), and while the pointer is on a device the volume and brightness keys act on that device while the microphone mute key stays on ThinFloe (step 3).

**Architecture:** Step 2 adds a pure clipboard codec (`deskflow/clipdata.rs`), clipboard messages in the sans-IO `Session`, a Wayland data-control bridge on its own connection (`deskflow/clip.rs`), a pure policy (`deskflow/clipsync.rs`) that `run.rs` drives, and the per-device Clipboard switch in the Deskflow band. Step 3 adds labwc patch `0007`, a keyboard-shortcuts inhibitor on the capture strip, and the local mic-mute rule.

**Tech Stack:** Rust (menu crate, wayland-client 0.31, wayland-protocols 0.32 `staging` for `ext_data_control_v1`, wayland-protocols-wlr for `zwlr_data_control_v1`), C patch on labwc 0.9.3, bash runtime tests.

**Spec:** `docs/superpowers/specs/2026-11-21-deskflow-server-design.md` (sections Clipboard, Keyboard → Media keys follow the pointer, Build order 2 and 3). Step-1 plan for conventions: `docs/superpowers/plans/2026-11-21-deskflow-server-core.md`. Notes: `docs/notes/deskflow.md`.

## Global Constraints

- Text clipboard only (format Text = 0, UTF-8, LF); clipboard id 0 (`kClipboardClipboard`) only; the primary selection (id 1) is not bridged.
- Cap: 1 MiB (1048576 bytes) of marshalled clipboard data per transfer; larger content is not sent and not accepted.
- The daemon reads only `generated/deskflow`, never `config.toml`; the menu is the only writer of `[deskflow]`.
- The daemon's event loop never blocks: every pipe read/write of clipboard data is non-blocking and polled; the 2 s watchdog must never fire because of a slow or hung clipboard peer.
- Getting back to ThinFloe never waits for the network or the client (Ctrl+Alt+Shift+Escape, watchdog, keep-alive) — unchanged by this work.
- No new compositor keybinding. The six media-key bindings stay `allowWhenInhibited="yes"`; toplevel inhibitors (OxuLnk, ThinLinc grab) keep today's meaning.
- Microphone mute stays on ThinFloe: `XF86AudioMicMute` is never sent to a device; on its release while captured the daemon runs `thinfloe-menu --osd-key mic-mute` only when the volume-key switch (`THINFLOE_VOLUME_KEYS=on`, which also binds mic-mute) is on.
- Builds: `make … VERSION=deskflow BUILD_JOBS=4`; cargo with `CARGO_TARGET_DIR=/scratch/oetiker/cargo-target-thinfloe-deskflow` and `-j 4`. Never more than 4 cores. Tests that feed adversarial input run under `systemd-run --user --scope -p MemoryMax=2G`.
- UI code: colours from the theme, text sizes from `brand.rs` (`ui_code_has_no_style_literals`). Labels in English, Simplified Technical English.
- CHANGES.md entries per the user's rules (reader-facing, ≤3 sentences, no release header).
- No code copied from Deskflow or lan-mouse (MIT project).

## Deskflow wire facts (read from Deskflow source 80221f0; the implementer may re-check in the clone at `/scratch/oetiker/claude-tmp/claude-1003/-scratch-oetiker-claude-worktrees-thinfloe-deskflow/4b793ca4-6740-4c0b-93d1-cdc95c78106a/scratchpad/deskflow-src`)

- Marshalled clipboard: u32 BE number of formats, then per format u32 format id, u32 size, bytes (`IClipboard.cpp`). ThinFloe sends exactly one format (Text, 0); on receive it takes the first Text entry and ignores other formats; a truncated entry yields no text.
- DCLP transfer = one start chunk (mark 1, data = the marshalled size as ASCII decimal, no NUL), one or more data chunks (mark 2, ≤ 512 KiB each), one end chunk (mark 3, empty data); all carry the same id and seq. An empty transfer still has one (empty) data chunk (`StreamChunker.cpp`).
- Receive (`ClipboardChunk::assemble`): data before start or end before start is an error; data beyond the declared size is an error; end with fewer bytes than declared is an error; a declared size above the cap is dropped silently (chunks ignored until end), not a disconnect.
- The client sends CCLP(id, seq) when it grabs; if it is not the active screen it sends the DCLP at once, else after COUT (`Client.cpp` ~l.240, ~l.660). The server sends CCLP to every other screen when one grabs and DCLP to the active screen; on entering a screen the server sends it DCLP.
- DSOP options: pairs of u32 (option code, value). `CLPS` (`kOptionClipboardSharing`, value 0/1) and `CLSZ` (`kOptionClipboardSharingSize`, value in KiB). Option codes are the four ASCII bytes as a big-endian u32 (`OPTION_CODE`, `OptionTypes.h` l.30 — verify).

## Review Focus

1. A clipboard owner that never writes, or writes forever (a hung app, `yes | wl-copy`): the daemon must stop reading at 1 MiB or after 2 s, drop the transfer, and keep forwarding input. → test in Task 2 (`reader_stops_at_cap_and_deadline`).
2. A hostile client sending DCLP chunks without start, oversize declared sizes, more data than declared, or an endless stream of chunks: no allocation above the cap, no disconnect for an oversize transfer, disconnect only for a protocol error. → tests in Task 1.
3. The daemon setting the ThinFloe selection from a device must not bounce the same text straight back to that device as a "change" (echo loop, like the CALV one of step 1). → test in Task 3 (`own_selection_is_not_sent_back`).
4. Clipboard switch off for one device: that device gets no CCLP/DCLP, its DSOP says `CLPS 0`, and text it sends anyway is ignored; switching it on or off takes effect on reload without a reconnect. → tests in Task 1 (DSOP on reload) and Task 3.
5. Media keys while captured with the volume-key switch off: they go to the device too (there is no ThinFloe binding to suppress), and mic-mute does nothing locally. With the switch on and not captured, every media key still acts on ThinFloe with a ThinLinc/OxuLnk inhibitor. → runtime cases in Task 6 and Task 7.

---

## Step 2: Clipboard

### Task 1: Clipboard codec and Session messages

**Files:**
- Create: `src/thinfloe-menu/src/deskflow/clipdata.rs` (+ `pub mod clipdata;` in `deskflow/mod.rs`)
- Modify: `src/thinfloe-menu/src/deskflow/session.rs`

**Interfaces:**
- Produces (clipdata.rs):
  - `pub const CLIP_MAX: usize = 1 << 20;` `pub const CHUNK: usize = 512 * 1024;`
  - `pub fn marshal_text(text: &[u8]) -> Vec<u8>`
  - `pub fn unmarshal_text(data: &[u8]) -> Option<Vec<u8>>`
  - `pub fn chunks(id: u8, seq: u32, marshalled: &[u8]) -> Vec<Msg>` (Msg::Dclp start/data…/end)
  - `pub struct Assembler` with `pub fn push(&mut self, id: u8, seq: u32, mark: u8, data: &[u8]) -> Asm`; `pub enum Asm { Started, InProgress, Finished(Vec<u8>), Dropped, Error }` (Finished carries the marshalled bytes; Dropped = oversize transfer ended or chunk of an oversize transfer).
  - `pub fn options(clipboard: bool) -> Vec<u32>`: `[CLPS, 1, CLSZ, 1024]` when on, `[CLPS, 0]` when off.
- Produces (session.rs):
  - `Session::set_clipboard(&mut self, on: bool) -> Vec<Out>`: stores the flag; when Active and the flag changed, sends a new DSOP.
  - The first DINF's DSOP now carries `clipdata::options(flag)` (flag default true until set).
  - `Out::Clipboard { text: Vec<u8> }` — a finished inbound transfer for id 0 with Text, only while the flag is on.
  - `Out::Grabbed` — the client sent CCLP for id 0 (flag on).
  - `Session::clipboard(&mut self, text: &[u8]) -> Vec<Out>`: CCLP + DCLP chunks (id 0, next seq) when live and flag on and `marshal_text(text).len() <= CLIP_MAX`; else nothing.
  - `Session::grab(&mut self) -> Vec<Out>`: CCLP only (tells an inactive device ThinFloe owns the clipboard).
  - An Assembler Error closes the connection ("bad clipboard"); Dropped is ignored.

- [ ] Step 1: failing tests in clipdata.rs: `marshal_roundtrip` (`unmarshal_text(&marshal_text(b"h\xc3\xa4llo\n")) == Some(b"h\xc3\xa4llo\n".to_vec())`), `unmarshal_skips_other_formats` (formats HTML then Text → the Text), `unmarshal_truncated_is_none`, `chunks_split_at_512k` (a marshalled input of CHUNK*2+1 bytes → start data "1048577", marks [1,2,2,2,3], data lengths [CHUNK, CHUNK, 1]), `empty_transfer_has_one_data_chunk`, `assembler_errors` (data before start → Error; more than declared → Error; end short → Error), `assembler_oversize_is_dropped_not_error` (start "2000000" → Dropped for start, data, end; no buffer grows: check an internal len accessor is 0), `options_codes` (CLPS == u32::from_be_bytes(*b"CLPS")).
- [ ] Step 2: failing tests in session.rs: `dsop_carries_clipboard_option`, `set_clipboard_resends_dsop_when_active`, `inbound_transfer_yields_clipboard`, `inbound_ignored_when_off`, `bad_chunk_closes`, `clipboard_out_sends_cclp_then_dclp`, `clipboard_over_cap_sends_nothing`.
- [ ] Step 3: implement; run `cargo test -j 4 deskflow::`; clippy, fmt.
- [ ] Step 4: commit `feat(deskflow): clipboard codec and messages`.

### Task 2: Wayland clipboard bridge

**Files:**
- Create: `src/thinfloe-menu/src/deskflow/clip.rs`
- Modify: `Cargo.toml` only if a feature is missing (`staging` is already on).

**Interfaces:**
- Produces:
  - `pub struct Clip`; `pub fn connect() -> Result<Clip, String>` — its own `wayland_client::Connection`; binds `ext_data_control_manager_v1`, else `zwlr_data_control_manager_v1`, else Err (the daemon then runs without clipboard and logs once).
  - `pub fn fd(&self) -> BorrowedFd<'_>`, `pub fn dispatch(&mut self, now: Instant) -> Result<Vec<ClipEvent>, String>`, `pub fn flush(&mut self) -> Result<(), String>`.
  - `pub fn pipe_fds(&self) -> Vec<(RawFd, libc::c_short)>` and `pub fn on_pipe_ready(&mut self, fd: RawFd, now: Instant) -> Vec<ClipEvent>`, `pub fn deadline(&self) -> Option<Instant>` and `pub fn tick(&mut self, now: Instant) -> Vec<ClipEvent>` (drops transfers older than 2 s).
  - `pub enum ClipEvent { Changed(Vec<u8>) }` — a selection set by another client, text read in full (≤ CLIP_MAX).
  - `pub fn set_text(&mut self, text: Vec<u8>)` — new data source offering `text/plain;charset=utf-8`, `text/plain`, `UTF8_STRING`, `STRING`, `TEXT` and the private marker `application/x-thinfloe-deskflow`; each `send(mime, fd)` writes the text non-blocking from the poll loop and closes the fd.
- Decisions: an offer that lists `application/x-thinfloe-deskflow` is ThinFloe's own and produces no event. Mime preference to read: `text/plain;charset=utf-8`, `UTF8_STRING`, `text/plain`, `STRING`, `TEXT`; an offer with none → no event. A new selection cancels a read in progress. Reading stops at CLIP_MAX+1 bytes (transfer dropped) or after 2 s. Pure helpers `pick_mime(&[String]) -> Option<&str>` and a `CappedRead` buffer type are unit-tested.

- [ ] Step 1: failing unit tests: `pick_mime_prefers_utf8`, `own_marker_is_skipped`, `reader_stops_at_cap_and_deadline` (CappedRead fed CLIP_MAX+1 bytes → Over; fed nothing past 2 s → Expired).
- [ ] Step 2: implement; a runtime smoke under headless labwc: `thinfloe-menu --deskflow-probe-clip` (hidden test mode, like the existing `probe_capture`) prints `changed <len> <sha256-hex>` on each foreign selection and sets the selection from stdin lines `set <text>`. Add a case to `test/runtime/test-labwc-runtime.sh`: `wl-copy` some text → probe prints it; probe sets text → `wl-paste` returns it; `yes | head -c 3000000 | wl-copy` → probe prints nothing and keeps answering. If `wl-clipboard` is missing in `test/runtime/Dockerfile`, add it there (runtime test image only, not the ThinFloe image).
- [ ] Step 3: run `cargo test -j 4` and the runtime case (iterate with `TF_RUNTIME_MENU=<host-built binary>`); clippy, fmt.
- [ ] Step 4: commit `feat(deskflow): Wayland clipboard bridge`.

### Task 3: Clipboard policy and daemon wiring

**Files:**
- Create: `src/thinfloe-menu/src/deskflow/clipsync.rs`
- Modify: `src/thinfloe-menu/src/deskflow/run.rs` (poll the Clip fd and pipe fds; route events)

**Interfaces:**
- Consumes: Task 1 Session API, Task 2 Clip API.
- Produces (pure, sans-IO):
  - `pub struct ClipSync`; `pub enum ClipAct { Send { conn: usize, text: Vec<u8> }, Grab { conn: usize }, SetLocal(Vec<u8>) }` (conn = the daemon's connection index).
  - `fn on_local(&mut self, text: Vec<u8>, active: Option<usize>, live: &[(usize, bool)]) -> Vec<ClipAct>` — ThinFloe's selection changed: Send to the active conn if its clipboard is on, Grab to every other live conn with clipboard on; remembers the text.
  - `fn on_enter(&mut self, conn: usize, clipboard: bool) -> Vec<ClipAct>` — Send the remembered text (none if ThinFloe never had text).
  - `fn on_remote(&mut self, conn: usize, text: Vec<u8>) -> Vec<ClipAct>` — SetLocal(text), remember it, and mark it so the `ClipEvent::Changed` that may follow with the same bytes is not sent back to `conn` (Clip already skips its own offers; this covers a second client re-offering the same text).
  - `fn on_grabbed(&mut self, conn: usize, live: &[(usize, bool)]) -> Vec<ClipAct>` — a device grabbed: Grab to every other live clipboard conn.
- [ ] Step 1: failing tests: `enter_sends_remembered_text`, `enter_without_text_sends_nothing`, `local_change_goes_to_active_and_grab_to_others`, `switch_off_device_gets_nothing`, `remote_text_sets_local`, `own_selection_is_not_sent_back`, `grab_from_one_device_reaches_the_others`.
- [ ] Step 2: implement ClipSync; wire run.rs: Clip connected at start (failure logged, clipboard off), its fds in the poll set, its deadline in the timeout; `Out::Clipboard`/`Out::Grabbed` and capture enter call ClipSync; `set_clipboard` called on every connection at reload with the device's `clipboard` flag. A Clip error drops the bridge (logged) and never ends the daemon.
- [ ] Step 3: `cargo test -j 4`; clippy, fmt.
- [ ] Step 4: commit `feat(deskflow): bridge the text clipboard to attached devices`.

### Task 4: Clipboard switch in the Deskflow band

**Files:**
- Modify: `src/thinfloe-menu/src/deskflowtab.rs`, `src/thinfloe-menu/src/deskflow_ui.rs`

**Interfaces:**
- Produces: in the selected device's detail (next to Position and Forget) a switch labelled `Share the clipboard`; toggling writes `clipboard` of that `[[deskflow.device]]` through the store and reloads the daemon like every `[deskflow]` write.
- [ ] Step 1: failing test in deskflowtab.rs: `clipboard_switch_writes_device` (toggle off → the saved config has `clipboard = false` for that device only).
- [ ] Step 2: implement; `ui_code_has_no_style_literals` stays green.
- [ ] Step 3: `cargo test -j 4`; clippy, fmt; commit `feat(menu): per-device clipboard switch for Deskflow`.

### Task 5: Clipboard interop test, notes, changelog

**Files:**
- Modify: `test/runtime/test-labwc-runtime.sh` (the Deskflow interop section), `test/runtime/Dockerfile` if a tool is missing, `docs/notes/deskflow.md`, `CHANGES.md`.

- [ ] Step 1: interop cases with the real `deskflow-core client`: (a) `wl-copy` text on ThinFloe, capture into the client → the client's X clipboard (`xclip -o -selection clipboard` or `xsel` on the client's display) holds the text; (b) set the client's clipboard while captured, leave (pointer back) → `wl-paste` on ThinFloe returns it; (c) the device with `clipboard = false` → after capture the client's clipboard is unchanged. If the client cannot be driven to own its clipboard in the harness, document why in the notes and keep (a) and (c).
- [ ] Step 2: `make test-runtime VERSION=deskflow BUILD_JOBS=4` green (iterate with `TF_RUNTIME_MENU`, final run on the make-built image).
- [ ] Step 3: notes: a Clipboard section (flow, cap, own-offer marker, what is not bridged); CHANGES.md entry: e.g. "Text copied on ThinFloe can now be pasted on a computer attached with Deskflow, and text copied there can be pasted on ThinFloe. Settings → Screen has a "Share the clipboard" switch per device."
- [ ] Step 4: commit `test(deskflow): clipboard interop; docs`.

## Step 3: Media keys follow the pointer

### Task 6: labwc patch 0007

**Files:**
- Create: `containers/ubuntu/patches/0007-layer-inhibitor-blocks-all-keybinds.patch` (DEP-3 header like 0004)
- Modify: the Dockerfile's patch list if patches are listed by name; `Makefile` `PATCH_FILES` if listed by name (check `test-container-inputs.sh`); `test/runtime/test-labwc-runtime.sh`.

**Interfaces:**
- Produces: a `zwp_keyboard_shortcuts_inhibitor_v1` on a layer surface counts (a server-wide counter or a list, decremented on destroy) and, while that layer surface has the seat's keyboard focus, `handle_keybinding` runs no binding at all, `allowWhenInhibited` ones included. Inhibitors on views behave as in 0004/0006.
- [ ] Step 1: runtime case (red on the current image): a probe client (`thinfloe-menu --deskflow-probe` gains `--inhibit`, or the existing capture probe) holds an overlay layer surface with exclusive keyboard + inhibitor; with `THINFLOE_VOLUME_KEYS=on` and `TF_OSD_KEY_CMD` pointing at a recorder, inject `XF86AudioRaiseVolume` → the recorder is not called and the probe sees the key; after the probe exits the key calls the recorder again. Also inject while an x11-probe window holds an X11 keyboard grab (Risk 1 part 2) and the strip took the focus → recorder not called. Existing case (`allowWhenInhibited` passes a toplevel inhibitor) stays green.
- [ ] Step 2: write the patch on labwc 0.9.3 with 0001–0006 applied; rebuild the container; runtime case green.
- [ ] Step 3: CLAUDE.md: patch list in "labwc is a local rebuild" and the keybinding constraint gain `0007`; commit `feat(labwc): a layer-surface inhibitor blocks every keybinding`.

### Task 7: Inhibitor at capture and the local mic-mute key

**Files:**
- Modify: `src/thinfloe-menu/src/deskflow/capture.rs` (bind `zwp_keyboard_shortcuts_inhibit_manager_v1`; create the inhibitor on the strip right after setting exclusive interactivity in `begin_capture`, destroy it in `end_capture`), `src/thinfloe-menu/src/deskflow/forward.rs` (new `Action::LocalMicMute` on the release of keysym `XF86AudioMicMute` 0x1008ffb2 while captured; the press and release are not sent), `run.rs` (runs `thinfloe-menu --osd-key mic-mute` detached, own process group, no wait, when `mic_mute_local` is true), `src/thinfloe-menu/src/deskflowcfg.rs` / `generated.rs` (the generated file gains `mic_mute_local = <volume-key switch>`; it is regenerated, and the daemon reloaded, when the volume-key switch changes; it is not part of `config.toml`'s `[deskflow]`).
- [ ] Step 1: failing unit tests: `mic_mute_release_is_local_and_not_sent` (forward), `generated_deskflow_carries_mic_mute_local` (generated.rs), `volume_switch_regenerates_deskflow`.
- [ ] Step 2: implement; interop case: while captured, inject `XF86AudioRaiseVolume` → the client sees the key (its log or the harness's key trace shows the down for KeyID 0xE0AF) and the ThinFloe recorder is not called; inject `XF86AudioMicMute` → recorder called with `mic-mute`, client sees nothing; after release (escape chord) the volume key reaches the recorder again.
- [ ] Step 3: `cargo test -j 4`, clippy, fmt, `make test-runtime` green; notes (Media keys section; replace "Media keys still act on ThinFloe in this step"), CLAUDE.md Deskflow rule line if needed, CHANGES.md entry ("While the pointer is on a computer attached with Deskflow, the volume and brightness keys act on that computer and ThinFloe shows no volume or brightness display. The microphone mute key still mutes ThinFloe.").
- [ ] Step 4: commit `feat(deskflow): media keys follow the pointer`.

### Task 8: Final gates

- [ ] `cargo test`, clippy, fmt; `make test-unit`; `make test-runtime`; `make test`; `test-image-size.sh` (record growth with reason if needed). Whole-branch review of steps 2–3, fix wave, then PR.
