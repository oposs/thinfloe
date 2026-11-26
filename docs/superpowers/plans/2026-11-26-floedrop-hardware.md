# FloeDrop hardware fixes (Branch A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the FloeDrop bugs from the 2026-11-26 hardware test on thinfloe-2b3967 (4.13.0, TP-Link RTL8812BU stick) and the card-choice follow-ups of PR #62 (items A1 to A11).

**Architecture:** The root helper (`src/thinfloe-share/src/radio.rs`, `card.rs`) gets the card order with a probe per card, the country fallback, the real link channel and a wedge watch on owlyd's STATS journal lines; the daemon (`daemon.rs`, `state.rs`, new `incoming.rs`) writes `receiving` lines and log lines, withdraws its mDNS service before the radio goes and keeps its service id per session; the menu (`share_state.rs`, `network_ui.rs`) shows the new lines. No contract changes format: the up/ready line keeps its seven fields, new facts travel as a flag (`no-country`) and as receiver lines with trailing fields.

**Tech Stack:** Rust 2024 (thinfloe-share: tokio, zbus 5, owly-airdrop/owlyd at rev 21171d8; thinfloe-menu: egui, shadcn-egui), shell integration tests, `iw` and `journalctl` in the image.

**Spec:** `docs/superpowers/plans/2026-11-26-round-fix-everything.md`, section "Branch A: fix/floedrop-hardware" (rows A1 to A11). Background: `docs/notes/floedrop.md`, PR #62 body ("fix/floedrop-card-choice" follow-ups), Owly source at `~/.cargo/git/checkouts/owly-e8e12b3d697e0d99/21171d8`.

## Deviations from the round spec

- **A1:** owlyd's `Link` property carries no channel (`crates/owlyd/src/api.rs:84-137`: interface, index, mac, address, announce). The channel is read from `iw dev` on the station's phy after `Acquire` returns. When it cannot be read the channel is 0 and the texts leave ", channel N" out, instead of showing the probe's channel, which is the wrong one (6 before the country applies).
- **A2:** Owly reports no transfer progress (`ReceiverEvent` at `crates/owly-airdrop/src/driver/receiver.rs:77-99` has only Request, Withdrawn, Urls, Received, Failed, Discovered), so the band shows start and end only. The `receiving` line is a receiver line of its own (sender name, inbox folder, `receiving`, since, then `files<TAB>bytes<TAB>first`), not a state change of an outbox line: the sending phone need not be in the outbox list (no Bluetooth, not yet discovered), and the outbox thread owns those lines.
- **A3:** The band text names "Settings → Time", but the Time band is inside Settings → Network, right below FloeDrop (`network_ui.rs:84-90`). Text used: "No country set: set the time zone in the Time band below for faster FloeDrop."
- **A4:** owlyd has no STATS over D-Bus; they exist only as owlyd log lines every 10 s (`crates/owly-awdl/src/driver.rs:378-401`). The root helper follows `journalctl -u owlyd.service`. For a card that is not on USB the text says "The Wi-Fi card <card> stopped sending. Restart the machine." (unplugging an internal card is not possible); USB keeps the spec text.
- **A6:** When the probe refuses every card, the error is the probe's reason for the first card in preference order (Owly's own sentence), with no Acquire attempted.

## Global Constraints

- Work in the worktree `/scratch/oetiker/claude-worktrees/thinfloe-floedrop-hardware` on branch `fix/floedrop-hardware`, made from `round/2026-11-26b` (Task 0). All paths below are relative to it.
- At most 4 cores: every cargo call has `CARGO_BUILD_JOBS=4 -j 4 -- --test-threads=4` and runs under `flock /scratch/oetiker/claude-tmp/thinfloe-cargo.lock systemd-run --user --scope -q -p MemoryMax=6G --`; image targets under `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock` with `VERSION=floedrop-hw`. Never `/tmp`: `TMPDIR=$PWD/.tmp` (the Makefile default).
- Shorthands used in the steps:
  - `SHARE_TEST <filter>` = `cd src/thinfloe-share && mkdir -p ../../.tmp && TMPDIR=$PWD/../../.tmp flock /scratch/oetiker/claude-tmp/thinfloe-cargo.lock systemd-run --user --scope -q -p MemoryMax=6G -- env CARGO_BUILD_JOBS=4 cargo test --locked -j 4 <filter> -- --test-threads=4`
  - `MENU_TEST <filter>` = the same in `src/thinfloe-menu`.
  - `make test-share` / `make test-menu` = fmt, clippy `-D warnings` and all tests of the crate (run them under the cargo flock).
- thinfloe-share is a binary crate: an unused `pub fn` is a clippy error. Rename, do not keep dead wrappers.
- Exact texts (verbatim from the spec, with the deviations above):
  - A4 USB: "The Wi-Fi stick <card> stopped sending. Unplug it and plug it in again." (<card> is the station interface, e.g. `wlxc03a55f0d060`)
  - A4 other bus: "The Wi-Fi card <card> stopped sending. Restart the machine."
  - A3: "No country set: set the time zone in the Time band below for faster FloeDrop."
  - A2 band: "Receiving <file> from <sender>" for one file, "Receiving <n> files from <sender>" for more.
  - A7: "The Wi-Fi card <iface> (<model>) cannot run AirDrop; the USB stick <stick> is used for the network."
- Contracts (CLAUDE.md, FloeDrop): the helper's `up` line and the state file's `ready` line keep exactly seven fields; readers ignore unknown trailing fields; the daemon reads only `generated/share`; the radio helper never calls `AcquireDetached`; SIGHUP goes to the daemon pid only.
- No `exit`, initramfs or Makefile prerequisite change is needed: `SHARE_FILES` already covers every file under `src/thinfloe-share` (`Makefile:215-217`), including a new `src/incoming.rs` / `src/health.rs`.
- UI code: colours from the theme, sizes from `brand.rs` (`ui_code_has_no_style_literals` guards it).
- Every FloeDrop toggle saves the home: accepted, no change.

## Review Focus

