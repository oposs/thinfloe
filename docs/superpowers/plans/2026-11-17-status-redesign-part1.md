# Status redesign, part 1: tabs and tile split — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The status area keeps only live values (new `connection` tile, zone on the clock, fans with the temperatures), and the fixed facts move to the Network tab, a Graphics band on the Screen tab and a new "This machine" tab; an opt-in public IP lookup feeds the `connection` tile.

**Architecture:** Element and layout changes stay inside `src/thinfloe-menu/src/status/`. New facts are read by small pure modules (`pciids.rs`, `netdetail.rs`, `graphics.rs`, `machine.rs`, `publicip.rs`) that take their roots/commands as parameters, so tests run on a fake tree and fake scripts. Subprocesses (`curl`, `vainfo`, `wl-copy`) run through one helper `quiet::run` in their own process group with a timeout, a stdout cap and stderr discarded, always off the UI thread (menu jobs or the lookup thread).

**Tech Stack:** Rust (menu crate, egui + shadcn-egui `lyra`, jiff), no new crate.

**Spec:** [docs/superpowers/specs/2026-11-17-status-redesign-design.md](../specs/2026-11-17-status-redesign-design.md) — section "Part 1: tabs and tile split" and the part-1 lines of "Tests". Part 2 (`response`) and part 3 (`history-*`) are out of scope.

## Global Constraints

- Read `CLAUDE.md` and `docs/notes/status-area.md` first. Every rule there holds: `draw` is pure and stays in its rect, colours from the theme, text sizes from named constants in `brand.rs` (`ui_code_has_no_style_literals` covers every new UI file), shadcn-egui `lyra` widgets, nothing rounded.
- `config.toml` is written only through the settings store (`store.rs`), from a job thread.
- No subprocess in a sampler that runs every second; the public IP lookup runs in its own thread, never in the network sampler.
- The public IP lookup never runs unless the user switched it on.
- New `.rs` files are picked up by `MENU_FILES`/`MENU_DIRS` (a `find` in the `Makefile`); no Makefile line is needed. Register each module in `src/thinfloe-menu/src/lib.rs`. No new package in the Dockerfile: `curl`, `vainfo`, `wl-clipboard` and `pciutils` (`/usr/share/misc/pci.ids`) are in the image already; `usb.ids` is not.
- Gates for every task, run in `src/thinfloe-menu`, max 4 cores:
  - `systemd-run --user --scope -q -p MemoryMax=4G -- env CARGO_BUILD_JOBS=4 cargo test --locked -j 4` — all pass
  - `systemd-run --user --scope -q -p MemoryMax=4G -- env CARGO_BUILD_JOBS=4 cargo clippy --locked -j 4 --all-targets -- -D warnings` — no warnings
  - `cargo fmt --check` — no diff
- One commit per task, message `feat(menu): …` / `docs: …`, ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Decisions this plan makes (the spec left them open)

