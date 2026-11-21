# Deskflow Server, Step 1 (Server Core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ThinFloe's keyboard and pointer drive paired computers that run the stock Deskflow client, attached to a side of a ThinFloe monitor in Settings → Screen.

**Architecture:** A new mode `thinfloe-menu --deskflow` (module `src/thinfloe-menu/src/deskflow/`) runs one single-threaded `libc::poll` loop over the Wayland fd, the TCP listener, the TLS connections, a command socket and a signal pipe, plus a watchdog thread. Protocol, pairing, key mapping and edge logic are sans-IO modules with unit tests; the loop only moves bytes and Wayland events. The menu supervises the process, owns all UI and is the only writer of `config.toml`; the daemon reads `generated/deskflow` and writes a state file.

**Tech Stack:** Rust 1.96 (edition 2024), wayland-client 0.31, wayland-protocols(-wlr) client bindings, rustls (ring provider), rcgen, ring (SHA-256), xkbcommon-dl (already in Cargo.lock), shadcn-egui for the UI; bash runtime tests in headless labwc with `wlrctl`/`wtype` and Ubuntu's `deskflow` 1.26.0 package as the interop client.

**Spec:** [docs/superpowers/specs/2026-11-21-deskflow-server-design.md](../specs/2026-11-21-deskflow-server-design.md) — read it first. This plan covers its **Build order step 1** only. Clipboard (step 2) and media keys/patch `0007` (step 3) get their own plans; in step 1 media keys keep acting on ThinFloe and no clipboard message is sent or handled (incoming CCLP/DCLP are read and dropped).

## Global Constraints