1. A keyboard layout list such as `ch,de` or a variant-only line in `generated/environment`: the first layout decides; a missing or unreadable file gives no country, never an error (Task 4 test `the_first_layout_of_a_list_decides`).
2. owlyd restarts while the radio is up (D-Bus activation again): its STATS counters start from 0, which must not count as a stuck interval or as a wedge (Task 5 test `counters_that_go_back_start_again`).
3. A sender whose name holds a tab or newline, and a first file name with a tab: the `receiving` line must stay one line of seven fields (Task 6 test `a_receiving_line_with_tabs_stays_one_line`).
4. Two phones sending at the same time, or one phone sending two batches: two `receiving` lines, each removed by its own end event (Task 6 test `two_transfers_have_two_lines`).
5. `iw dev` output with two phys (internal card and stick) where both have an interface with a channel: only the station's phy counts (Task 3 test `the_channel_is_read_from_the_stations_phy_only`).

---

### Task 0: Branch and worktree

**Files:** none.

- [ ] **Step 1: Make the worktree**

Run: `git -C /scratch/oetiker/claude-worktrees/thinfloe-round-2026-11-26b worktree add /scratch/oetiker/claude-worktrees/thinfloe-floedrop-hardware -b fix/floedrop-hardware round/2026-11-26b`
Expected: "Preparing worktree (new branch 'fix/floedrop-hardware')". Copy the spec and this plan into `docs/superpowers/plans/` of the new worktree if they are not committed on the round branch yet.

- [ ] **Step 2: Baseline**

Run: `make test-share` and `make test-menu` (under the cargo flock).
Expected: both green; note the test counts for the gates task.

---

### Task 1: Texts: 7260 skip list, the used-stick case, the start failure, doc comments (A7, A8, A9, A11)

**Files:**
- Modify: `src/thinfloe-share/src/card.rs:8-15` (`AWDL_BROKEN`), `:45-50` (`broken_text`), `:113-158` (`choose_station`)
- Modify: `src/thinfloe-share/src/daemon.rs:116-125` (`ready_text` neighbour), `:491` (start failure), doc comments at `:1-9, 52-53, 82, 127, 154, 174, 296, 610, 709-710, 813, 987, 1169`
- Test: the `tests` modules of `card.rs` and `daemon.rs`

**Interfaces:**
- Produces: `pub fn broken_text(iface: &str, model: &str, used_stick: Option<&str>) -> String` (card.rs); `pub fn card_label(iface: &str, station: &str, driver: &str) -> String` (daemon.rs: "wlx1 (rtw88_8822bu)", "wlx1", or the iface when station is empty); `pub fn start_failed_text(iface: &str, station: &str, driver: &str, why: &str) -> String` = "FloeDrop cannot start on <card_label>: <why>". Task 3 uses `card_label` in `ready_text`.

- [ ] **Step 1: Write the failing tests**

In card.rs tests:
- `the_intel_7260_is_skipped_too`: for dev in `[0x08b1, 0x08b2]`, `inputs(&["wlp3s0"], None)` with `pci(0x8086, dev)` gives `Err("The Wi-Fi card wlp3s0 (Intel Wireless 7260) cannot run AirDrop; plug in a USB Wi-Fi stick that can.")`; beside a free `usb()` card `wlx00e04c123456` the stick is taken.
- `a_3160_beside_a_stick_used_for_the_network_names_the_stick`: `inputs(&["wlp2s0", "wlp3s0"], None)` with `wlp2s0` = `usb()` (connected in the fixture) and `wlp3s0` = `pci(0x8086, 0x08b3)` gives `Err("The Wi-Fi card wlp3s0 (Intel Wireless 3160) cannot run AirDrop; the USB stick wlp2s0 is used for the network.")`.

In daemon.rs tests:
- `the_start_failure_names_the_card`: `start_failed_text("awdl0", "wlxc03a55f0d060", "rtw88_8822bu", "mDNS: x")` == "FloeDrop cannot start on wlxc03a55f0d060 (rtw88_8822bu): mDNS: x"; with station "wlx1" and driver "" == "FloeDrop cannot start on wlx1: mDNS: x"; with station "" == "FloeDrop cannot start on awdl0: mDNS: x".

- [ ] **Step 2: Run them to verify they fail**

Run: `SHARE_TEST card::tests` and `SHARE_TEST the_start_failure_names_the_card`
Expected: FAIL (compile error: `start_failed_text` not found; wrong text for 08b1).

- [ ] **Step 3: Implement**

- `AWDL_BROKEN`: add `(0x8086, 0x08b1, "Intel Wireless 7260")` and `(0x8086, 0x08b2, "Intel Wireless 7260")`; the doc comment says: same firmware family as the 3160, skipped by family, not tested on hardware.
- `broken_text` gains `used_stick`; `choose_station` remembers the first USB card it skipped as in use and passes it.
- daemon.rs line 491: `Err(e) => break 'main Exit::Error(start_failed_text(&iface, &station, &driver, &e))`.
- Replace every literal backslash-backtick in daemon.rs comments by a plain backtick.

- [ ] **Step 4: Run tests and the backtick check**