- HTTP: `curl` subprocess via `quiet::run`: `curl -s --proto =https --max-redirs 0 --max-time 5 --max-filesize 64 --write-out '\n%{response_code}' --url <URL>`; only status `200` counts; stdout capped at 128 bytes, body at 64. No `-L`, so no redirect is followed; `--url` keeps a URL from ever being read as an option. No HTTP crate.
- GPU and WiFi names come from `/usr/share/misc/pci.ids` (present in the image), read streaming with a 4 MiB cap; without a match, the ids (`8086:a0f0`). USB devices show `USB 0bda:8179` (no `usb.ids` in the image).
- Boot type: `Menu.usb_boot` (set by `main.rs` through `set_usb_boot`). Add-on versions: `AddonStatus.installed` of `Menu::addons()`, labelled with the tile name. Host name and version: the `header::Header` that `main.rs` already reads, handed to the menu with a new `Menu::set_header`.
- `hardware::Snapshot` loses `uptime`, `maker`, `model` (only the retired `system` element drew them); the "This machine" tab reads them itself.
- `netstat::Info` loses `dns` (the tiles no longer show it); `netstat::dns_in_use` serves the Network tab. `Info.gateway` stays: it is part of the lookup's route key.
- Public IP settings are saved through a new `generated::Table::PublicIp` with no generated files, so a switch flip writes only `config.toml`. The switch saves at once; the address field saves on Enter or focus loss and only when empty or `https:`.
- Turning the switch off clears the answer and the schedule; a changed lookup address restarts the schedule. A failure is logged once when the last state was not already a failure.
- A retired `network-info` line that meets an existing `connection` line is dropped silently.
- `connection` row text exact: `Link`, `IPv4`, `VPN`, `Public`; failure value `–` (U+2013).
- The clock's long zone name is cut at the start with a leading `…`.
- The "This machine" tab leaves out a row whose value is unknown; the uptime is counted on from the reading (no re-read each minute).
- vainfo runs as `vainfo` (the session's display) with a 5 s timeout, once per menu run.

## Review Focus

1. A saved layout with `-network-info 1x2` (hidden) or with both `network-info` and `connection`: the user expects one `connection`, hidden if it was hidden, at the old place and size. Test: Task 1 `a_hidden_network_info_line_stays_hidden_as_connection`, `a_network_info_line_beside_connection_is_dropped`.
2. A hand-edited `public_ip_url` (`http://…`, `file:///…`, ` https://x` with a space, a newline, a value starting with `-`): no request may leave the machine. Test: Task 6 `a_lookup_refuses_a_bad_address_without_running_curl`.
3. A Network Apply or "Let the menu manage it" running while the switch is flipped: the switch must not snap back. Test: Task 5 `a_network_apply_keeps_the_public_ip_settings_set_while_it_ran`.
4. A captive portal that answers `200` with an HTML page, or a lookup service that answers with a long body or a redirect: shown as `Public –`, never as garbage. Test: Task 6 `public_ip_answers_parse_only_as_an_address`, `the_lookup_follows_no_redirect`.
5. A machine without a DRM card, without `pci.ids`, or where `vainfo` hangs (QEMU, a thin client without VA-API): the Graphics band says so instead of waiting forever. Test: Task 8 `a_slow_vainfo_is_not_available`, `a_machine_without_drm_cards_has_no_gpu`.

---

### Task 1: Layout descriptors with a default-shown flag, and retired ids

**Files:**
- Modify: `src/thinfloe-menu/src/status/layout.rs` (`Descriptor`, `choice_of`, `default_layout`, `complete`, `parse`, tests)
- Modify: `src/thinfloe-menu/src/status/mod.rs` (`Element`, `descriptors`)

**Interfaces:**
- Produces:
  - `pub struct Descriptor { pub id: &'static str, pub size: Size, pub shown: bool }` (replaces the tuple type alias).
  - `Element::shown_by_default(&self) -> bool` — provided method, default `true`.
  - `layout::RETIRED: [(&str, Option<&str>); 2] = [("network-info", Some("connection")), ("system", None)]`.
  - `default_layout(&[Descriptor])` and `complete(&mut Layout, &[Descriptor])` use `Descriptor::shown`.

- [ ] **Step 1: Write the failing tests in `layout.rs`**

```rust
#[test]
fn new_elements_are_appended_shown() {           // kept, gains the hidden case
    let elems = [d("battery", sz(1, 1), true), d("clock", sz(1, 1), true), d("history-temp", sz(2, 1), false)];
    let mut l = p("-clock 2x1\n");
    complete(&mut l, &elems);
    assert_eq!((l.choices[1].id.as_str(), l.choices[1].size, l.choices[1].shown), ("battery", sz(1, 1), true));
    assert_eq!((l.choices[2].id.as_str(), l.choices[2].size, l.choices[2].shown), ("history-temp", sz(2, 1), false));
    assert!(!default_layout(&elems).choices[2].shown);
}
#[test]
fn a_network_info_line_becomes_connection_at_its_place_and_size() {
    // known() offers "connection": [1x2, 2x1]
    let l = p("battery 1x1\nnetwork-info 2x1\nclock 1x1\n");
    assert_eq!(l.choices[1], Choice { id: "connection".into(), size: sz(2, 1), shown: true, known: true });
    assert!(l.notes.is_empty(), "{:?}", l.notes);
    assert!(!to_text(&l).contains("network-info"));
}
#[test]
fn a_hidden_network_info_line_stays_hidden_as_connection() { /* "-network-info 1x2" -> connection 1x2 shown=false */ }
#[test]
fn a_system_line_is_dropped_without_a_note() { /* "battery 1x1\nsystem 2x1\n" -> ids ["battery"], notes empty */ }
#[test]
fn a_network_info_line_beside_connection_is_dropped() { /* "connection 1x2\nnetwork-info 2x1\n" -> one connection 1x2, notes empty */ }
```

`d(id, size, shown)` is a test helper building a `Descriptor`; `known` in the test module offers `connection`.

- [ ] **Step 2: Run** `cargo test status::layout` — expect compile failure (`Descriptor` is a tuple).
- [ ] **Step 3: Implement.** In `parse`, map the id through `RETIRED` before the duplicate check: a `None` successor skips the line silently; a successor already in `seen` skips silently; otherwise the line continues under the new id. `descriptors()` in `mod.rs` fills `shown` from `shown_by_default()`. Update the other `ELEMS` uses in the test module to the struct.
- [ ] **Step 4: Run** the gates — all pass.
- [ ] **Step 5: Commit** `feat(menu): status layout knows retired ids and hidden-by-default elements`.

---

### Task 2: The `connection` element replaces `network-info`; the session total goes

**Files:**
- Rename: `src/thinfloe-menu/src/status/network_info.rs` → `src/thinfloe-menu/src/status/connection.rs` (`git mv`, then edit)
- Modify: `src/thinfloe-menu/src/status/mod.rs` (`pub mod connection`, `ELEMENTS`, `Data`)
- Modify: `src/thinfloe-menu/src/netstat.rs` (drop `Since`, `Series.total`, `Snapshot.total`, `bytes`, `Info.dns`; add `dns_in_use`)
- Modify: `src/thinfloe-menu/src/menu.rs` (drop `netstat_since`, `network_bases`; `set_network`)
- Modify: `src/thinfloe-menu/src/app.rs`, `src/thinfloe-menu/src/ui.rs` (callers and tests that build `Since`)
- Modify: `src/thinfloe-menu/src/status/network.rs` (module doc names `connection`)

**Interfaces:**
- Produces:
  - `pub struct Connection;` id `"connection"`, name `"Connection"`, sizes `[1x2, 2x1]`, short size `None`, sources `[Source::Network]`, `has_content` = `snap.info.visible()` (as `network-info` today).
  - `pub fn rows(snap: &Snapshot) -> Vec<Row>` (Task 6 adds a `public` parameter); rows `Link`, `IPv4`, `VPN`; drop order from the bottom `Public`, then `VPN`; `Link` and `IPv4` stay. `LABELS = ["Link", "IPv4", "VPN", "Public"]`.
  - `Data` loses `network_since`.
  - `Menu::set_network(&mut self, snap: netstat::Snapshot)`.
  - `pub fn dns_in_use(roots: &Roots) -> Vec<String>` in `netstat.rs`: `run/systemd/resolve/resolv.conf`, else `etc/resolv.conf`, through `nameservers`, `MAX_DNS` and `MAX_DNS_LEN` as today.
- `ELEMENTS` for this task: battery, network, connection, temperatures, cpu, memory, system, clock (the reorder is Task 3).

- [ ] **Step 1: Rewrite the tests in `connection.rs`**: `one_fact_a_row_label_left_value_in_a_column` (sample → `["Link eth0 1 Gbit/s", "IPv4 192.168.1.23/24", "VPN wg0 up"]` with the sample's values), `rows_tighten_first_then_the_vpn` (order of dropping `["VPN"]`; `Link`, `IPv4` always stay), `the_wide_size_keeps_the_link_the_address_and_the_vpn`, `connection_draws_inside_its_rect_at_every_size`, `it_has_content_when_the_graph_has`; keep `a_wifi_row_that_does_not_fit_leaves_out_the_word_wifi`, `a_long_value_is_cut_at_the_padding`, `names_from_the_system_never_break_a_row`, `no_text_overlaps_another_or_leaves_the_element`, `labels_shrink_only_on_the_tightest_rows_of_the_large_cell`. Delete `the_rates_are_in_the_graphs_colours`, `the_session_total_counts_from_its_start`, and the Peak/Since parts of the others; delete `Tone::Rx`/`Tx` if unused.
- [ ] **Step 2: Add `dns_in_use_prefers_resolved_then_etc`** in `netstat.rs` (fake tree via `Roots::under`: both files → resolved's servers; resolved empty → `/etc`'s; neither → empty). Rename `a_gap_after_a_rest_restarts_the_graph_and_keeps_the_total` to `a_gap_after_a_rest_restarts_the_graph`, drop every `total` assertion there and in the other `Series` tests. Delete `the_network_total_counts_from_the_running_session_program` and `the_network_total_of_two_sessions_counts_from_the_one_that_remains` in `menu.rs`.
- [ ] **Step 3: Run** `cargo test` — expect failures/compile errors.
- [ ] **Step 4: Implement** the removals and the element; update `status/mod.rs` tests (`the_default_list_is_every_element_in_order`, `a_short_screen_gives_the_chosen_network_its_short_size_and_leaves_the_list_out`) and every layout string in `statustab.rs`, `ui.rs`, `app.rs` tests from `network-info` to `connection`.
- [ ] **Step 5: Run** the gates — all pass.
- [ ] **Step 6: Commit** `feat(menu): connection tile replaces network details; session totals dropped`.

---

### Task 3: Temperatures with fans, `system` retired, new default list, traffic short size 2x1

**Files:**
- Modify: `src/thinfloe-menu/src/status/hardware.rs` (`Show`, elements, `lines`, `lines_in`, tests)
- Modify: `src/thinfloe-menu/src/hardware.rs` (`Snapshot` loses `uptime`, `maker`, `model`; `Sampler.machine` goes; `machine`, `parse_uptime`, `clean_dmi` stay `pub` for Task 9)
- Modify: `src/thinfloe-menu/src/status/network.rs` (`short_size` → `SIZES[2]`, test `THREE_BY_ONE` → `TWO_BY_ONE`)
- Modify: `src/thinfloe-menu/src/status/mod.rs` (`ELEMENTS`, default-list tests)
- Modify: `src/thinfloe-menu/src/statustab.rs`, `src/thinfloe-menu/src/ui.rs`, `src/thinfloe-menu/src/app.rs` (fixtures naming `system`; delete the `hardware::System` picture test at `ui.rs` ~6866)
- Modify: `test/runtime/test-labwc-runtime.sh` (the cover layout: `-connection 1x2`, no `-system`)

**Interfaces:**
- Produces: `Show { Temperatures, Cpu, Memory }`; `Temperatures` name `"Temperatures and fans"`, sizes `[1x1, 1x2]`; `ELEMENTS = [battery, network, connection, clock, temperatures, cpu, memory]`; `status::hardware::uptime(secs: u64) -> String` stays `pub`.

- [ ] **Step 1: Write the failing tests**
  - `status/hardware.rs` `the_fans_follow_the_sensors_and_drop_first`: snapshot with 2 temps and fans `[1200, 900]`: `per = 3` → last line `Fans 1200 900 rpm`; `per = 2` → no fan line, both temps; one fan → `Fan 1200 rpm`.
  - `an_element_with_only_fans_or_only_sensors_has_content`.
  - Move `the_fans_that_fit_stay_with_their_unit_and_never_reach_the_label` and `the_first_fans_stay_and_one_more_would_not_fit` to `Show::Temperatures`; delete `system_draws_inside_its_rect_at_every_size`, `the_system_element_has_one_line_for_the_fans_and_one_for_the_machine`, `a_model_has_its_label_on_the_left_and_a_long_one_stops_short_of_it`; update `the_elements_are_what_the_spec_says`.
  - `status/mod.rs` `the_default_list_packs_to_the_redesign_layout` (replaces `the_default_list_built_from_the_elements_packs_to_the_pr6_layout`): ids `["battery","network","connection","clock","temperatures","cpu","memory"]`, packed `(col,row,w,h)` = `[(0,0,1,1),(1,0,2,2),(3,0,1,2),(0,1,1,1),(0,2,1,1),(1,2,1,1),(2,2,1,1)]`, `Area::shipped().planned_rows(false) == 3`. In `layout.rs` rename `the_default_list_packs_to_the_pr6_layout` to `first_fit_packs_eight_items_into_three_rows` (sizes unchanged; it now tests packing only).
  - `a_short_screen_holds_battery_and_the_traffic_graph`: shipped layout, short → `battery (0,0,1,1)`, `network (1,0,2,1)`, nothing else.
  - `statustab.rs`: replace `system` in the fixtures; the crowded layout's `NoRoom` case moves to `memory` or another element that still overflows.
- [ ] **Step 2: Run** `cargo test` — expect failures.
- [ ] **Step 3: Implement.** The fan row is added only when `temps.len() < per`; the width cut of `lines_in` now applies to `Show::Temperatures`.
- [ ] **Step 4: Run** the gates; also `bash -n test/runtime/test-labwc-runtime.sh`.
- [ ] **Step 5: Commit** `feat(menu): fans join the temperatures; system tile retired; new default list`.

---

### Task 4: The clock shows its zone

**Files:**
- Modify: `src/thinfloe-menu/src/status/clock.rs`

**Interfaces:**
- Produces: `pub fn zone_label(zone: &TimeZone, ts: Timestamp) -> String` — `zone.iana_name()` when it is `Some` and not `"localtime"` (the name `log::zone_for` gives a copied `/etc/localtime`); otherwise the abbreviation from `zone.to_offset_info(ts).abbreviation()`; the UTC zone gives `UTC`. `lines_at` gains the third line `("Zone", zone_label(..))`. In `paint`, a value too wide for the space right of the label (or, if the label is dropped, the padding) is cut from the start and prefixed with `…`.

- [ ] **Step 1: Failing tests**
  - `the_zone_line_names_an_iana_zone`: `Europe/Zurich` → third line `("Zone", "Europe/Zurich")`.
  - `a_posix_tz_shows_its_abbreviation`: `TimeZone::posix("CET-1CEST,M3.5.0,M10.5.0/3")` at a January epoch → `"CET"`, at a July epoch → `"CEST"`.
  - `a_copied_localtime_shows_its_abbreviation`: `log::zone_for(None, <copied Europe/Zurich tzif file in a TestDir>)` → `"CET"` in January.
  - `utc_is_utc`: `TimeZone::UTC` → `"UTC"`.
  - `a_long_zone_keeps_the_city`: `America/Argentina/Buenos_Aires` at the small cell height → drawn value ends with `Buenos_Aires` and starts with `…`; inside the rect.
  - `clock_draws_inside_its_rect_at_every_size` keeps passing with three lines at both `CELL_HEIGHTS`.
- [ ] **Step 2: Run** `cargo test status::clock` — fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the gates.
- [ ] **Step 5: Commit** `feat(menu): the clock names its time zone`.

---

### Task 5: Public IP settings and the Network tab switch

**Files:**
- Modify: `src/thinfloe-menu/src/config.rs` (`NetworkCfg`)
- Modify: `src/thinfloe-menu/src/generated.rs` (`Table::PublicIp => &[]`)
- Modify: `src/thinfloe-menu/src/jobs.rs` (`Job::PublicIpSave`, `JobResult::PublicIpSaved`, `public_ip_save`; `network_apply` and `net_take_over` keep the public IP keys)
- Modify: `src/thinfloe-menu/src/network.rs` (`NetEvent`, `NetRequest`, `NetTab` state)
- Modify: `src/thinfloe-menu/src/network_ui.rs` (band)
- Modify: `src/thinfloe-menu/src/menu.rs`, `src/thinfloe-menu/src/app.rs`, `src/thinfloe-menu/src/main.rs` (wiring; initial value from `opened.store.cfg.network`)
- Create: `src/thinfloe-menu/src/publicip.rs` (only `DEFAULT_URL`, `url_ok`, `PublicIpPrefs` in this task)

**Interfaces:**
- Produces:
  - `NetworkCfg { …, pub public_ip: bool, pub public_ip_url: String }` (serde default `false`, `""`).
  - `impl NetworkCfg { pub fn with_public_ip_of(self, current: &NetworkCfg) -> NetworkCfg }` — copies both keys from `current`.
  - `publicip::DEFAULT_URL: &str = "https://api.ipify.org"`; `pub fn url_ok(url: &str) -> bool` — starts with `https://` (ASCII case-insensitive), a non-empty host, at most 256 bytes, no whitespace or control character; `pub fn effective_url(cfg_url: &str) -> &str` (empty → `DEFAULT_URL`).
  - `#[derive(Clone, Debug, PartialEq, Eq, Default)] pub struct PublicIpPrefs { pub on: bool, pub url: String }`.
  - `NetEvent::PublicIp(bool)`, `NetEvent::PublicIpUrlText(String)`, `NetEvent::PublicIpUrlCommit`; `NetRequest::SavePublicIp(PublicIpPrefs)`; `NetTab::set_public_ip(PublicIpPrefs)`, `NetTab::public_ip(&self) -> &PublicIpPrefs`, `NetTab::public_ip_url_text(&self) -> &str`, `NetTab::public_ip_url_refused(&self) -> bool`.
  - `Job::PublicIpSave(PublicIpPrefs)` → `jobs::public_ip_save(paths: &Paths, prefs: &PublicIpPrefs) -> Result<(), String>` through `store.update(Table::PublicIp, …)`; `JobResult::PublicIpSaved(Result<(), String>)`; an error goes to `NetTab.error`.
- UI: a band "Public IP address" after "DNS", outside the Apply-disabled group: a switch "Show the public IP address"; below it a text field "Lookup address", hint `https://api.ipify.org`; while the typed text is neither empty nor `url_ok`, a muted note "Only https:// addresses are used." and nothing is saved.

- [ ] **Step 1: Failing tests**
  - `config.rs` `each_table_round_trips` gains `public_ip = true`, `public_ip_url = "https://ip.example.net"`; `a_config_without_public_ip_keys_has_it_off`.
  - `publicip.rs` `only_https_lookup_addresses_are_accepted`: true for `https://api.ipify.org`, `HTTPS://ip.example.net/plain`; false for `http://api.ipify.org`, `https://`, `file:///etc/passwd`, `https://a b`, `https://a\n`, `-K/etc/x`, a 257-byte https address.
  - `network.rs` `the_switch_saves_at_once_and_the_address_on_commit`: `PublicIp(true)` → `Some(NetRequest::SavePublicIp(PublicIpPrefs { on: true, url: "" }))`; `PublicIpUrlText("http://x")` → `None`, `public_ip_url_refused()`; `PublicIpUrlCommit` → `None`; `PublicIpUrlText("https://ip.example.net")` then `PublicIpUrlCommit` → save with that url.
  - `jobs.rs` `a_network_apply_keeps_the_public_ip_settings_set_while_it_ran`: with the existing fake network tool answering `apply` with success, call `network_apply` with an `on_spawn` closure that runs `public_ip_save(&paths, &PublicIpPrefs { on: true, url: "https://ip.example.net".into() })` (the store is not locked while the tool runs); afterwards `cfg.network.public_ip == true` and the url is kept. `config.rs` `with_public_ip_of_copies_both_keys` covers the copy used by `net_take_over`.
  - `jobs.rs` `a_public_ip_save_writes_only_config_toml`: `generated/` unchanged.
- [ ] **Step 2: Run** — fail.
- [ ] **Step 3: Implement.** In `network_apply` both the staged and the committed candidate use `net.clone().with_public_ip_of(&s.cfg.network)` under the lock at that moment; `net_take_over` does the same inside its `update` closure.
- [ ] **Step 4: Run** the gates.
- [ ] **Step 5: Commit** `feat(menu): Network tab switch for the public IP address`.

---

### Task 6: The public IP lookup and the `Public` row

**Files:**
- Create: `src/thinfloe-menu/src/quiet.rs`
- Modify: `src/thinfloe-menu/src/publicip.rs` (lookup, schedule, thread)
- Modify: `src/thinfloe-menu/src/status/mod.rs` (`Data.public_ip`, `Area::places_element`)
- Modify: `src/thinfloe-menu/src/status/connection.rs` (the `Public` row)
- Modify: `src/thinfloe-menu/src/app.rs` (`Samplers` gains the lookup thread), `src/thinfloe-menu/src/menu.rs` (`public_ip` answer, `status_sources`), `src/thinfloe-menu/src/main.rs` (`curl_cmd` from `TF_CURL_CMD`, default `curl`)

**Interfaces:**
- Consumes: `PublicIpPrefs`, `url_ok`, `effective_url` (Task 5); `connection::rows` (Task 2), which becomes `rows(snap: &Snapshot, public: Option<&publicip::Answer>) -> Vec<Row>`.
- Produces:
  - `quiet::run(cmd: &[String], args: &[String], stdin: Option<&[u8]>, cap: usize, timeout: Duration) -> Result<Quiet, String>`; `pub struct Quiet { pub stdout: Vec<u8>, pub status: Option<i32> }`. Child in its own process group (`process_group(0)`), stderr null, stdout null when `cap == 0`; more than `cap` bytes or past `timeout` → SIGKILL to the group and `Err`.
  - `publicip::Answer { Addr(IpAddr), Failed }`; `pub fn parse_answer(body: &[u8]) -> Option<IpAddr>` (trimmed ASCII, at most 64 bytes); `pub fn curl_args(url: &str) -> Vec<String>` (the args in "Decisions"); `pub fn lookup(curl: &[String], url: &str, timeout: Duration) -> Result<IpAddr, String>` — checks `url_ok` before spawning; splits the last line as the response code, only `200` accepted.
  - `pub type RouteKey = (Option<String>, Option<Ipv4Addr>, Option<Ipv4Addr>)` (iface, gateway, IPv4) and `pub fn route_key(info: &netstat::Info) -> RouteKey`.
  - `pub struct Want { pub prefs: PublicIpPrefs, pub placed: bool, pub sampling: bool, pub route: RouteKey }`; `pub fn active(&self) -> bool` = `prefs.on && placed && sampling`.
  - `pub struct Schedule`; `pub fn due(&mut self, want: &Want, now: Instant) -> bool` — when active: the first time, when `route` differs from the last lookup's, when `EVERY = 15 min` passed; never when inactive; `prefs.on == false` or a changed `prefs.url` resets it.
  - `pub fn spawn(curl: Vec<String>, want: Arc<Mutex<Want>>, feed: Arc<Feed>, wake: impl Fn() + Send + 'static) -> Arc<Mutex<Option<Answer>>>` — checks every second; clears the answer while `prefs.on` is false; logs one warn line `public IP lookup failed: <reason>` (source `public-ip`) when the new state is `Failed` and the last was not; calls `wake` when the answer changes.
  - `Area::places_element(&self, id: &str, short: bool) -> bool`.
  - `Data.public_ip: Option<&'a publicip::Answer>` — `Some` only while the switch is on and an answer exists. `Public 203.0.113.7`; `Failed` → `Public –`.
  - App: the thread starts the first time `prefs.on` is set (never again); each frame sets `Want` from `menu.network.public_ip()`, `places_element("connection", short)`, the network sampler's `on` flag and `route_key` of the latest snapshot.

- [ ] **Step 1: Failing tests**
  - `quiet.rs`: `a_tool_past_its_timeout_is_killed` (`sleep 5`, timeout 200 ms → `Err` within 1 s), `more_than_cap_is_an_error`, `stdin_reaches_the_tool` (`cat` with cap 16).
  - `publicip.rs`: `public_ip_answers_parse_only_as_an_address` (`b" 203.0.113.7\n"` → v4; `b"2001:db8::1"` → v6; `b"<html><body>Login</body></html>"`, `b""`, 65 bytes of digits, `b"203.0.113.7 extra"` → `None`).
  - `a_lookup_refuses_a_bad_address_without_running_curl`: fake curl script in a `TestDir` touches a marker; `lookup(.., "http://x", ..)`, `"file:///etc/passwd"`, `"-K/etc/x"` → `Err`, marker absent.
  - `the_lookup_follows_no_redirect`: `curl_args` contains `--max-redirs`,`0`, `--proto`,`=https`, `--url`; no `-L`/`--location`; fake curl printing `203.0.113.7\n301` → `Err`; printing `203.0.113.7\n200` → `Ok(203.0.113.7)`.
  - `the_lookup_runs_only_while_wanted` (counts `due` calls that return true, stepping 1 s over t in [0, 3600) s): off → 0; on but `placed: false` → 0; on but `sampling: false` → 0; active → 4 (t0, 15, 30, 45 min); a route change at 5 min → one more; off at 20 min and on at 21 min → one at 21 min.
  - `connection.rs`: `the_public_row_shows_the_answer_and_drops_first` (`Public 203.0.113.7` last; `Failed` → `Public –`; in the 2x1 size at the small height `Public` is the first row gone); `connection_draws_inside_its_rect_at_every_size` with a long IPv6 answer.
  - `status/mod.rs` `places_element_follows_the_planned_cells` (shipped: `connection` true, short false; `-connection` false).
  - `app.rs` sampler test (pattern of the existing `Samplers::new` tests ~1827): with the switch off no lookup thread is spawned.
- [ ] **Step 2: Run** — fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the gates.
- [ ] **Step 5: Commit** `feat(menu): opt-in public IP lookup for the connection tile`.

---

### Task 7: Network tab — MAC, gateway, WiFi chip and driver, "DNS in use"

**Files:**
- Create: `src/thinfloe-menu/src/pciids.rs`, `src/thinfloe-menu/src/netdetail.rs`
- Modify: `src/thinfloe-menu/src/jobs.rs` (`Paths` gains `net_roots: netstat::Roots`, `pci_ids: PathBuf`; `JobResult::NetStatus` carries details)
- Modify: `src/thinfloe-menu/src/app.rs` (`Job::NetStatus` runs `netdetail::read` after `network_status`), `src/thinfloe-menu/src/main.rs` (paths)
- Modify: `src/thinfloe-menu/src/network.rs` (`NetTab::status_done` stores `Details`; `details()` getter), `src/thinfloe-menu/src/nettab.rs` (`detail_line`, `dns_in_use_line`), `src/thinfloe-menu/src/network_ui.rs` (`connections`)

**Interfaces:**
- Produces:
  - `pciids::name(path: &Path, vendor: u16, device: u16) -> Option<String>` — streaming, 4 MiB cap; `"<vendor name> <device name>"`, or the vendor name alone when the device is not listed.
  - `netdetail::IfDetail { pub name: String, pub mac: Option<String>, pub gateway: Option<Ipv4Addr>, pub chip: Option<String>, pub driver: Option<String> }`; `netdetail::Details { pub ifaces: Vec<IfDetail>, pub dns_in_use: Vec<String> }`.
  - `netdetail::read(roots: &netstat::Roots, ifaces: &[nettab::Iface], pci_ids: &Path) -> Details` — MAC from `class/net/<if>/address` (all-zero dropped); gateway from `netstat::default_route` when its iface is this one; for `IfKind::Wifi` only: driver = file name of `device/driver` link, chip = PCI `device/vendor`+`device/device` through `pciids::name`, else `NNNN:NNNN`; USB (`device/../idVendor`, `idProduct`) → `USB 0bda:8179`. `dns_in_use` = `netstat::dns_in_use(roots)`.
  - `JobResult::NetStatus(Result<Vec<Iface>, String>, netdetail::Details)`.
  - `nettab::detail_line(d: &IfDetail) -> Option<String>` → e.g. `MAC 3c:52:82:aa:bb:cc · gateway 192.168.1.1 · Intel Corporation Wi-Fi 6 AX201 · iwlwifi`; `None` when empty. `nettab::dns_in_use_line(dns: &[String]) -> String` → `DNS in use 192.168.1.1, 9.9.9.9` or `DNS in use: none`.
  - UI: in "Connections now", a muted `detail_line` under each `iface_line`, then the `dns_in_use_line` row.

- [ ] **Step 1: Failing tests**
  - `pciids.rs` `names_come_from_the_list_else_none` (fixture text with `8086  Intel Corporation` / `\ta0f0  Wi-Fi 6 AX201`, a subsystem line `\t\t…`, a comment; unknown device → vendor only; unknown vendor → `None`; missing file → `None`).
  - `netdetail.rs` `a_wifi_adapter_names_its_chip_and_driver` (fake `sys/class/net/wlan0/{address,device/vendor,device/device}`, `device/driver` → symlink `…/iwlwifi`), `a_usb_adapter_shows_its_ids`, `the_gateway_belongs_to_the_default_route_interface` (fake `proc/net/route`), `a_wired_interface_has_no_chip_line`.
  - `nettab.rs` `dns_in_use_from_a_fake_resolv_conf` (Roots under a `TestDir` with `run/systemd/resolve/resolv.conf` → line lists its servers; only `etc/resolv.conf` → that file's), `detail_line_joins_what_is_known`.
- [ ] **Step 2: Run** — fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the gates.
- [ ] **Step 5: Commit** `feat(menu): Network tab shows MAC, gateway, WiFi chip and DNS in use`.

---

### Task 8: Screen tab "Graphics" band

**Files:**
- Create: `src/thinfloe-menu/src/graphics.rs`
- Modify: `src/thinfloe-menu/src/jobs.rs` (`Job::Graphics`, `JobResult::Graphics(graphics::Info)`, `Paths.vainfo_cmd`), `src/thinfloe-menu/src/app.rs`, `src/thinfloe-menu/src/main.rs` (`TF_VAINFO_CMD`, default `vainfo`)
- Modify: `src/thinfloe-menu/src/menu.rs` (`graphics: Option<graphics::Info>`, asked once when `settings_tab` becomes `Screen`)
- Modify: `src/thinfloe-menu/src/ui.rs` (`screen_tab`: `graphics_band` after the monitors band)

**Interfaces:**
- Consumes: `pciids::name` (Task 7), `quiet::run` (Task 6).
- Produces:
  - `graphics::Gpu { pub card: String, pub ids: (u16, u16), pub name: Option<String>, pub driver: Option<String> }`; `pub fn gpus(sys: &Path, pci_ids: &Path) -> Vec<Gpu>` — `class/drm/card<N>` only (not `card0-HDMI-A-1`), `device/vendor`, `device/device`, `device/driver` link name; one entry per device.
  - `graphics::Codec { pub name: &'static str, pub decode: bool, pub encode: bool }`; `pub fn parse_vainfo(text: &str) -> Vec<Codec>` — `VAProfileH264*` → `H.264`, `VAProfileHEVC*` → `HEVC`, `VAProfileVP9*` → `VP9`, `VAProfileAV1*` → `AV1`; `VAEntrypointVLD` → decode, `VAEntrypointEncSlice*` → encode; order H.264, HEVC, VP9, AV1; absent codecs left out.
  - `pub enum Vaapi { Codecs(Vec<Codec>), NotAvailable }`; `pub fn vaapi(cmd: &[String], timeout: Duration) -> Vaapi` (5 s in production; non-zero exit, timeout or no codec → `NotAvailable`).
  - `pub struct Info { pub gpus: Vec<Gpu>, pub vaapi: Vaapi }`; `pub fn codec_line(c: &Codec) -> String` → `H.264 decode, encode`.
  - Band "Graphics": per GPU `GPU <name or ids>` and `Driver <driver>`; then the codec lines, or `VA-API not available`; no GPU → `No graphics device found`; before the job answers → `Reading…`.

- [ ] **Step 1: Failing tests**: `vainfo_profiles_become_codec_names` (fixture of a real Intel `vainfo` output with H264/HEVC/VP9 VLD and H264 EncSlice → three codecs, H.264 decode+encode), `a_slow_vainfo_is_not_available` (fake script `sleep 5`, timeout 200 ms), `a_failing_vainfo_is_not_available` (exit 1), `gpus_come_from_drm_cards` (fake sysfs `card0` + `card0-HDMI-A-1` → one GPU, name from a pci.ids fixture, driver `i915`), `a_machine_without_drm_cards_has_no_gpu`, `without_pci_ids_the_gpu_shows_its_ids`; `menu.rs` `the_graphics_are_read_once_when_the_screen_tab_first_opens` (switching to Screen twice → one `Job::Graphics`).
- [ ] **Step 2: Run** — fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the gates.
- [ ] **Step 5: Commit** `feat(menu): Screen tab shows the graphics device and its video decoding`.

---

### Task 9: "This machine" tab with Copy

**Files:**
- Create: `src/thinfloe-menu/src/machine.rs`, `src/thinfloe-menu/src/machine_ui.rs`
- Modify: `src/thinfloe-menu/src/settings.rs` (`SettingsTab::Machine`, label `"This machine"`, last in `ALL`; `SettingsEvent::MachineCopy`)
- Modify: `src/thinfloe-menu/src/hardware.rs` (`pub fn cpu_model(cpuinfo: &str) -> Option<String>`, `pub fn cpu_count(cpuinfo: &str) -> Option<usize>`)
- Modify: `src/thinfloe-menu/src/menu.rs` (`machine: MachineTab`, `set_header`), `src/thinfloe-menu/src/main.rs` (`set_header(header.clone())`, `TF_WL_COPY_CMD` default `wl-copy`), `src/thinfloe-menu/src/jobs.rs` (`Job::MachineFacts`, `Job::Copy(String)`, results, `Paths.wl_copy_cmd`, `Paths.hardware_roots`), `src/thinfloe-menu/src/app.rs` (run the jobs; `machine.next_change` in the wake chain beside `network_wake`), `src/thinfloe-menu/src/ui.rs` (dispatch to `machine_ui::machine_tab`)

**Interfaces:**
- Consumes: `hardware::machine`, `hardware::parse_meminfo`, `hardware::parse_uptime`, `status::hardware::uptime`, `status::hardware::usage` style for RAM, `quiet::run`.
- Produces:
  - `machine::Facts { pub maker, pub model, pub cpu_model: Option<String>, pub cores: Option<usize>, pub ram_total: Option<u64>, pub kernel: Option<String>, pub uptime: Option<u64> }`; `pub fn read(roots: &hardware::Roots) -> Facts` (kernel from `proc/sys/kernel/osrelease`).
  - `pub fn rows(f: &Facts, uptime_now: Option<u64>, header: &Header, usb_boot: bool, addons: &[(String, String)]) -> Vec<(String, String)>` — labels in this order: `Maker`, `Model`, `CPU model`, `Cores`, `RAM`, `ThinFloe version`, `Kernel`, `Boot` (`USB` / `Netboot`), `Hostname`, `Uptime`, then one row per installed add-on (tile name, `AddonStatus.installed`); unknown values left out.
  - `pub fn clipboard_text(rows: &[(String, String)]) -> String` — `Label: value\n` per row.
  - `pub struct MachineTab`; `pub fn poll(&mut self, open: bool) -> Option<Job>` (`MachineFacts` on each opening of the tab), `pub fn facts_done(&mut self, f: Facts, now: Instant)`, `pub fn uptime_at(&self, now: Instant) -> Option<u64>` (reading plus elapsed), `pub fn next_change(&self, now: Instant) -> Option<Duration>` (next whole minute of uptime while open), `pub fn copy_done(&mut self, r: Result<(), String>)`.
  - `jobs::copy_text(cmd: &[String], text: &str) -> Result<(), String>` — `quiet::run` with `stdin`, cap 0, 3 s.
  - UI: one band, label/value rows in the Screen tab's form-row style, a `Copy` button (secondary); after a copy a muted `Copied` or the warn text of the error.

- [ ] **Step 1: Failing tests**: `machine.rs` `rows_are_in_order_and_unknown_ones_left_out` (full facts → the 10 labels then `ThinLinc 4.19.0`; no maker → no `Maker` row; netboot → `Boot Netboot`), `the_clipboard_text_is_label_colon_value_lines`, `facts_come_from_proc_and_sys` (fake `proc/cpuinfo` with 4 `processor` lines and `model name : Intel(R) Core(TM) i5-7260U`, `proc/meminfo`, `proc/sys/kernel/osrelease`, DMI files); `hardware.rs` `cpu_model_and_count_from_cpuinfo`; `menu.rs` `the_machine_tab_reads_its_facts_when_it_opens` and `copy_sends_the_rows_to_the_clipboard` (`SettingsEvent::MachineCopy` → `Effect::Start(Job::Copy(text))` with the `clipboard_text`); `jobs.rs` `copy_text_feeds_the_tool_its_stdin` (fake `wl-copy` script `cat > file`); `settings.rs` `the_tabs_end_with_this_machine`.
- [ ] **Step 2: Run** — fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the gates.
- [ ] **Step 5: Commit** `feat(menu): This machine tab with a Copy button`.

---

### Task 10: Image, full tests, notes and changelog

**Files:**
- Modify: `docs/notes/status-area.md` (element table, default list and its packing, short screen, connection rows, clock zone, temperatures with fans, retired ids, hidden-by-default flag, public IP lookup and its run conditions; remove Peak/Total/Since and the session total)
- Modify: `docs/notes/menu.md` (Network tab additions, Graphics band, "This machine" tab, public IP switch)
- Modify: `CHANGES.md` under `## [Unreleased]`
- Modify: `CLAUDE.md` only where a pointer changed (the "Where code lives" line for the status area if `network_info` is named there; none expected)

- [ ] **Step 1: Load the skill `repo-infra:writing-style`** before writing any prose.
- [ ] **Step 2: Notes.** Update both notes files; keep them describing how it works now, not the history.
- [ ] **Step 3: CHANGES.md** under `### New` / `### Changed`, user-facing, no implementation names, at most three sentences each, most important first. Entries cover: the Connection tile replacing network details (Down, Up, Peak, Total, Since gone; gateway and DNS now on the Network tab; a saved layout keeps its place); the clock's zone line; fans with the temperatures and the retired "Fans, uptime, model" tile; the opt-in public IP address with its lookup address; the Network tab's MAC, gateway, WiFi chip and "DNS in use"; the Screen tab's Graphics band; the "This machine" tab with Copy; the traffic graph's 2x1 size on a short screen.
- [ ] **Step 4: Unit tests:** `make test-unit` — all Python and cargo tests pass.
- [ ] **Step 5: Image and integration tests:**

```bash
flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make image VERSION=status
flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test VERSION=status
flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make test-runtime VERSION=status
```

Pass `timeout: 600000` on each call and wait for it to end. Expected: every test passes; `test-image-size.sh` unchanged (no new package). `test-runtime` is needed because `test-labwc-runtime.sh` changed in Task 3.
- [ ] **Step 6: Commit** `docs: status redesign part 1 in the notes and the changelog`.