- Builds and tests use at most 4 cores: `CARGO_BUILD_JOBS=4`, `cargo test -j 4 -- --test-threads=4`; any test that feeds network or adversarial input runs under `systemd-run --user --scope -p MemoryMax=2G --` (the Makefile's `$(MEMCAP)`).
- No `wgpu`, `glow`, `eframe` (menu draws on the CPU). New crates: `default-features = false`, only the features named in Task 4.
- `config.toml` is written only by the menu through `store.rs`; the daemon never opens it.
- UI colours from the theme, text sizes from `brand.rs` constants (`ui_code_has_no_style_literals` must stay green); shadcn-egui `lyra`, nothing rounded.
- No new labwc keybinding and no labwc patch in step 1.
- English for code, comments, keys; UI text in Simplified Technical English.
- Defaults pinned by the spec: listen `false`, port `24800`, edge_resistance `medium` (off 0 / low 15 / medium 40 / high 100 logical px), resistance reset after 300 ms without outward push, keep-alive 3 s with 3 misses, watchdog stall 2 s, TLS+Hello deadline 10 s, ≤ 4 simultaneous handshakes, ≤ 8 pending devices, message cap 1 MiB, client name cap 64 bytes, protocol minor 6–8 (send 8, Hello name `Barrier`), escape `Ctrl+Alt+Shift+Escape`, restart back-off 1, 2, 4 … 60 s.
- Identity: ECDSA P-256 self-signed, CN `ThinFloe <hostname>`, valid 2000-01-01 to 2099-12-31, `~/.config/thinfloe/deskflow/identity.pem` mode 0600; fingerprint `v2:sha256:<lowercase hex>` over the DER certificate.
- Every behaviour change gets a `CHANGES.md` line under `## [Unreleased]` (Task 13); no release headers.
- Commit after each task with a Conventional-Commit subject and the `Co-Authored-By` trailer.

## Review Focus

1. **The monitor the device is attached to is unplugged or powered off while captured** → capture ends at once, keys and pointer are back on ThinFloe (Task 10 test `output_gone_releases`).
2. **A scaled (1.5) or rotated monitor** → the strip covers exactly the logical edge and the entry point maps proportionally (Task 9 test `strip_on_scaled_and_rotated_output`).
3. **Keys held when capture ends** (including the escape chord itself) → every key sent down to the device is sent up before COUT; nothing stays pressed on the device (Task 10 test `release_sends_up_for_held_keys`).
4. **A hostile or broken client name** (control characters, 10 kB, invalid UTF-8, a TAB or newline) → stored capped and escaped; the state file stays one line per device (Task 6 test `state_line_escapes_hostile_names`, Task 5 test `hello_back_name_capped`).
5. **The daemon gets SIGTERM while captured** (switch off, menu exit) → pointer and keyboard return to ThinFloe before exit, the device gets CBYE (Task 10 runtime case `sigterm_while_captured`).

---

## File Structure

New, under `src/thinfloe-menu/src/deskflow/`:

| File | Responsibility |
|---|---|
| `mod.rs` | `DESKFLOW_ARG`, `deskflow_main`, paths from env |
| `proto.rs` | messages, encode/decode, 4-byte framing (pure) |
| `ident.rs` | identity load/create, fingerprint, random art (pure except file I/O) |
| `tls.rs` | rustls server config, accept-any client cert verifier |
| `pair.rs` | trust decision, pending list and its limits (pure) |
| `session.rs` | one connection's protocol state machine (sans-IO) |
| `statefile.rs` | state file write/parse (shared with the menu) |
| `cmd.rs` | command datagram encode/decode, socket bind/send |
| `keymap.rs` | keysym → KeyID, XKB mods → KeyModifierMask (pure) |
| `edge.rs` | strip geometry, entry mapping, tracked position, resistance (pure) |
| `capture.rs` | Wayland: outputs, strips, pointer lock, keyboard, relative motion |
| `run.rs` | poll loop, listener, connections, watchdog, signals |

Menu side: `deskflowcfg.rs` (schema conversions + generated file), `deskflowtab.rs` (tab state and actions), `deskflow_ui.rs` (band), `deskflow_sup.rs` (supervisor). Modified: `config.rs`, `generated.rs`, `store.rs`, `lib.rs`, `main.rs`, `app.rs`, `menu.rs`, `monitors_ui.rs`, `Cargo.toml`, `overlay/usr/bin/thinfloe-upgrade`, `test/unit/test_upgrade_image.py`, `test/runtime/Dockerfile`, `test/runtime/test-labwc-runtime.sh`, docs.

The `Makefile` needs no change: `MENU_FILES`/`MENU_DIRS` are a `find` over the crate.

---

### Task 1: Capture spike — strip, lock and keyboard over a full-screen grab (gate)

This is the spec's Risk 1. It builds the real Wayland part of `capture.rs`, not a throwaway.

**Files:**
- Create: `src/thinfloe-menu/src/deskflow/mod.rs`, `deskflow/capture.rs`, `deskflow/edge.rs` (only `pub struct Rect { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }` here; Task 9 adds the rest)
- Modify: `src/thinfloe-menu/src/lib.rs` (`pub mod deskflow;`), `src/thinfloe-menu/src/main.rs` (dispatch next to `osd::OSD_ARG`), `Cargo.toml` (`wayland-protocols = { version = "0.32", default-features = false, features = ["client", "unstable", "staging"] }` — use the version that matches `wayland-client 0.31`), `test/runtime/test-labwc-runtime.sh`

**Interfaces:**
- Produces: `pub const DESKFLOW_ARG: &str = "--deskflow"`; `pub fn deskflow_main(args: &[OsString]) -> ExitCode`; `--deskflow --probe-capture <output> <side>` test hook.
- Produces in `capture.rs`:
  - `pub struct Capture` with `connect() -> Result<Capture, String>`, `fd()`, `dispatch() -> Result<Vec<CapEvent>, String>`, `flush()`
  - `pub fn outputs(&self) -> Vec<OutputGeom>` (name, logical x, y, w, h from `zxdg_output_v1`)
  - `pub fn set_strips(&mut self, strips: &[StripSpec])` (`StripSpec { id: u32, output: String, rect: Rect }`)
  - `pub fn begin_capture(&mut self, strip: u32)`, `pub fn end_capture(&mut self)`
  - `pub enum CapEvent { Enter{strip:u32, x:f64, y:f64}, Leave{strip:u32}, Relative{dx:f64, dy:f64}, Locked, Unlocked, Key{code:u32, pressed:bool}, Modifiers{depressed:u32, latched:u32, locked:u32, group:u32}, Keymap{fd:OwnedFd, size:u32}, RepeatInfo{rate:i32, delay:i32}, Button{button:u32, pressed:bool}, Axis{v120_x:i32, v120_y:i32}, KeyboardLeft, OutputsChanged }`

- [ ] **Step 1: Write the runtime case** `run_capture_probe_case` in `test-labwc-runtime.sh`: start an X11 full-screen window with `XGrabKeyboard` (reuse `x11-probe.py`'s grab window), start `thinfloe-menu --deskflow --probe-capture HEADLESS-1 right-of` (prints one line per `CapEvent` on stdout; on `Enter` it calls `begin_capture` at once, on Escape key `end_capture`), `wlrctl pointer move` to the right edge of HEADLESS-1, `wtype x`, `wtype -k Escape`, `wtype y`. Assert: probe printed `enter`, `locked`, `key 45 down` (x) in that order; the X window got no `x`; after Escape the probe printed `unlocked` and the X window got `y` and is `state:active` again (`wlrctl toplevel find`).
- [ ] **Step 2: Run** `make test-runtime`. Expected: FAIL (no `--deskflow` mode).
- [ ] **Step 3: Implement `capture.rs` and the probe.** Strip = `zwlr_layer_surface_v1` on layer Overlay, anchored to the edge, 1 px deep, exclusive zone 0, interactivity None, a a transparent ARGB `wl_shm` buffer of the strip's size, input region = whole strip. `begin_capture` order (spec, pushback finding 1): (1) `zwp_pointer_constraints_v1.lock_pointer(strip, pointer, None, Oneshot)` and `zwp_relative_pointer_manager_v1.get_relative_pointer`, (2) `set_keyboard_interactivity(Exclusive)` + commit, (3) emit nothing until `locked` arrives. `end_capture`: destroy lock, interactivity None, commit. Relative motion uses `dx_unaccel`/`dy_unaccel`.
- [ ] **Step 4: Run** `make test-runtime`. Expected: the new case PASSes.
- [ ] **Step 5: Gate.** If `locked` never arrives or the key goes to the X window: **stop the plan** and report to the owner with the probe output (the spec's labwc-patch fall-back is a decision for them). Otherwise commit: `feat(deskflow): edge strip with pointer lock and keyboard capture`.

### Task 2: Protocol codec

**Files:** Create `deskflow/proto.rs` (tests inline, as the crate does).

**Interfaces:**
- Produces: `pub enum Msg` with variants for every message the spec's *Protocol subset* lists for step 1: `Hello{major:u16,minor:u16}`, `HelloBack{major,minor,name:Vec<u8>}`, `Qinf`, `Ciak`, `Calv`, `Cnop`, `Crop`, `Dsop(Vec<u32>)`, `Csec(u8)`, `Cinn{x:i16,y:i16,seq:u32,mods:u16}`, `Cout`, `Cbye`, `Dinf{x,y,w,h,warp,mx,my: i16}`, `KeyDown{id:u16,mods:u16,button:u16}`, `KeyRepeat{id,mods,count,button: u16}`, `KeyUp{id,mods,button: u16}`, `MouseDown(i8)`, `MouseUp(i8)`, `MouseMove{x:i16,y:i16}`, `Wheel{dx:i16,dy:i16}`, `Cclp{id:u8,seq:u32}`, `Dclp{id:u8,seq:u32,mark:u8,data:Vec<u8>}`, `Eicv{major,minor}`, `Ebsy`, `Eunk`, `Ebad`, `Unknown([u8;4])`
- `pub fn encode(m: &Msg, minor: u16) -> Vec<u8>` (payload only; KeyDown → `DKDL…%s` with empty lang at minor 8, `DKDN` at 6–7; KeyRepeat → 5-field with empty lang at 8, 4-field below)
- `pub fn decode(payload: &[u8]) -> Result<Msg, ProtoError>`; `pub fn decode_hello_back(payload:&[u8]) -> Result<Msg, ProtoError>` (Hello has no 4-letter code)
- `pub struct Framer` with `push(&mut self, bytes: &[u8])`, `next(&mut self) -> Result<Option<Vec<u8>>, ProtoError>`; `pub fn frame(payload: &[u8]) -> Vec<u8>`; `pub const MAX_MSG: usize = 1 << 20`.

- [ ] **Step 1: Write failing tests:** `hello_bytes` (`encode(Hello{1,8})` == `b"Barrier\x00\x01\x00\x08"`); `hello_back_parses_name` (`b"Barrier\0\x01\0\x08\0\0\0\x06laptop"` → name `laptop`); `frame_is_be_length` (`frame(b"CALV")` == `[0,0,0,4,b'C',b'A',b'L',b'V']`); `framer_splits_and_joins` (two frames pushed one byte at a time); `framer_refuses_oversize` (length `MAX_MSG+1` → Err); `key_down_v18_has_lang` (ends with `[0,0,0,0]` and code `DKDL`); `key_down_v16_is_dkdn`; `key_repeat_v18_five_fields` (payload len 4+8+4); `dinf_roundtrip` (1920×1080 → bytes `\x07\x80`); `cinn_layout`; `truncated_payload_is_error` for every fixed-size message; `unknown_code_kept`.
- [ ] **Step 2: Run** `cargo test -j 4 deskflow::proto -- --test-threads=4` in `src/thinfloe-menu`. Expected: FAIL (module missing).
- [ ] **Step 3: Implement** big-endian, strings as `u32` length + bytes capped by the frame.
- [ ] **Step 4: Run** the same command. Expected: PASS.
- [ ] **Step 5: Commit** `feat(deskflow): protocol messages and framing`.

### Task 3: Config schema and `generated/deskflow`

**Files:** Modify `config.rs` (`DeskflowCfg`, `DeviceCfg`, field `deskflow` in `ThinfloeConfig`), `generated.rs` (`Table::Deskflow` → `&["deskflow"]`, render arm), `store.rs` (rebuild-from-generated reads it back); create `deskflowcfg.rs`.

**Interfaces:**
- `pub struct DeskflowCfg { pub listen: bool, pub port: u16, pub edge_resistance: Resistance, pub devices: Vec<DeviceCfg> }` (TOML: `[deskflow]`, `[[deskflow.device]]` exactly as the spec's *Configuration* block; `device` is the TOML key of `devices`)
- `pub struct DeviceCfg { pub name: String, pub fingerprint: String, pub output: String, pub side: Side, pub clipboard: bool }` (`side` as `kanshi::Side` words; `clipboard` default true, unused in step 1)
- `pub enum Resistance { Off, Low, Medium, High }` with `pub fn px(self) -> f64` (0, 15, 40, 100)
- `deskflowcfg::render(&DeskflowCfg) -> String` (body of `generated/deskflow`: the same TOML table, after `generated::HEADER`) and `deskflowcfg::parse(text:&str) -> Result<DeskflowCfg,String>`; `deskflowcfg::validate(&DeskflowCfg) -> Result<(),String>` (fingerprint matches `^v2:sha256:[0-9a-f]{64}$`, name 1–64 bytes, unique names, at most one device per (output, side), port ≠ 0).
- The generated file is mode 0600 (`generated::mode_of`).

- [ ] **Step 1: Failing tests:** `default_is_off` (listen false, port 24800, Medium); `toml_roundtrip_matches_spec_block`; `render_parse_roundtrip`; `validate_refuses_bad_fingerprint`, `…_duplicate_edge`, `…_duplicate_name`; `generated_mode_0600`; in `store.rs` tests `broken_config_keeps_deskflow_devices` (break `config.toml`, reopen, devices back from `generated/deskflow`).
- [ ] **Step 2: Run** `cargo test -j 4 -- --test-threads=4 deskflow store`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** full `cargo test -j 4 -- --test-threads=4` (existing config/migrate tests must stay green). Expected: PASS.
- [ ] **Step 5: Commit** `feat(config): [deskflow] table and generated/deskflow`.

### Task 4: Identity, fingerprint, random art; TLS config

**Files:** Create `deskflow/ident.rs`, `deskflow/tls.rs`; `Cargo.toml`: `rustls` (features `ring`, `std`, `tls12`, `logging` off), `rcgen` (features `ring`, `pem`), `ring` (default off), `rustls-pki-types`. Add each with `cargo add --no-default-features` at its current stable version; `--locked` builds need the updated `Cargo.lock` committed.

**Interfaces:**
- `pub struct Identity { pub cert_der: Vec<u8>, pub key_der: Vec<u8> }`; `pub fn load_or_create(path:&Path, hostname:&str) -> Result<Identity,String>` (creates dir 0700, file 0600, atomically via `crate::atomic`)
- `pub fn fingerprint(der:&[u8]) -> String` → `v2:sha256:<hex>`; `pub fn colon_hex(fp:&str) -> String` (uppercase, colons); `pub fn random_art(fp:&str) -> Vec<String>` (Deskflow's `generateFingerprintArt` in `src/lib/net/SecureUtils.cpp` — port the algorithm, cite the Deskflow commit in a comment; 17 columns)
- `tls::server_config(id:&Identity) -> Arc<rustls::ServerConfig>`: TLS 1.2+1.3, client certificate **required**, a `ClientCertVerifier` that accepts any certificate (signature checks of the handshake still on) — trust is decided later by fingerprint; `tls::peer_fingerprint(conn:&rustls::ServerConnection) -> Option<String>`.

- [ ] **Step 1: Failing tests:** `create_then_load_same_fingerprint`, `identity_file_mode_0600`, `fingerprint_format` (regex), `fingerprint_known_vector` (SHA-256 of a fixed DER byte string), `colon_hex_shape` (95 chars, uppercase), `random_art_shape` (box 19 wide incl. borders, 9+2 lines, stable for a fixed fp), `tls_handshake_needs_client_cert` (in-memory rustls client without cert → server errors; with any self-signed cert → `peer_fingerprint` equals that cert's fingerprint).
- [ ] **Step 2: Run** `cargo test -j 4 deskflow::ident deskflow::tls -- --test-threads=4`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: PASS. Also `cargo clippy -j 4` clean.
- [ ] **Step 5: Commit** `feat(deskflow): TLS identity and fingerprints`.

### Task 5: Pairing and the session state machine

**Files:** Create `deskflow/pair.rs`, `deskflow/session.rs`.

**Interfaces:**
- `pair.rs`: `pub enum Verdict { Trusted(usize /* device index */), Pending, FingerprintChanged }`; `pub fn judge(devices:&[DeviceCfg], name:&str, fp:&str) -> Verdict`; `pub struct Pending` with `note(&mut self, name, fp, flags, now_epoch)` (keeps at most 8, drops the oldest), `reject(name, fp)`, `is_rejected(name, fp) -> bool`, `entries() -> &[PendingEntry]`, `forget_trusted(devices)`.
- `session.rs`: `pub struct Session`; `Session::new(now: Instant) -> (Session, Vec<Out>)` (sends Hello 1.8); `on_payload(&mut self, p:&[u8], now) -> Vec<Out>`; `tick(&mut self, now) -> Vec<Out>`; `set_verdict(&mut self, v: Verdict) -> Vec<Out>`; `enter(&mut self, x:i16, y:i16, mods:u16) -> Vec<Out>`, `leave(&mut self) -> Vec<Out>`, `send(&mut self, m: Msg) -> Vec<Out>`; `pub enum Out { Send(Vec<u8> /* framed */), Named{name:String}, Screen{w:i16,h:i16}, Close(&'static str) }`; `pub fn minor(&self) -> u16`.
- Rules: HelloBack major ≠ 1 or minor ∉ 6..=8 → EICV + Close; name capped to 64 bytes, bytes kept raw (escaped only for display); after `Trusted` → QINF, then on the first DINF → CIAK, CROP, DSOP(empty) + `Screen`, on later DINFs CIAK + `Screen`; `Pending`/`FingerprintChanged` → EUNK + Close; no HelloBack within 10 s of `new` → Close; CALV sent every 3 s, Close after 9 s without any message from the client; CALV from client is answered.

- [ ] **Step 1: Failing tests:** `judge_known_pending_changed`; `pending_keeps_eight_newest`; `rejected_not_noted_again`; `hello_first`; `old_minor_gets_eicv` (minor 5); `hello_back_name_capped` (10 kB name → 64 bytes); `trusted_gets_qinf_then_ciak_after_dinf`; `pending_gets_eunk_and_close`; `hello_deadline_10s`; `keepalive_every_3s_dead_after_9s`; `calv_is_answered`; `key_form_follows_minor` (minor 7 session encodes DKDN).
- [ ] **Step 2–4:** run `cargo test -j 4 deskflow:: -- --test-threads=4` red, implement, green.
- [ ] **Step 5: Commit** `feat(deskflow): pairing rules and protocol session`.

### Task 6: State file and command socket

**Files:** Create `deskflow/statefile.rs`, `deskflow/cmd.rs`.

**Interfaces:**
- `pub enum DevState { Pending, Rejected, Connected, Active, Offline }`; `pub struct DevLine { name: String, fingerprint: String, state: DevState, since: i64, width: u32, height: u32, flags: Vec<String> }`; `pub enum Head { SelfInfo{fingerprint:String, listen:String}, Error(String) }`; `pub struct StateFile { head: Head, devices: Vec<DevLine> }`
- `pub fn render(s:&StateFile) -> String`; `pub fn parse(text:&str) -> StateFile` (ignores unknown trailing fields and bad lines); `pub fn write(path:&Path, s:&StateFile) -> io::Result<()>` (atomic, 0600); line format exactly as the spec's *Processes* section; names escaped with `\t`, `\n`, `\\`, `\xNN` for other control bytes and invalid UTF-8.
- Path: `$TF_DESKFLOW_STATE`, else `$XDG_RUNTIME_DIR/thinfloe-deskflow.state`; socket `$TF_DESKFLOW_SOCKET`, else `$XDG_RUNTIME_DIR/thinfloe-deskflow.sock`.
- `pub enum Cmd { Reject{name:String, fingerprint:String}, Reload }`; `encode`/`decode` as one datagram `reject\t<escaped name>\t<fp>\n` / `reload\n`; `bind`/`send` follow `osd/socket.rs` (remove stale file, 0600, non-blocking).

- [ ] **Step 1: Failing tests:** `render_parse_roundtrip`, `state_line_escapes_hostile_names` (name with TAB, newline, `\x01`, invalid UTF-8 → one line, roundtrips), `parse_ignores_extra_fields`, `same_name_two_lines`, `error_head`, `cmd_roundtrip`, `cmd_garbage_is_none`.
- [ ] **Step 2–4:** red, implement, green.
- [ ] **Step 5: Commit** `feat(deskflow): state file and command socket`.

### Task 7: Daemon loop and the interop harness

**Files:** Create `deskflow/run.rs`; modify `deskflow/mod.rs`, `test/runtime/Dockerfile` (add `deskflow`; it is in Ubuntu 26.04 universe, 1.26.0), `test/runtime/test-labwc-runtime.sh`.

**Interfaces:**
- `pub fn run(env: RunEnv) -> Result<(), String>` with `RunEnv { generated: PathBuf /* generated/deskflow */, identity: PathBuf, state: PathBuf, socket: PathBuf, hostname: String }`.
- Loop: `libc::poll` with timeout ≤ 250 ms over capture fd, listener, each connection, command socket, signal self-pipe (SIGTERM, SIGHUP via `signal-hook`-free `libc::sigaction` writing to a pipe). Connections: non-blocking `TcpStream` + `rustls::ServerConnection` (`read_tls`/`process_new_packets`/`write_tls`), ≤ 4 in handshake (extra accepts are closed at once), handshake + HelloBack ≤ 10 s. After TLS, `pair::judge` with the peer fingerprint feeds `Session::set_verdict`. Two connections with the same trusted name: the newer gets EBSY.
- Reload (SIGHUP or `Cmd::Reload`): re-read `generated/deskflow`; close connections whose device is gone; if `listen` is false, exit 0; if the port changed, re-bind.
- Watchdog: a thread checks an `AtomicU64` heartbeat (ms since start) the loop stores each turn; stale > 2 s → `libc::_exit(70)`.
- Exit 0 when the Wayland connection fails (`Capture::dispatch` returns Err): labwc is gone, so is the session.
- State file rewritten on every change of any device line; first line `self` with the identity fingerprint and `0.0.0.0:<port>`; `Head::Error` when bind fails (e.g. `port 24800 in use`), then exit 1.

- [ ] **Step 1: Discover the client CLI** in the runtime image: `podman run --rm <runtime image> deskflow-core --help` (or `deskflow-client`); write down the exact flags for name, server address, TLS on, log level DEBUG2, config dir, in a comment at the top of the new runtime case. Find the client's `trusted-servers` path from `Settings::tlsTrustedServersDb()` for that config dir.
- [ ] **Step 2: Write the runtime case** `run_deskflow_pairing_case`: start the daemon with a `TF_CONFIG_DIR` whose `generated/deskflow` has `listen = true`, port 24800, no devices; read our fingerprint from the state file's first line; pre-fill the client's `trusted-servers` with it; start the client (name `laptop`, server `127.0.0.1`, under Xwayland of the same labwc). Assert, each within 15 s: state file has `laptop … pending`; append the device (fingerprint from that line, `output = "HEADLESS-1"`, `side = "right-of"`) to `generated/deskflow` and send `reload` over the socket; state becomes `connected` with width/height of the client's screen; the client log shows it connected. Then a second client with name `laptop` and a fresh config dir (other certificate) → line `laptop … pending … fp-changed` and the first stays `connected`. Then `listen = false` + reload → daemon exits 0, nothing listens on 24800 (`ss -ltn`). Wrap the client in `timeout`.
- [ ] **Step 3: Run** `make test-runtime`. Expected: FAIL.
- [ ] **Step 4: Implement** `run.rs`; `deskflow_main` builds `RunEnv` from `ConfigDir::from_env()`, `/etc/hostname` and the env paths of Task 6, and sets `PR_SET_PDEATHSIG` SIGTERM. If ECDSA is refused by the client (TLS alert in its log), switch `ident::load_or_create` to RSA-2048 (rcgen + ring cannot make RSA keys: use `rcgen` with the `aws_lc_rs` feature for that case only) and note it in the commit.
- [ ] **Step 5: Run** `make test-runtime`. Expected: PASS. Write in the commit body the facts the spec marked "verify": ECDSA accepted (yes/no), client retried after EUNK, Hello framed.
- [ ] **Step 6: Commit** `feat(deskflow): listening daemon with pairing`.

### Task 8: Key mapping

**Files:** Create `deskflow/keymap.rs`.

**Interfaces:**
- `pub fn key_id(keysym: u32, utf32: u32) -> Option<u16>`: Unicode (BMP) when the keysym yields a printable character; else the Deskflow `kKey…` value for function, navigation, editing, keypad and modifier keysyms, taken as numeric constants from Deskflow's `src/lib/deskflow/KeyTypes.h` (cite the commit hash in the table's comment); media keysyms map too (`kKeyAudioMute` 0xE0AD, `kKeyBrightnessDown` 0xE0B8, `kKeyBrightnessUp` 0xE0B9, the volume keys) for step 3, though labwc keeps them in step 1; `XF86AudioMicMute` → None.
- `pub fn mod_mask(shift:bool, ctrl:bool, alt:bool, logo:bool, altgr:bool, caps:bool, num:bool, scroll:bool) -> u16` with the `KeyModifierMask` bits from `KeyTypes.h`.
- `pub fn button(evdev: u32) -> u16` = evdev + 8.
- `pub struct Xkb` (via `xkbcommon-dl`; add it to `Cargo.toml` at the version already in `Cargo.lock`): `from_keymap_fd(fd, size) -> Result<Xkb,String>`, `update_mask(d,l,lk,g)`, `keysym(evdev) -> u32`, `utf32(evdev) -> u32`, `mods() -> Mods`.

- [ ] **Step 1: Failing tests:** `latin_letters_are_unicode` (keysym `a` → 0x61, `A` → 0x41), `umlaut` (`adiaeresis` → 0xE4), `return_escape_f1_match_keytypes` (values from `KeyTypes.h`), `mic_mute_is_none`, `mod_mask_bits`, `button_is_x11_keycode` (evdev 30 → 38), `xkb_de_layout_y_is_z` (build a keymap for `de` from names; evdev `KEY_Y` (21) → keysym `z`).
- [ ] **Step 2–4:** red, implement, green.
- [ ] **Step 5: Commit** `feat(deskflow): key ids and modifier mask`.

### Task 9: Edge geometry and edge resistance

**Files:** Create `deskflow/edge.rs`.

**Interfaces:**
- `Rect` from Task 1 (logical px)
- `pub fn strip_rect(out: Rect, side: Side) -> Rect` (1 px deep along that edge)
- `pub fn edge_free(out: Rect, side: Side, others: &[Rect]) -> bool` (false when any other output shares that edge segment)
- `pub fn entry_point(out: Rect, side: Side, at: (f64,f64), dev: (i16,i16)) -> (i16,i16)` (proportional along the edge, on the device's opposite edge: right-of → x = 0)
- `pub struct Tracker` with `new(side, start:(i16,i16), dev:(i16,i16))`, `motion(&mut self, dx:f64, dy:f64) -> Track`; `pub enum Track { Move(i16,i16), Back{along: f64 /* 0..1 */} }` (clamped inside the device; `Back` when the position crosses the attached edge back towards ThinFloe).
- `pub struct Resistance` with `new(threshold_px: f64)`, `push(&mut self, side, dx, dy, now: Instant) -> bool` (true when the outward sum reaches the threshold; threshold 0 → true on the first event, also with zero motion), `reset()`; resets itself when `now` is ≥ 300 ms after the last outward push; inward motion does not subtract.

- [ ] **Step 1: Failing tests:** `strip_rects_each_side`; `strip_on_scaled_and_rotated_output` (logical 1707×960 at x=1920 and a 1080×1920 portrait output: strip lengths equal the logical edge); `edge_between_two_monitors_not_free`; `entry_maps_proportionally` (y 360 of 720 → 540 of 1080); `tracker_clamps_and_comes_back`; `resistance_levels` (15/40/100 reached after exactly that outward sum); `resistance_ignores_inward_and_sideways`; `resistance_resets_after_300ms`; `resistance_off_fires_at_once`.
- [ ] **Step 2–4:** red, implement, green.
- [ ] **Step 5: Commit** `feat(deskflow): edge geometry and edge resistance`.

### Task 10: Forwarding and the way back

**Files:** Modify `deskflow/run.rs`, `deskflow/capture.rs`; `test-labwc-runtime.sh`.

**Interfaces:** Consumes Tasks 1, 5, 8, 9. Produces the captured state in `run.rs`: `struct Active { device: usize, tracker: Tracker, held: Vec<(u16 /*id*/, u16 /*button*/)> }`.

Behaviour:
- Strips: one per connected device whose output exists and whose edge is `edge_free`; recomputed on `OutputsChanged`, connect, disconnect, reload. A device without a strip shows `flags` `edge-busy` or state `offline` in the state file.
- `Enter` + `Relative` events feed `Resistance`; when it fires → `begin_capture`; on `Locked` → CINN at `entry_point`, state `active`.
- Captured: Relative → `Tracker::motion` → DMMV, or on `Back` → release; Button → DMDN/DMUP with Deskflow's button ids from `src/lib/deskflow/MouseTypes.h` (the protocol header's comment and the code disagree on which of 2 and 3 is right; the header file wins, and the interop case clicks both); Axis → DMWM with v120 values (positive up/right as Deskflow); Key → DKDN/DKRP/DKUP via `keymap`, with client-side repeat generated from `RepeatInfo`; keys already down at capture start are not sent.
- Release (one function `release(reason: &str)`): DKUP for every `held` key, DMUP for held buttons, COUT, `end_capture`, log `thinfloe-deskflow: released (<reason>)` to stderr. Triggers: `Back`; Escape pressed while Ctrl, Alt, Shift are down (not forwarded); session Close or socket error; `Unlocked`; `KeyboardLeft`; the strip's output gone; SIGTERM (then CBYE to all, exit 0).

- [ ] **Step 1: Write runtime cases** on top of Task 7's pairing setup (`edge_resistance = "low"`): `pointer_reaches_client` (`wlrctl pointer move` to the right edge, then several `wlrctl pointer move 10 0`; client DEBUG log shows "entering screen" and mouse moves; `wtype x` shows a key event in the client log and not in a local foot window); `touch_without_push_stays` (one move to the edge only: no enter in the client log); `escape_chord_releases`; `client_killed_releases` (kill the client while captured → `wtype y` reaches the local window within 1 s); `output_gone_releases` (`wlr-randr --output HEADLESS-1 --off` while captured); `sigterm_while_captured` (SIGTERM → local keys work, client log shows a clean disconnect).
- [ ] **Step 2: Unit test** `release_sends_up_for_held_keys` in `run.rs` (factor the captured-state logic into a pure `fn on_cap_event(&mut Active, &CapEvent, …) -> Vec<Action>` so it is testable without Wayland): press a, Ctrl, Alt, Shift, Escape → the output has DKDN a, DKDN Ctrl/Alt/Shift, then DKUP for all four, then COUT, and no Escape.
- [ ] **Step 3: Run** `make test-runtime` and `cargo test -j 4 -- --test-threads=4`. Expected: FAIL.
- [ ] **Step 4: Implement.**
- [ ] **Step 5: Run both.** Expected: PASS.
- [ ] **Step 6: Commit** `feat(deskflow): forward keyboard and pointer, release on every way back`.

### Task 11: Menu supervisor and lifecycle

**Files:** Create `deskflow_sup.rs`; modify `app.rs` (start/stop with the config, SIGTERM before exit 0/10/11), `test-labwc-runtime.sh`.

**Interfaces:**
- `pub struct Supervisor`: `new(program: PathBuf) -> Supervisor`; `want(&mut self, listen: bool, now: Instant)`; `tick(&mut self, now: Instant) -> Option<Instant /* next wake */>`; `reload(&self)` (SIGHUP); `stop(&mut self, wait: Duration)` (SIGTERM to the process group, wait ≤ 1 s, then SIGKILL); `pub fn last_failure(&self) -> Option<String>`.
- Spawn: `process_group(0)`, stdout/stderr to the session log the menu's other children use, `--deskflow`. Exit status ≠ 0 → restart after 1, 2, 4 … 60 s; a run longer than 60 s resets the back-off. Exit 0 with listen still wanted is also restarted.
- The menu calls `reload()` after each successful `store.update(Table::Deskflow, …)`.

- [ ] **Step 1: Failing unit tests** (with `/bin/sh -c` stand-ins as `program`): `backoff_doubles_to_60`, `long_run_resets_backoff`, `listen_off_stops`, `stop_kills_group`.
- [ ] **Step 2: Runtime case** `deskflow_lifecycle`: menu with `listen = true` → a `--deskflow` process and a listener on 24800; menu exits 0 → both gone within 2 s; start again, `kill -9` the menu → daemon gone within 2 s (PDEATHSIG); start again, `kill -9` the daemon → a new one within 2 s.
- [ ] **Step 3–4:** red, implement, green (`cargo test` and `make test-runtime`).
- [ ] **Step 5: Commit** `feat(menu): run and supervise the Deskflow daemon`.

### Task 12: Screen band, pairing UI and devices in the arrangement

**Files:** Create `deskflowtab.rs`, `deskflow_ui.rs`; modify `menu.rs` (state + events), `monitors_ui.rs` (draw devices), the Screen tab's drawing code (the place that draws `SettingsTab::Screen` today), `brand.rs` if a new text size is needed.

**Interfaces:**
- `pub struct DeskflowTab { pub cfg: DeskflowCfg, pub state: StateFile, pub attaching: Option<(String /*name*/, String /*fp*/)>, pub error: Option<String> }`
- `pub enum DeskflowEvent { Listen(bool), Resistance(Resistance), Attach{name,fp}, Place{name, output:String, side:Side}, Reject{name,fp}, Forget{name}, CancelAttach }`; `pub fn apply(tab:&mut DeskflowTab, ev: DeskflowEvent) -> Option<DeskflowRequest>`; `pub enum DeskflowRequest { Save(DeskflowCfg), Send(Cmd) }` (the menu turns `Save` into `store.update(Table::Deskflow, …)` + `Supervisor::reload`, `Send` into `cmd::send`).
- `pub fn free_edges(profile:&Profile, heads) -> Vec<(String /*criteria*/, Side)>` minus edges taken by other devices — what the Position control offers.
- `deskflow_ui::band(ui:&mut Ui, t:&Theme, tab:&DeskflowTab) -> Option<DeskflowEvent>`.
- The menu re-reads the state file when its mtime changes, at the cadence it reads the battery file.

UI copy (exact):
- switch: "Let other computers use this keyboard and mouse (Deskflow)"
- line: "This computer: <addr>:<port>" and its fingerprint (random art + colon hex), with the note "Check this fingerprint on the other computer before you trust it there."
- select: "Edge resistance" with "Off", "Low", "Medium", "High"
- pending row: "<name> wants to attach" + art + hex; buttons "Attach…", "Reject"; with `fp-changed`: "<name> has a new fingerprint. Attach it again only if you expect this."
- attached device: rectangle beside its monitor in the arrangement, scaled from width/height (a default 16:9 box when 0), status dot in theme colours (connected, active, offline); selected: "Position", "Forget"; inactive: "This edge is not free in the current arrangement."
- daemon failure: "Deskflow stopped: <reason>"

- [ ] **Step 1: Failing tests** in `deskflowtab.rs`: `attach_then_place_saves_device` (fingerprint from the pending line, output/side from Place), `reject_sends_cmd`, `forget_removes`, `free_edges_excludes_shared_and_taken`, `listen_toggle_saves`; `ui_code_has_no_style_literals` covers the new UI files (check the test's file list includes them).
- [ ] **Step 2–4:** red, implement, green; then `make test-runtime` (menu still maps and runs).
- [ ] **Step 5: Owner look:** a screenshot of Settings → Screen with one pending and one attached device (headless run, `grim`), attached to the task's report.
- [ ] **Step 6: Commit** `feat(menu): Deskflow band and devices in the monitor arrangement`.

### Task 13: Identity exclusion, notes, changelog

**Files:** Modify `overlay/usr/bin/thinfloe-upgrade` (`IDENTITY_EXCLUDES` += `"./.config/thinfloe/deskflow"`), `test/unit/test_upgrade_image.py`, `CLAUDE.md` (fresh-install rule; *Where code lives*; a *Menu* rule for the state file contract; the notes list), create `docs/notes/deskflow.md`, modify `docs/notes/home-persistence.md`, `CHANGES.md`.

- [ ] **Step 1: Failing test** `test_fresh_install_skips_deskflow_identity` in `test_upgrade_image.py` (next to the ssh-host-keys case: `--exclude=./.config/thinfloe/deskflow` in argv, and the file absent from the copied home).
- [ ] **Step 2: Run** `python3 -m unittest test.unit.test_upgrade_image -v`. Expected: FAIL. Implement. Expected: PASS.
- [ ] **Step 3: Write** `docs/notes/deskflow.md` (how pairing, capture, resistance and the way back work; the state file and socket contract; the facts Task 7 verified; known limits: no clipboard yet, media keys on ThinFloe until step 3, clients without a certificate, the first-contact order) and the CLAUDE.md lines.
- [ ] **Step 4: CHANGES.md** under `### New` (user's view, ≤ 3 sentences): Settings → Screen has a switch "Let other computers use this keyboard and mouse (Deskflow)"; computers running the Deskflow client then show up there, and after you compare the fingerprint and attach one beside a monitor, pushing the pointer past that edge moves keyboard and mouse to it. Ctrl+Alt+Shift+Escape always brings them back.
- [ ] **Step 5: Full gates:** `make test-unit`, `make test`, `make test-runtime`, `cargo clippy -j 4`, `cargo fmt --check` in the crate; `test-image-size.sh` green (raise the budget only with a reason in the commit).
- [ ] **Step 6: Commit** `docs(deskflow): notes, fresh-install exclusion, changelog`.

### Task 14: Owner hardware check (not automatable)

- [ ] A real stick, a Linux or macOS laptop with Deskflow 1.26: pair, attach right-of, type with `de` on both sides, push through the edge at each resistance level, use the escape chord; repeat inside a full-screen ThinLinc session. Record the result in `docs/notes/deskflow.md`.