Run: `SHARE_TEST` (all) and `grep -cP '\\\x60' src/thinfloe-share/src/daemon.rs` (PCRE: a backslash, then a backtick)
Expected: all PASS; the grep prints `0`.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-share/src/card.rs src/thinfloe-share/src/daemon.rs
git commit -m "fix(floedrop): skip the Intel 7260, name the used stick and the failed card"
```

---

### Task 2: Try the cards in order until the probe accepts one (A6)

**Files:**
- Modify: `src/thinfloe-share/src/card.rs:113-158` (`choose_station` becomes `candidates`), its tests `:219-366`
- Modify: `src/thinfloe-share/src/radio.rs:29-41` (`RadioApi`), `:141-213` (`hold`), `:256-264` (`Owlyd::mode_channel`), tests `:587-615, 684-756`

**Interfaces:**
- Produces (card.rs): `pub fn candidates(i: &CardInputs) -> Result<Vec<String>, String>`: every free, not-broken card, USB first, then sorted name; the `Err` texts are those `choose_station` gave (NO_STATE, NO_CARD, `broken_text`).
- Produces (radio.rs): `pub struct Probe { pub mode: String, pub refusal: Option<String> }`; `pub fn parse_probe(json: &str) -> Result<Probe, String>` (mode = `v["mode"]` or "unknown"; refusal = `v["missing"][0]["say"]` when `missing` is a non-empty array; bad JSON: `Err("owlyd's probe: <e>")`). `RadioApi::mode_channel` is replaced by `async fn probe(&self) -> Result<Probe, String>`. The channel no longer comes from the probe (Task 3).

- [ ] **Step 1: Write the failing tests**

- card.rs `the_candidates_come_in_order_of_preference`: inputs `["wlp3s0", "wlx00e04c123456", "wlx00e04c654321"]` with `wlp3s0` = `pci(0x8086, 0x2723)`, both wlx = `usb()` → `["wlx00e04c123456", "wlx00e04c654321", "wlp3s0"]`; a 3160 among them is left out.
- The existing card tests call `candidates(&i).unwrap()[0]` where they called `choose_station(&i).unwrap()`, and `candidates(&i)` where they compared the `Err`.
- radio.rs `probe_json_parses`: `{"mode":"exclusive","channel":6,"missing":[]}` → mode "exclusive", refusal None; `{"mode":"exclusive","missing":[{"id":"hardware","say":"Owly hasn't been tested on this computer's Wi-Fi (rtl8xxxu)."}]}` → refusal `Some("Owly hasn't been tested on this computer's Wi-Fi (rtl8xxxu).")`; `"x"` → Err starting "owlyd's probe: ".
- radio.rs: the `Fake` gets `refuse: Vec<String>` (stations its probe refuses) and reads the station from the radio.toml path it is handed (`Fake { toml: PathBuf, .. }`), logging `"probe <station>"`. Test `a_refused_stick_falls_back_to_the_next_card`: cards `wlx00e04c123456` (usb) and `wlp3s0` (pci 8086:2723, not in the fixture's route list: use `free_card()` with `wifi_ifaces = ["wlx00e04c123456", "wlp3s0"]`), the stick refused: calls `["probe wlx00e04c123456", "probe wlp3s0", "acquire", .., "release 5"]`, the up line names `wlp3s0`, radio.toml holds `station = "wlp3s0"`.
- `all_refused_reports_the_first_cards_reason`: both refused with reasons "A" and "B": exit 1, output `"error	A
"`, no "acquire" call.
- Update `a_stale_detached_lease_is_released_first` to expect `["release 3", "probe wlx00e04c123456", "acquire", ...]` (the rest as Task 3 leaves it).

- [ ] **Step 2: Run them to verify they fail**

Run: `SHARE_TEST radio::tests` and `SHARE_TEST card::tests`
Expected: compile errors (`candidates`, `parse_probe`, `probe` not found).

- [ ] **Step 3: Implement**

`hold`: `candidates(card)` (error → error line, exit 1, no call); release a stale detached lease; then for each candidate: `write_atomic(radio_toml_path, &radio_toml(station, country))`, `api.probe()`; the first with `refusal == None` is the station; a refusal is logged on stderr as `thinfloe-share: Owly refuses <station>: <refusal>` and the first one is kept. None accepted: the first refusal as the error line, exit 1. A probe call error stays an error line, exit 1. `Owlyd::probe` = `parse_probe(&self.radio.probe().await...)`.

- [ ] **Step 4: Run tests**

Run: `make test-share`
Expected: PASS, no clippy warning.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-share/src/card.rs src/thinfloe-share/src/radio.rs
git commit -m "fix(floedrop): fall back to the next card when Owly's probe refuses one"
```

---

### Task 3: Report the channel the link came up on (A1)

**Files:**
- Modify: `src/thinfloe-share/src/radio.rs` (`RadioApi`, `hold` after acquire, `Owlyd` impl, tests)
- Modify: `src/thinfloe-share/src/daemon.rs:116-125` (`ready_text`), test `the_ready_log_names_the_card` `:1238-1253`
- Modify: `src/thinfloe-menu/src/share_state.rs:115-146` (`status_lines`), its tests
- Modify: `test/integration/test-floedrop-image.sh:40-55` (`iw` must be in the image)

**Interfaces:**
- Consumes: `card_label` (Task 1).
- Produces: `pub fn iw_channel(iw_dev: &str, phy_index: &str) -> Option<u32>` (radio.rs): in the `phy#<phy_index>` section of `iw dev` output, the number of the first `channel N (` line of any interface; other phys are ignored. `RadioApi` gains `async fn link_channel(&self, station: &str) -> Option<u32>` (Owlyd: `/sys/class/net/<station>/phy80211/index`, then `iw dev`). `Up.channel` = that or 0. Channel 0 means unknown in every text.

- [ ] **Step 1: Write the failing tests**

- radio.rs `the_channel_is_read_from_the_stations_phy_only`: fixture text with `phy#0` (Interface wlp2s0, type managed, `channel 1 (2412 MHz), width: 20 MHz`) and `phy#1` (Interface owlyibss0, type IBSS, `channel 44 (5220 MHz), width: 20 MHz (no HT), center1: 5220 MHz`; Interface wlxc03a55f0d060, type managed, no channel line): `iw_channel(text, "1") == Some(44)`, `iw_channel(text, "0") == Some(1)`, `iw_channel(text, "2") == None`, `iw_channel("", "1") == None`.
- radio.rs: the `Fake` returns `link_channel` from a field `channel: Option<u32>` and logs `"channel <station>"`. Test `the_up_line_has_the_links_channel`: with `channel: Some(44)` the up line is `up	awdl0	exclusive	44	no-ble	wlx00e04c123456	`; with `None` the channel field is `0`. The existing tests that expected 44 from the probe set `channel: Some(44)`.
- daemon.rs `the_ready_log_names_the_card` adds: `ready_text("awdl0", "wlx1", "rtw88_8822bu", "exclusive", 0) == "ready on wlx1 (rtw88_8822bu, exclusive)"`.
- share_state.rs `an_unknown_channel_is_left_out`: `status_lines` of `ready_on("wlx1", "rtw88_8822bu")` with channel 0 gives "Ready on wlx1 (rtw88_8822bu, exclusive). Phones must be set to AirDrop 'Everyone for 10 Minutes'."

- [ ] **Step 2: Run them to verify they fail**

Run: `SHARE_TEST`, `MENU_TEST share_state`
Expected: FAIL (missing functions; channel text).

- [ ] **Step 3: Implement**

`hold`: after `acquire` and the BLE future, `let channel = api.link_channel(&station).await.unwrap_or(0);` `ready_text` and `status_lines` drop ", channel N" for 0. In `test-floedrop-image.sh` add `usr/sbin/iw` to `FILES` and to the executable check, with a comment that the radio helper reads the link channel with it.

- [ ] **Step 4: Run tests**

Run: `make test-share` and `make test-menu`
Expected: PASS. `test-floedrop-image.sh` runs in the gates (needs the image).

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-share src/thinfloe-menu/src/share_state.rs test/integration/test-floedrop-image.sh
git commit -m "fix(floedrop): report the channel the radio runs on, not the probe's"
```

---

### Task 4: Country from the keyboard layout, warning when there is none (A3)

**Files:**
- Modify: `src/thinfloe-share/src/card.rs:160-175` (beside `country`), tests `:368-378`
- Modify: `src/thinfloe-share/src/radio.rs:410-436` (`zone`, `gather`), `hold` (flags)
- Modify: `src/thinfloe-menu/src/share_state.rs:115-146`, tests

**Interfaces:**
- Produces (card.rs): `pub fn layout_country(layout: &str, zone1970: &str) -> Option<String>`: first entry of a comma list, trimmed; two ASCII lowercase letters, not in `NO_COUNTRY_LAYOUTS: &[&str] = &["us", "eu"]`, upper-cased, and only if that code appears in the first column of `zone1970` (comma lists split); anything else None (`latam`, `ara`, `epo`). `pub fn choose_country(zone: Option<&str>, layout: Option<&str>, zone1970: &str) -> Option<String>`: the zone's country, else the layout's.
- Produces (radio.rs): `pub fn env_layout(text: &str) -> Option<String>`: the value of the last `XKB_DEFAULT_LAYOUT=` line, None when empty. `gather` reads `<tfuser home>/.config/thinfloe/generated/environment` (the file `generated.rs:103` writes). `hold` pushes the flag `no-country` after the BLE flags when `country` is None.
- Produces (menu): `pub const NO_COUNTRY: &str = "No country set: set the time zone in the Time band below for faster FloeDrop."` in share_state.rs, shown with `Tone::Warn` after the ready line when `flags` holds `no-country`.

- [ ] **Step 1: Write the failing tests**

- card.rs `a_layout_that_is_a_country_gives_it` (fixture `testdata/zone1970.tab`; extend it with `DE`, `GB` and `US` lines if missing): `ch` → CH, `de` → DE, `gb` → GB, `us` → None, `eu` → None, `latam` → None, `zz` → None, `""` → None.
- card.rs `the_first_layout_of_a_list_decides`: `"ch,de"` → CH; `"us,de"` → None.
- card.rs `the_zone_wins_over_the_layout`: `choose_country(Some("Asia/Tokyo"), Some("ch"), ZONES) == Some("JP")`; `choose_country(Some("UTC"), Some("ch"), ZONES) == Some("CH")`; `choose_country(None, None, ZONES) == None`.
- radio.rs `the_layout_comes_from_generated_environment`: `env_layout("# header
XKB_DEFAULT_LAYOUT=ch
XKB_DEFAULT_VARIANT=fr
") == Some("ch")`; `XKB_DEFAULT_LAYOUT=` → None; no line → None.
- radio.rs `no_country_is_a_flag`: `hold` with country None prints flags `no-ble,no-country`; with `Some("CH")` only `no-ble`.
- share_state.rs `no_country_warns_in_the_band`: `status_lines(&ready(&["no-country"]))` = [ready line Plain, (NO_COUNTRY, Warn)]; with `["no-ble", "no-country"]` the Bluetooth line comes before the country line.

- [ ] **Step 2: Run them to verify they fail**

Run: `SHARE_TEST`, `MENU_TEST share_state`
Expected: FAIL (functions missing).

- [ ] **Step 3: Implement** as the Interfaces block says. `gather` logs on stderr `thinfloe-share: country <CC> from the keyboard layout <layout>` when the fallback decides.

- [ ] **Step 4: Run tests**

Run: `make test-share` and `make test-menu`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-share src/thinfloe-menu/src/share_state.rs
git commit -m "fix(floedrop): take the country from the keyboard layout, warn without one"
```

---

### Task 5: Detect a stick that stopped sending (A4)

**Files:**
- Create: `src/thinfloe-share/src/health.rs` (STATS parsing and the wedge rule; `mod health;` in `main.rs`)
- Modify: `src/thinfloe-share/src/card.rs` (the text), `src/thinfloe-share/src/radio.rs` (`RadioApi`, `hold`, `Owlyd`, tests)
- Modify: `test/integration/test-floedrop-image.sh` (`usr/bin/journalctl` executable)

**Interfaces:**
- Consumes: `CardInfo.bus` (card.rs).
- Produces (health.rs):
  - `pub struct Counters { pub sent: u64, pub radio_full: u64, pub gave_up: u64 }` (`sent` = tx_action + tx_unicast + tx_multicast).
  - `pub fn parse_stats(line: &str) -> Option<Counters>`: the text after `STATS ` anywhere in the line (owlyd's tracing prefix and `journalctl -o cat` vary); None for other lines.
  - `pub const STUCK_GAVE_UP: u64 = 20;` `pub const STUCK_INTERVALS: u32 = 3;` An interval (two consecutive STATS lines, 10 s apart) is stuck when Δgave_up ≥ STUCK_GAVE_UP and 2·Δgave_up ≥ Δsent. Wedged after STUCK_INTERVALS stuck intervals in a row (30 s). A counter lower than before (owlyd restarted) makes the line the new baseline and resets the count.
  - `#[derive(Default)] pub struct Health` with `pub fn feed(&mut self, c: Counters) -> bool` (true once wedged).
  - `pub fn busy(text: &str) -> bool`: the text holds "os error 16" or "Device or resource busy".
- Produces (card.rs): `pub fn wedged_text(station: &str, bus: Bus) -> String` with the two texts of Global Constraints.
- Produces (radio.rs): `RadioApi` gains `async fn wait_wedged(&self)` (completes once `Health::feed` says wedged). Owlyd's impl spawns `journalctl -u owlyd.service -f -n 0 -o cat --no-pager` with `kill_on_drop(true)` and stdout piped, feeds every parsed line, and logs `thinfloe-share: <station> gave up <n> of <m> frames for 30 s; the card stopped sending` once; if journalctl cannot start it logs that and stays pending (no detection, never a false error).

- [ ] **Step 1: Write the failing tests**

health.rs (`line(sent, full, gave_up)` builds a real line: `2026-11-26T10:00:00.000000Z  INFO owly_awdl::driver: STATS tx_action {a} tx_unicast 0 tx_multicast 0 tx_amsdu 0 | radio full {f} gave up {g} | rx_action 300 rx_data 50 rx_malformed 0 delivered 50 | dropped no_peer 0 queue 0 expired 0 | syn held 0 released 0 | no_overlap 0 | timer late avg 12 µs max 80 µs`):
- `stats_line_parses`: `parse_stats(&line(120, 7, 3)) == Some(Counters { sent: 120, radio_full: 7, gave_up: 3 })`; the same text without the prefix parses; `"add peer aa:bb"` → None; a STATS line with a non-number → None.
- `a_healthy_radio_is_never_wedged`: 20 lines with sent +120 and gave up +2 per line: never true.
- `three_stuck_intervals_in_a_row_are_a_wedge`: baseline, then sent +100 and gave up +60 per line: `feed` is false, false, true (third interval).
- `a_good_interval_resets_the_count`: stuck, stuck, good, stuck, stuck: never true; one more stuck: true.
- `counters_that_go_back_start_again`: stuck, stuck, then all counters back to 0 (restart), then stuck, stuck: false; one more: true.
- `few_frames_given_up_are_not_stuck`: Δgave_up 19 with Δsent 19: false forever.
- `a_busy_device_is_seen`: `busy("Couldn't bring owlyibss0 up: Device or resource busy (os error 16)")` true; `busy("AirDrop's radio is in use (lease 3).")` false.

card.rs `the_wedged_text_fits_the_bus`: `wedged_text("wlxc03a55f0d060", Bus::Usb)` == "The Wi-Fi stick wlxc03a55f0d060 stopped sending. Unplug it and plug it in again."; `Bus::Pci` → "The Wi-Fi card wlp3s0 stopped sending. Restart the machine." (with `wlp3s0`).

radio.rs (the `Fake` gets `wedged: bool` and `acquire_error: Option<String>`):
- `a_wedge_releases_the_lease_and_says_why`: station `wlx00e04c123456` as `Bus::Usb`, `wedged: true`, stop pending: exit 1, lines = [up line, `error	The Wi-Fi stick wlx00e04c123456 stopped sending. Unplug it and plug it in again.`], calls end with `"release 5"`.
- `a_busy_start_is_a_wedged_stick`: `acquire_error: Some("Couldn't bring owlyibss0 up: Device or resource busy (os error 16)")`: exit 1, output the same wedged error line, no release call.
- `another_acquire_error_is_passed_on`: acquire error "AirDrop's radio is in use (lease 3)." → `error	AirDrop's radio is in use (lease 3).`.

- [ ] **Step 2: Run them to verify they fail**

Run: `SHARE_TEST health`, `SHARE_TEST radio::tests`, `SHARE_TEST card::tests`
Expected: compile errors (module and functions missing).

- [ ] **Step 3: Implement**

`hold`: on an acquire error, `health::busy(&e)` turns it into `wedged_text(&station, bus)`; after the up line, a third `select!` branch `() = api.wait_wedged() => { api.release(lease).await; say(error wedged_text); 1 }`. `bus` from `card.cards.get(&station)` (default `Bus::Other`). In `test-floedrop-image.sh` add `usr/bin/journalctl` to `FILES` and the executable check.

- [ ] **Step 4: Run tests**

Run: `make test-share`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-share test/integration/test-floedrop-image.sh
git commit -m "fix(floedrop): end with a replug message when the stick stops sending"
```

---

### Task 6: Receiving lines in the state file and receive log lines (A2, daemon)

**Files:**
- Create: `src/thinfloe-share/src/incoming.rs` (`mod incoming;` in `main.rs`)
- Modify: `src/thinfloe-share/src/state.rs:1-176` (render, parse, write; module doc), tests
- Modify: `src/thinfloe-share/src/daemon.rs:316-328` (`StateFile`), `:518-525` (main loop step), `:557-562` (`next_step`), `:826-840` (`Airdrop.pending`), `:1010-1041` (`step`), `:1079-1140` (`sender_of`, `on_receiver`)
- Test: `src/thinfloe-share/tests/veth.rs:169-224` (`receive_from_owly_sender`)

**Interfaces:**
- Produces (incoming.rs): `#[derive(Debug, Clone, PartialEq, Eq)] pub struct Incoming { pub sender: String, pub files: u32, pub bytes: u64, pub first: String, pub since: u64 }`; `pub fn from_offer(sender: &str, offer: &Offer, since: u64) -> Incoming` (files = entries with `!is_directory`, first = the first such name, bytes = `ask_total` moved here from daemon.rs:127-135; sender "iPhone" when blank). Log texts, `Incoming` methods:
  - `offer_line(&self) -> String`: `offer from "Jane's iPhone": 2 files, 75000000 bytes` (one file: `1 file`)
  - `accepted_line(&self)`: `accepted the offer from "Jane's iPhone"`
  - `declined_line(&self, free: u64)`: `declined the offer from "Jane's iPhone": 2 files, 75000000 bytes, 1000 bytes free`
  - `received_line(&self)`: `received 2 files, 75000000 bytes from "Jane's iPhone"`
  - `failed_line(&self, why: &str)`: `receiving 2 files, 75000000 bytes from "Jane's iPhone" failed: <why>`
  - `withdrawn_line(&self)`: `the offer from "Jane's iPhone" was withdrawn`
  - A links offer logs `links from "<sender>" declined` (no Incoming).
- Produces (state.rs): the receiver line `sender<TAB>folder<TAB>receiving<TAB>since<TAB>files<TAB>bytes<TAB>first`, folder = `names::safe(sender, "iPhone")`; `pub fn render(head: &Head, devices: &[Device], incoming: &[Incoming]) -> String` (outbox lines first, then receiving lines), `pub fn write(path: &Path, head: &Head, devices: &[Device], incoming: &[Incoming]) -> io::Result<()>`, `pub fn parse_incoming(text: &str) -> Vec<Incoming>` (lines with field 3 `receiving` and at least 7 fields); `parse` keeps its signature and skips receiving lines.
- Produces (daemon.rs): `Airdrop.pending: HashMap<(u64, u64), Incoming>`; `Airdrop.devices: Vec<Device>` (last list from the outbox); `fn lists(&self) -> (Vec<Device>, Vec<Incoming>)` (incoming sorted by since, then sender); `step` and `next_step` return `bool` (something changed); `StateFile` gains `incoming: Vec<Incoming>`, cleared with `devices` at the end.

- [ ] **Step 1: Write the failing tests**

incoming.rs:
- `an_offer_counts_files_not_folders`: Files `[dir "Album", "Album/a.jpg" 10, "Album/b.jpg" 5]` → files 2, bytes 15, first = the `name` field of the first non-directory entry as the fixture gives it (assert that exact string).
- `the_log_lines_name_sender_files_and_bytes`: each of the six texts above, exactly, for sender "Jane's iPhone", 2 files, 75000000 bytes; `offer_line` with one file says `1 file`.

state.rs:
- `render_receiving_line`: `render(&Head::Starting, &[jane()], &[incoming])` == `"starting
Jane's iPhone	Jane's iPhone	idle	1764000000
Jane's iPhone	Jane's iPhone	receiving	1764000100	2	75000000	IMG_0042.DNG
"`.
- `a_receiving_line_with_tabs_stays_one_line`: sender `"a	b"` and first `"x	y.jpg"` render as `a_b` / `x_y.jpg`, one line, 7 fields; `parse_incoming` reads it back.
- `parse_skips_receiving_lines_for_devices`: `parse` of the text above gives one device; `parse_incoming` gives the one Incoming; a `receiving` line with 5 fields is skipped.
- Update `round_trip_and_atomic_write` and the render tests to the new signatures.

daemon.rs (pure parts):
- `two_transfers_have_two_lines`: build `lists`' sort through a free function `fn incoming_list(pending: &HashMap<(u64, u64), Incoming>) -> Vec<Incoming>`: two entries since 5 ("B") and 3 ("A") → ["A", "B"]; removing one key leaves one.

veth.rs `receive_from_owly_sender`: after the delivery, the state file has no `receiving` line (the transfer ended). The start line is not asserted (too short to see).

- [ ] **Step 2: Run them to verify they fail**

Run: `SHARE_TEST incoming`, `SHARE_TEST state::tests`, `SHARE_TEST two_transfers_have_two_lines`
Expected: compile errors.

- [ ] **Step 3: Implement**

`on_receiver`: Request → `log(offer_line)` (links: the links line); Accept → insert into `pending`, `log(accepted_line)`; Decline → `log(declined_line(free))` (replaces the old line at 1101-1105); Received → remove, `log(received_line)` on success (placing errors keep their own line); Failed → remove, `log(failed_line(reason))` (replaces 1132); Withdrawn → remove, `log(withdrawn_line)` when an entry existed. Each of these returns "changed". `sender_of` becomes `fn take(&mut self, generation: u64, id: u64) -> Incoming` (an unknown id gives sender "iPhone", 0 files). The main loop writes the state when `lists()` differs from `(st.devices, st.incoming)`. Update the module doc of state.rs with the new line.

- [ ] **Step 4: Run tests**

Run: `make test-share`
Expected: PASS (veth tests run where user namespaces exist).

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-share
git commit -m "feat(floedrop): receiving lines in the state file, a log line per offer and delivery"
```

---

### Task 7: The band shows a running transfer (A2, menu)

**Files:**
- Modify: `src/thinfloe-menu/src/share_state.rs:1-112` (doc, parse, read), `:159-229` (`ShareBand`), tests `:272-563`
- Modify: `src/thinfloe-menu/src/network_ui.rs:250-255` (draw the lines)
- Modify: `src/thinfloe-menu/src/app.rs:1098-1099` (unchanged call, new tuple)
- Test: `src/thinfloe-menu/src/ui.rs:6088-6130` (`the_floedrop_band_follows_dns`)

**Interfaces:**
- Consumes: the receiving line of Task 6.
- Produces: `pub struct Receiving { pub sender: String, pub files: u32, pub first: String }`; `pub fn parse_receiving(text: &str) -> Vec<Receiving>`; `pub fn read(path: &Path) -> Option<(Head, Vec<Receiving>, SystemTime)>`; `ShareBand::set_file(&mut self, file: Option<(Head, Vec<Receiving>, SystemTime)>)`; `ShareBand::receiving(&self) -> &[Receiving]` (empty unless `head()` is `Some(Head::Ready { .. })` from this run's file); `pub fn receiving_lines(r: &[Receiving]) -> Vec<(String, Tone)>` (Tone::Plain; "Receiving IMG_0042.DNG from Jane's iPhone"; "Receiving 3 files from Jane's iPhone").

- [ ] **Step 1: Write the failing tests**

- share_state.rs `receiving_lines_parse_and_read`: `parse_receiving` of `"ready	awdl0	exclusive	44		wlx1	rtw88_8822bu
Jane's iPhone	Jane's iPhone	idle	1
Jane's iPhone	Jane's iPhone	receiving	2	1	75000000	IMG_0042.DNG	extra
X	X	receiving	3
"` is one Receiving { "Jane's iPhone", 1, "IMG_0042.DNG" }.
- `the_band_says_what_is_arriving`: `receiving_lines` of one file → "Receiving IMG_0042.DNG from Jane's iPhone"; of files 3 → "Receiving 3 files from Jane's iPhone"; of an empty slice → [].
- `a_transfer_shows_only_while_ready`: a band with the switch on, a running daemon and a file with Head::Error and one Receiving: `receiving()` is empty; with Head::Ready it has the one entry; switch off: empty.
- ui.rs `the_floedrop_band_follows_dns`: the Ready file gets one Receiving and the texts hold "Receiving IMG_0042.DNG from Jane's iPhone". The existing `set_file` calls in share_state.rs tests pass `Vec::new()`.

- [ ] **Step 2: Run them to verify they fail**

Run: `MENU_TEST share_state` and `MENU_TEST the_floedrop_band_follows_dns`
Expected: compile errors.

- [ ] **Step 3: Implement**

`network_ui.rs`: after the `status_lines` loop, the same label call for each of `receiving_lines(band.receiving())`. Update the module doc of share_state.rs ("the lines after it are not shown" is no longer true for receiving lines).

- [ ] **Step 4: Run tests**

Run: `make test-menu`
Expected: PASS, including `ui_code_has_no_style_literals`.

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-menu/src
git commit -m "feat(menu): the FloeDrop band shows a file that is arriving"
```

---

### Task 8: One receiver after FloeDrop off/on (A5)

**Files:**
- Modify: `src/thinfloe-share/src/daemon.rs:536-555` (the end of `run`: order), `:893-903` (`service_id` in `Airdrop::start`), `:1142-1154` (`shutdown`)
- Test: `src/thinfloe-share/tests/veth.rs` (new tests; `Env::spawn` `:109-130` gets a variant with a script tail)

**Interfaces:**
- Produces: `fn service_id_at(path: &Path) -> String` (daemon.rs): the 12 lower-case hex digits in `path` when the file holds exactly that (trailing newline allowed), else a new `mdns::service_id(random_bytes())` written atomically (temp file + rename, mode 0600); a write error is logged and the new id used. Path: `paths.identity.join("service-id")` (`$XDG_RUNTIME_DIR/thinfloe-share/service-id`, next to the TLS identity, gone at logout). `const GOODBYE_WAIT: Duration = Duration::from_millis(200)` replaces the 50 ms sleep in `shutdown`.

- [ ] **Step 1: Write the failing tests**

- daemon.rs `the_service_id_is_kept_in_the_runtime_dir` (dir below `target/test-tmp`): first call makes a 12-hex id and the file; second call returns the same; a file holding `"nonsense"` is replaced by a new valid id; mode of the file 0600.
- veth.rs `the_service_is_withdrawn_before_the_radio_goes`: the fake helper's tail is `cat >/dev/null; ip link set owb down` (the radio going away). Browse on owa with `Mdns::new(a.index).browse()` until `BrowseEvent::Found` (10 s); SIGTERM the daemon; `BrowseEvent::Lost(instance)` for the found instance arrives within 5 s; the daemon exits 0.
- veth.rs `a_restart_keeps_the_service_name`: Found instance X; stop; start a second daemon on the same Env; the next Found has instance X.

- [ ] **Step 2: Run them to verify they fail**

Run: `SHARE_TEST the_service_id_is_kept_in_the_runtime_dir` and `SHARE_TEST --test veth`
Expected: the unit test does not compile; the two veth tests fail (no Lost within 5 s: the goodbye goes after the link is down; a new random instance after restart). Where user namespaces are off the veth tests print "skipped" and pass: then the controller runs them on a host that has them before the review.

- [ ] **Step 3: Implement**

End of `run`: write the error state, then `a.shutdown().await` (goodbye, `GOODBYE_WAIT`), then `drop(stdin.take())`, then wait for the helper. `Airdrop::start` takes `service_id_at(&paths.identity.join("service-id"))` after `Identity::load_or_generate` made the directory.

- [ ] **Step 4: Run tests**

Run: `make test-share`
Expected: PASS; `sigterm_closes_the_helper_stdin` and `sigterm_while_the_radio_starts` in tests/daemon.rs still pass (EOF within their limits).

- [ ] **Step 5: Commit**

```bash
git add src/thinfloe-share
git commit -m "fix(floedrop): withdraw the receiver before the radio goes, keep its id per session"
```

---

### Task 9: CHANGES.md, docs/notes/floedrop.md, CLAUDE.md contract line (A10 and all)

**Files:**
- Modify: `CHANGES.md` (`## [Unreleased]`, no release header)
- Modify: `docs/notes/floedrop.md`
- Modify: `CLAUDE.md` (FloeDrop bullet under "Rules that are easy to break")

- [ ] **Step 1: CHANGES.md entries** (users and administrators; what they saw first; at most three sentences each; no function names)

Under `### Fixed`:
- Settings → Network and the menu log said "channel 6" for FloeDrop while the radio ran on channel 44. They now show the channel the radio really uses.
- After FloeDrop was switched off and on, iPhones could list the machine twice and send the same files to both, so the inbox got " (2)" copies. The machine now shows up once.
- When a TP-Link RTL8812BU stick stopped sending, FloeDrop still said "Ready" and iPhones did not see the machine; later starts failed with "Device or resource busy". FloeDrop now stops and says "The Wi-Fi stick … stopped sending. Unplug it and plug it in again."
- A USB Wi-Fi stick that Owly cannot drive kept FloeDrop from using a working internal card; FloeDrop now tries the next card. The Intel Wireless 7260, of the same family as the 3160, is no longer used. When the only free card cannot run AirDrop and a USB stick is busy with the network, the message names the stick: "… the USB stick … is used for the network."
- "FloeDrop cannot start on …" now names the Wi-Fi card and driver instead of awdl0.

Under `### Changed`:
- While a phone sends files, Settings → Network shows "Receiving IMG_0042.DNG from Jane's iPhone" (or "Receiving 3 files from …"), and the menu log has a line for each offer, accept, decline and finished or failed delivery, with sender, file count and size.
- With no time zone set, FloeDrop takes the country from the keyboard layout (for example ch gives Switzerland), which allows the faster 5 GHz channel. If neither gives a country, Settings → Network says "No country set: set the time zone in the Time band below for faster FloeDrop."

- [ ] **Step 2: docs/notes/floedrop.md**

- Processes: the up line's channel is the link's from `iw dev` (0 when unknown, then left out of the texts); flags may hold `no-country`; the helper follows owlyd's journal for STATS.
- Card choice and country: cards in preference order, each probed, the first accepted taken (else Owly's reason for the first); 7260 (8086:08b1, 8086:08b2) on the skip list by family, untested; the used-stick text; country: zone, else keyboard layout (first of the list; `us` and `eu` give none; must be a code of zone1970.tab), else none with the band warning.
- Contracts: the `receiving` receiver line `name<TAB>folder<TAB>receiving<TAB>since<TAB>files<TAB>bytes<TAB>first`; flag `no-country`.
- Behaviour rulings: the wedge rule (STATS every 10 s; an interval is stuck when at least 20 frames were given up and at least half of those sent; 3 stuck intervals in a row, 30 s; EBUSY at start counts; the lease is released); the service id kept in `$XDG_RUNTIME_DIR/thinfloe-share/service-id` and the goodbye sent before the helper's stdin closes; the log lines.
- Tests (A10): replace "QEMU has no AWDL card: `make qemu-smoke` shows the band's error text" by: QEMU has no AWDL card; `make qemu-smoke` checks that boot and menu are unaffected, no test checks the band's text there. Add the new veth tests and test-floedrop-image.sh's `iw` and `journalctl`.
- Hardware results (2026-11-26, thinfloe-2b3967, ThinFloe 4.13.0): the wedge, channel 6 under UTC with the timings, the double listing.
- Owner checklist: read a healthy stick's STATS (`journalctl -u owlyd -g STATS`) and check the threshold leaves it alone; FloeDrop off/on shows one receiver; a large file shows "Receiving …" until it lands; with UTC and layout ch the band shows channel 44.
- Known limits: no byte progress (Owly gives none); a zone or layout change takes effect at the next FloeDrop start; the wedge threshold rests on one wedged stick.

- [ ] **Step 3: CLAUDE.md**

In the FloeDrop state-file bullet, after `name<TAB>folder<TAB>state<TAB>since`, add: ", state `receiving` with `files<TAB>bytes<TAB>first` after it".

- [ ] **Step 4: Check the files**

Run: `git diff --stat` and read the three diffs once.
Expected: only these three files; no release header in CHANGES.md.

- [ ] **Step 5: Commit**

```bash
git add CHANGES.md docs/notes/floedrop.md CLAUDE.md
git commit -m "docs(floedrop): hardware fixes in CHANGES and the notes"
```

---

### Task 10: Gates

The branch changes thinfloe-share (the image's helper and daemon), the menu's Network band and an image guard, so it runs the full set of CLAUDE.md "Workflow" before it joins the round branch. The smoke test it touches is `make qemu-smoke` (boot and menu with FloeDrop present and no AWDL card). It touches no ESP, upgrade, netboot, timezone or automount path, so the other smoke targets are not needed.

- [ ] **Step 1: Unit gates**

Run: `flock /scratch/oetiker/claude-tmp/thinfloe-cargo.lock make test-share && flock /scratch/oetiker/claude-tmp/thinfloe-cargo.lock make test-menu && flock /scratch/oetiker/claude-tmp/thinfloe-cargo.lock make test-unit` (Bash timeout 600000)
Expected: all green; the test counts grew by the tests this plan names compared with Task 0.

- [ ] **Step 2: Image and integration**

Run: `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make image VERSION=floedrop-hw && flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test VERSION=floedrop-hw` (one shell command, long timeout)
Expected: "0 not ok"; `test-floedrop-image.sh` passes with `iw` and `journalctl`.

- [ ] **Step 3: Runtime and smoke**

Run: `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test-runtime VERSION=floedrop-hw && flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make qemu-smoke VERSION=floedrop-hw`
Expected: test-runtime "0 not ok"; qemu-smoke passes.

- [ ] **Step 4: Record**

Put the commit hash, the commands and the counts in the PR body's Gates section (round PR, branch heading `fix/floedrop-hardware`). Hardware checks (wedge, double listing, channel 44 under UTC with layout ch) go to the owner checklist, not to the gates.

---

## Self-review

- Spec coverage: A1 Task 3; A2 Tasks 6 and 7; A3 Task 4; A4 Task 5; A5 Task 8; A6 Task 2; A7, A8, A9, A11 Task 1; A10 Task 9; toggle saves the home: no change (Global Constraints).
- Names used across tasks: `card_label` (1→3), `candidates`/`Probe`/`RadioApi::probe` (2), `link_channel`/`iw_channel` (3), `layout_country`/`choose_country`/`env_layout`/`NO_COUNTRY` (4), `Counters`/`Health`/`busy`/`wedged_text`/`wait_wedged` (5), `Incoming`/`parse_incoming` (6), `Receiving`/`parse_receiving`/`receiving_lines` (7), `service_id_at`/`GOODBYE_WAIT` (8).
- The `Fake` in radio.rs grows over Tasks 2, 3 and 5 (`refuse`, `toml`, `channel`, `wedged`, `acquire_error`); each task updates the expected call lists of the tests it touches.
