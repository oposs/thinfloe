# Network tab Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Settings screen gets a Network tab: it shows the connections, sets the wired address (DHCP or fixed), saves and forgets WiFi networks, and applies the change at once and at the next boot.

**Architecture:** The menu reads and writes `~/wifi.yaml` and `~/ethernet.yaml` itself (`netfiles.rs`, with `yaml-rust2` and the ownership rule of `owned.rs`), and checks each new file with `netplan generate --root-dir` as tfuser before it writes it. A new Python root tool `thinfloe-network`, always called with `sudo -n`, lists the interfaces, scans for WiFi, and installs the home files into `/etc/netplan` and runs `netplan apply`. In the menu, `nettab.rs` holds the tool's JSON and the input checks, `network.rs` the tab's state machine (the pattern of `upgrade.rs`: events in, requests out), `network_ui.rs` draws it. The initramfs installs `~/ethernet.yaml` next to `~/wifi.yaml`.

**Tech Stack:** Rust (egui 0.37, shadcn-egui 0.8, `yaml-rust2` 0.13); Python 3 stdlib; netplan.io 1.2, `iw`, `ip -j`, `wg`; POSIX sh (initramfs); bash test scripts.

**Spec:** `docs/superpowers/specs/2026-11-02-thinfloe-settings-and-writer-design.md`, §"Part C: Network", §"Who owns which file", "Settled after the builder check" 2, 3, 9, 10, "Settled after the pushback review" 8 (netboot), 9, 11.

## Global Constraints

- Owner header, exact: `# Written by the ThinFloe menu. Edits here are replaced.` (`owned::HEADER`).
- Hand-file text: `This is set by hand in ~/<file>`; button `Let the menu manage it`; rename to `<file>.by-hand`; an existing `.by-hand` refuses with `~/<file>.by-hand exists already`.
- The shipped template counts as no file: a `~/wifi.yaml` that contains `device name of your wifi card`, is empty, or holds only comments and blank lines. The menu's own file never contains that string.
- `~/ethernet.yaml` is the shipped `/etc/netplan/ethernet.yaml` with only the addressing changed: id `eth`, `match: name: 'en*'`, `optional: true` stay (the `10-netplan-eth.network.d/50-timezone.conf` drop-in depends on id `eth`).
- WiFi: open, WPA2-PSK and WPA3-SAE. A plain `password` covers WPA2 and WPA3 (netplan 1.2 renders `key_mgmt=WPA-PSK WPA-PSK-SHA256 SAE`), so the menu writes only `password` and `hidden`, never `auth`. Password: 8 to 63 characters, or 64 hex digits.
- Netboot: *Fixed address* is not offered; the root tool ignores a home `ethernet.yaml` on netboot; the initramfs does not install it on netboot.
- Apply while ThinLinc (`10-thinlinc`) or OxuLnk (`20-oxulnk`) runs asks first: `The remote session may disconnect for a few seconds.`
- Password note under the field, exact: `The password is saved with the home directory. Anyone who has the stick can read it.`
- After an Apply or a take-over the menu queues a home save (the Save-On-Exit queue); none after a failed restore.
- The menu runs as tfuser and writes only in the home. Root work only through `sudo -n /usr/bin/thinfloe-network status|scan|apply`; the root tool takes no argument from the menu beyond the subcommand and reads no password from it.
- Style: lyra, nothing rounded, no colour or size literal in UI code (grep guard with its positive control stays clean), body font Ubuntu Light. Never edit the shadcn-egui repository: a missing component means stop and ask the owner.
- Machine: at most 4 cores; menu tests under `systemd-run --user --scope -q -p MemoryMax=4G --`; builds `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=<tag> …` with `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests`; one `VERSION` per checkout; worktree builds use the `artifacts.link` trick from the handoff.

## Checked before writing this plan (2026-11-08, image `thinfloe-ubuntu:3.0.0`)

1. **netplan refuses `match` for WiFi:** `wifi: networkd backend does not support wifi with match:, only by interface name`. So the menu writes the device name (Settled 3, its fallback). With a name, `netplan generate` makes `netplan-wpa-<dev>.service`.
2. `network: {version: 2, renderer: networkd}` with no `wifis` passes; a `wifis` entry with no access points does not. A password of 5 characters fails (`ASCII passphrase must be between 8 and 63 characters`).
3. `netplan generate --root-dir <dir>` works as tfuser and exits 1 on a bad file.
4. `yaml-rust2` 0.13.0 (crates.io, 2026-09-11) is the YAML crate: maintained, pure Rust, loader and emitter. `serde_yaml` is archived.

## Owner decisions for this plan (2026-11-08)

- YAML is read and written in the menu, with a Rust crate; one ownership rule, in `owned.rs`.
- `status` runs with `sudo -n` every 2 s while the tab is open, like the Upgrade drive poll, and shows the WireGuard handshake.
- Accepted: the check before any write (call 1), `optional: true` (call 3), no `~/ethernet.yaml` on netboot, the draft dropped on *Back* (call 5).

## Calls made in this plan (the owner may overrule)

1. **Check, then write, then install.** Apply is one job: the menu renders the new files, runs `netplan generate --root-dir` on them in a scratch dir (with the wired file in force, so the set is checked as a whole), writes them to the home only when that passes, then calls `sudo -n thinfloe-network apply`, which checks the home files again and installs them. A file that fails the check is never written to the home.
2. **The default for *Automatic*** is a copy of the shipped file at `/usr/share/thinfloe/netplan/ethernet.yaml` (mode 644), made by the Dockerfile from `/etc/netplan/ethernet.yaml` (one source). The menu reads it to build a fixed-address file (`/etc/netplan/*.yaml` is mode 600, root only); *Automatic* removes a menu-owned `~/ethernet.yaml`, and `apply` installs the copy.
3. **The menu WiFi file sets `optional: true`** on the WiFi definition: without it netplan adds the WiFi device to `systemd-networkd-wait-online` (the comment in the shipped `ethernet.yaml` shows the 120 s stall), and a home moved to a machine without that device would wait.
4. **Device name:** the first entry, sorted, of `/sys/class/net/*/wireless`, read by the menu. When the menu file names a device the machine does not have, the tab says `WiFi adapter changed`, the draft counts as changed, and Apply writes the current name. Networks but no adapter: Apply fails with `No WiFi adapter found` and writes nothing.
5. **The draft:** changes collect until Apply. Switching tabs keeps them; *Back* drops them with the log line `Network changes that were not applied were dropped.`
6. **Saving a network with a name already saved** replaces its password and hidden flag; one row per name.
7. **No status file for the root tool:** the menu waits for each call; the result is one JSON line on stdout and the exit code. Log lines go to stderr (the menu log) and to the journal (`syslog`, ident `thinfloe-network`).
8. **Signal words** for WiFi: `strong` (≥ −60 dBm), `good` (≥ −70 dBm), `weak` (below). **WireGuard** counts as `connected` with a handshake in the last 180 s, else `not-connected`; the line shows the age (`handshake 42 s ago`, `no handshake`).

## JSON contract of `thinfloe-network` (Tasks 3–4 produce it, Task 6 parses it)

- `status` → `{"interfaces": [{"name": str, "kind": "wired|wifi|wireguard", "state": "connected|no-cable|not-connected|off", "addresses": ["a.b.c.d/p"], "ssid": str|null, "signal_dbm": int|null, "handshake_age_s": int|null}]}`.
- `scan` → `{"device": str, "networks": [{"ssid": str, "signal_dbm": int, "secured": bool, "supported": bool}]}`, strongest first, one entry per name, at most 64.
- `apply` → `{"result": "ok"|"failed", "reason": str}`. Exit codes: 0 ok, 1 failed, 2 misuse (not root, unknown subcommand).

## Review Focus

1. **Odd characters.** An SSID or password with `"`, `\`, `:`, `#`, a leading space, or non-ASCII letters must reach `wpa-<dev>.conf` exactly as typed. → Task 2 `render_round_trips_odd_text`, Task 5 `odd text reaches wpa conf`.
2. **A file that fails `netplan generate`** must change nothing: not the home, not `/etc/netplan`, and the tab shows the reason. → Task 2 `save_refuses_what_netplan_refuses`, Task 4 `test_apply_rejects_bad_home_file`, Task 7 `apply_failure_keeps_the_draft`.
3. **A home moved to another machine** (other WiFi device name, or none): boot does not wait, the tab says `WiFi adapter changed`, Apply fixes it. → Task 2 `render_wifi_is_optional` and `save_writes_the_current_device`, Task 7 `adapter_changed_marks_the_draft_dirty`.
4. **A hand-written or symlinked `~/wifi.yaml` / `~/ethernet.yaml`** is never written, renamed or removed except by *Let the menu manage it*; root never follows a link in the home. → Task 2 `save_never_touches_hand_or_link`, Task 4 `test_apply_refuses_linked_home_file`.
5. **A scan that cannot run** (no adapter, adapter down, `Device or resource busy`, a hanging `iw`): the dialog shows a reason and can be closed; nothing hangs. → Task 3 `test_scan_brings_link_up_and_retries_busy`, `test_scan_times_out`; Task 7 `scan_failure_shows_in_dialog`.

---

### Task 1: The initramfs installs `~/ethernet.yaml`; the image keeps the DHCP default

**Files:**
- Modify: `overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh` (`install_local_network`, lines 774–837)
- Modify: `overlay/usr/lib/dracut/modules.d/90thinfloe/setup-overlay.sh:77-79`
- Modify: `containers/ubuntu/Dockerfile` (the overlay `RUN` near line 281)
- Test: `test/integration/test-initramfs-home.sh` (the `install_local_network` block, line 410 on)

**Interfaces:**
- Produces: `install_local_network <newroot> [<ramroot-url>]`; a remote URL (`is_remote_url`) means netboot. `/usr/share/thinfloe/netplan/ethernet.yaml` (mode 644, byte-equal to the shipped `/etc/netplan/ethernet.yaml`).
- Behaviour: when `~/ethernet.yaml` is a regular file (not a link) and the boot is not netboot, copy it over `$root/etc/netplan/ethernet.yaml`, mode 600, log `thinfloe: installing ethernet.yaml into /etc/netplan`. Netboot: log `thinfloe: netboot, ~/ethernet.yaml not installed`, leave the shipped file. Link or absent: `thinfloe: no usable ~/ethernet.yaml, skipping`. Still always returns 0; the wifi, ethernet and wg0 halves stay independent. `setup-overlay.sh` passes `"$(cat /tmp/thinfloe-ramroot 2>/dev/null)"`. The comment block names the new file and why netboot skips it.

- [ ] **Step 1: Write the failing cases** in `test-initramfs-home.sh`: `ethernet.yaml installed with mode 600`, `ethernet.yaml symlink refused (shipped file unchanged)`, `no ethernet.yaml leaves the shipped file`, `netboot skips ethernet.yaml but installs wifi.yaml`, `a failing ethernet copy still installs wg0`. (The Dockerfile copy is checked in the built image by Task 5.)
- [ ] **Step 2: Run, expect the new cases to FAIL:** `test/integration/test-initramfs-home.sh`
- [ ] **Step 3: Implement**; Dockerfile: `&& install -D -m 644 /etc/netplan/ethernet.yaml /usr/share/thinfloe/netplan/ethernet.yaml` after the `chmod 600`.
- [ ] **Step 4: Run, expect all `ok`.** `sh -n` on both shell files.
- [ ] **Step 5: Commit** `initramfs: install ~/ethernet.yaml, except on netboot`.

### Task 2: The menu reads and writes the network files (`netfiles.rs`, `owned.rs`)

**Files:**
- Create: `src/thinfloe-menu/src/netfiles.rs`
- Modify: `src/thinfloe-menu/src/owned.rs`, `src/thinfloe-menu/src/lib.rs`, `src/thinfloe-menu/Cargo.toml` (`yaml-rust2 = "0.13"`), `Cargo.lock`

**Interfaces:**
- Produces in `owned.rs`:
  - `pub fn write_with(path: &Path, body: &str, replaceable: fn(&str) -> bool) -> io::Result<()>`: as `write`, but a `ByHand` file for which `replaceable(text)` is true may be replaced. `write` becomes `write_with(path, body, |_| false)`.
  - `pub fn remove(path: &Path) -> io::Result<()>`: removes a `Menu` file; `Missing` is fine; `ByHand` → `PermissionDenied`; a link is never followed (it counts as `ByHand`).
  - `read` already refuses to follow a link: keep, and pin it with a test (`read_refuses_a_link`).
- Produces in `netfiles.rs`:
  - `pub const WIFI_FILE: &str = "wifi.yaml"; pub const WIRED_FILE: &str = "ethernet.yaml"; pub const TEMPLATE_MARKER: &str = "device name of your wifi card"; pub const DEFAULT_WIRED: &str = "/usr/share/thinfloe/netplan/ethernet.yaml";`
  - `pub fn is_template(text: &str) -> bool` (marker, empty, or comments and blank lines only); `pub enum FileState { Missing, Template, Menu, Hand }`; `pub fn state(text: Option<&str>, template_rule: bool) -> FileState` on top of `owned::owner`.
  - `pub struct Net { pub ssid: String, pub hidden: bool, pub password: Option<String> }` (`None`: open network); `pub enum Wired { Dhcp, Static { address: Ipv4Addr, prefix: u8, gateway: Option<Ipv4Addr>, dns: Vec<Ipv4Addr> } }`.
  - `pub struct WifiFile { pub state: FileState, pub device: Option<String>, pub networks: Vec<Net>, pub manageable: bool, pub error: Option<String> }`; `pub struct WiredFile { pub state: FileState, pub wired: Wired, pub manageable: bool, pub error: Option<String> }`.
  - `pub fn parse_wifi(text: Option<&str>) -> WifiFile`. `manageable` (Settled after pushback 9): top keys ⊆ `{network}`, network ⊆ `{version, renderer, wifis}`, exactly one WiFi definition with keys ⊆ `{dhcp4, optional, access-points}`, each AP null, `{}` or keys ⊆ `{password, hidden}`. A hand file that is not manageable still lists its AP names (passwords are not kept). YAML that does not load → `error = Some("~/wifi.yaml: <loader message>")`, no networks.
  - `pub fn parse_wired(text: Option<&str>) -> WiredFile`: manageable when there is exactly one `ethernets` entry `eth` with keys ⊆ `{match, dhcp4, addresses, routes, nameservers, optional}`; `addresses[0]`, the `to: default` route and `nameservers.addresses` give `Static`, `dhcp4: true` with no addresses gives `Dhcp`.
  - `pub fn render_wifi(device: Option<&str>, networks: &[Net]) -> String`: header + YAML through `YamlEmitter`; no networks or no device: `network: {version: 2, renderer: networkd}` only; else one definition `<device>` with `dhcp4: true`, `optional: true`, `access-points` (an AP is `{}`, or holds `password` and/or `hidden: true`). Panics never; the output never contains `TEMPLATE_MARKER` (input checks refuse it).
  - `pub fn render_wired(default: &str, wired: &Wired) -> Result<Option<String>, String>`: `None` for `Dhcp`; for `Static`, `default` loaded, the `eth` definition gets `dhcp4: false`, `addresses: [cidr]`, `routes: [{to: default, via: gw}]` when a gateway is given, `nameservers: {addresses: [...]}` when DNS is given; every other key stays.
  - `pub fn wifi_devices(sys_net: &Path) -> Vec<String>` (sorted names with a `wireless` entry).
  - `pub fn check_netplan(netplan: &str, files: &[(&str, &str)]) -> Result<(), String>`: a fresh dir under `$TMPDIR` (else `$XDG_RUNTIME_DIR`), `etc/netplan/<name>` mode 600, `<netplan> generate --root-dir <dir>` with a 20 s timeout; `Err` holds the first `ERROR:` line (else the last stderr line); the dir is removed.
  - `pub struct Homes { pub home: PathBuf, pub sys_net: PathBuf, pub netplan: String, pub default_wired: PathBuf }` (from `$HOME`, `/sys/class/net`, `netplan`, `DEFAULT_WIRED`; overridable by `TF_NETWORK_SYS`, `TF_NETPLAN`, `TF_NETWORK_DEFAULT_WIRED` for tests, as `env_path` does).
  - `pub struct Change { pub wifi: Option<Vec<Net>>, pub wired: Option<Wired> }` (`None` = unchanged).
  - `pub fn load(h: &Homes) -> (WifiFile, WiredFile)`.
  - `pub fn save(h: &Homes, c: &Change, netboot: bool) -> Result<(), String>`: refuses a hand-owned target (`~/<file> is set by hand`), `Static` on netboot, and `No WiFi adapter found` (networks, no device); renders the WiFi file with the current `wifi_devices()[0]`; checks the new set with `check_netplan` (wired: the new menu file, else the old one, else the default); then `owned::write_with(.., is_template)` for WiFi, `owned::write` for wired, `owned::remove` for wired `Dhcp`. Nothing is written when any step before the first write fails.
  - `pub fn take_over(h: &Homes, which: &str /* "wifi" | "wired" */) -> Result<(), String>`: only when `manageable`; `owned::take_over`, then writes the menu file with the same networks and passwords (device kept) or the same addressing, checked like `save`.
- Input checks used by `save` and by the UI (Task 6 shows the messages): `pub fn check_ssid(s: &str) -> Result<(), String>` (1–32 bytes, no `TEMPLATE_MARKER`: `The name needs 1 to 32 bytes.` / `This name cannot be used.`), `pub fn check_password(p: &str) -> Result<(), String>` (`The password needs 8 to 63 characters.`; 64 hex digits pass).

- [ ] **Step 1: Write the failing tests** (`crate::testdir`, a fake `sys/class/net` dir, a fake `netplan` script that exits by a flag file):
  - In `owned.rs`: `write_with_replaces_only_what_it_may`, `remove_keeps_hand_and_link`, `read_refuses_a_link`.
  - `template_rules`: marker, empty, comments-only → `Template`; a header in line 1 → `Menu`; anything else → `Hand`; the rule off → `Hand`.
  - `render_wifi_is_optional`: `optional: true`, `dhcp4: true`, no `auth`, header first.
  - `render_wifi_without_networks_is_bare`.
  - `render_round_trips_odd_text`: SSIDs `Café "Nord"`, ` lead`, `a:b #c`, `back\slash`, `yes`, `123`; password `p@ss: "x" #1\`; `parse_wifi(Some(&render_wifi(..)))` gives them back exactly, all as strings.
  - `render_wired_keeps_eth_match_optional`: from the shipped file's text: id `eth`, `match.name == "en*"`, `optional == true`, `dhcp4 == false`, the default route.
  - `parse_wifi_manageable_rules` (an extra AP key `auth` → not manageable, names still listed; two definitions → not manageable; broken YAML → error).
  - `parse_wired_reads_back_render_wired`.
  - `save_keeps_unchanged_file_alone`, `save_writes_the_current_device` (old file names `wlp2s0`, machine has `wlp58s0`).
  - `save_refuses_what_netplan_refuses`: fake netplan prints `ERROR: x: bad` and exits 1 → `Err("ERROR: x: bad")`, home byte-equal.
  - `save_never_touches_hand_or_link`: a hand file and a symlinked file both byte-equal after `save`, `Err`.
  - `save_replaces_the_template`, `save_static_refused_on_netboot`, `save_dhcp_removes_menu_wired_only`, `save_without_adapter_fails`.
  - `take_over_keeps_networks_and_passwords`, `take_over_refuses_unmanageable`, `take_over_refuses_existing_by_hand`.
  - `input_checks`: password 7 → Err, 8 → Ok, 63 → Ok, 64 hex → Ok, 64 other → Err; SSID 33 bytes → Err; SSID with the marker → Err.
- [ ] **Step 2: Run, expect FAIL:** `cd src/thinfloe-menu && systemd-run --user --scope -q -p MemoryMax=4G -- cargo test --locked -j 4 netfiles:: owned::` (first `cargo add yaml-rust2@0.13`; `--locked` needs the updated `Cargo.lock`).
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS**; the whole crate stays green; the "no GPU library" guard in `test-session-packages.sh` still passes.
- [ ] **Step 5: Commit** `menu: read and write ~/wifi.yaml and ~/ethernet.yaml`.

### Task 3: `thinfloe-network status` and `scan`

**Files:**
- Create: `overlay/usr/bin/thinfloe-network` (mode 755, `#!/usr/bin/python3 -I`, importable like `thinfloe-addon`: nothing runs on import)
- Create: `test/unit/test_network.py` (loads the tool through `importlib`, as `test_addon.py` does)
- Create: `test/unit/network-fixtures/` (`ip-link.json`, `ip-addr.json`, `iw-link-connected.txt`, `iw-link-not.txt`, `iw-scan.txt`, `wg-handshakes.txt`), written to match `iproute2` 6.x / `iw` 6.x / `wg` output.

**Interfaces:**
- Produces: `parse_ip(link_json: str, addr_json: str, sys_net: str) -> List[dict]`, `parse_iw_link(text: str) -> Tuple[Optional[str], Optional[int]]`, `parse_handshake(text: str, now: int) -> Optional[int]`, `parse_scan(text: str) -> List[dict]`, `status() -> dict`, `scan() -> dict`, `main(argv: List[str]) -> int`; commands from `IP = os.environ.get("TF_NETWORK_IP", "ip")`, `IW = os.environ.get("TF_NETWORK_IW", "iw")`, `WG = os.environ.get("TF_NETWORK_WG", "wg")`, `SYS_NET = os.environ.get("TF_NETWORK_SYS", "/sys/class/net")`.
- Kind: `wifi` when `<sys>/<if>/wireless` exists; `wired` when `<sys>/<if>/device` exists and `type` is 1; `wireguard` when `ip -j -d link` reports `linkinfo.info_kind == "wireguard"`; everything else is left out. State: `off` without `UP` in flags; wired `connected` with `LOWER_UP`, else `no-cable`; WiFi `connected` when `iw dev <if> link` names an SSID, else `not-connected`; WireGuard per call 8 (`wg show <if> latest-handshakes`, 0 = none). Addresses: IPv4 `local/prefixlen`.
- `scan`: no WiFi device → exit 1 `No WiFi adapter found`. Link down → `ip link set <if> up` first. `iw dev <if> scan`, timeout `TF_NETWORK_SCAN_TIMEOUT` (default 20 s); `Device or resource busy` → wait 2 s, one retry. Parse BSS blocks: `SSID:` (decode `\xNN`; empty or all-NUL names dropped), `signal: -NN.NN dBm`, `RSN:` or `WPA:` → secured; `capability: … Privacy` without RSN/WPA → `supported: false` (WEP). Keep the strongest per name, sort strongest first, cut at 64.
- Both need root (exit 2 `needs root` otherwise; tests set `TF_NETWORK_ASSUME_ROOT=1`).

- [ ] **Step 1: Write the failing tests:** `test_parse_ip_kinds_and_states` (wired up, wired no cable, WiFi, wg0, `lo` and a bridge left out), `test_parse_iw_link`, `test_parse_handshake` (recent, old, 0), `test_parse_scan_dedupes_and_sorts`, `test_parse_scan_marks_wep_and_open`, `test_parse_scan_decodes_escapes`, `test_scan_brings_link_up_and_retries_busy` (fake `ip`/`iw` scripts that record calls; the first scan prints `command failed: Device or resource busy (-16)`), `test_scan_times_out` (fake `iw` sleeps 60 s; `TF_NETWORK_SCAN_TIMEOUT=1` → exit 1 within 3 s), `test_scan_without_adapter`, `test_status_needs_root`.
- [ ] **Step 2: Run, expect FAIL:** `python3 -B -m unittest test.unit.test_network -v` (from the repo root; `make test-unit` finds it too).
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS.**
- [ ] **Step 5: Commit** `thinfloe-network: list interfaces and scan for WiFi`.

### Task 4: `thinfloe-network apply`

**Files:**
- Modify: `overlay/usr/bin/thinfloe-network`, `test/unit/test_network.py`

**Interfaces:**
- Produces: `apply() -> Tuple[int, str]`; `HOME_DIR = os.environ.get("TF_NETWORK_HOME")` (unset: `getpwnam("tfuser").pw_dir`), `ETC_NETPLAN = os.environ.get("TF_NETWORK_ETC", "/etc/netplan")`, `DEFAULT_WIRED = os.environ.get("TF_NETWORK_DEFAULT_WIRED", "/usr/share/thinfloe/netplan/ethernet.yaml")`, `ESP_DIR = os.environ.get("TF_NETWORK_ESP", "/run/thinfloe/storage/efi")` (netboot: not a mount point), `NETPLAN = os.environ.get("TF_NETWORK_NETPLAN", "netplan")`, `LOCK = os.environ.get("TF_NETWORK_LOCK", "/run/thinfloe/network.lock")`.
- Behaviour: root only. Holds `LOCK` (`flock`, exclusive). Opens `~/wifi.yaml` and `~/ethernet.yaml` with `O_NOFOLLOW`, 64 KiB cap; a link fails the whole apply, nothing changed. Parses no YAML; only the template rule, by text (marker, empty, comments only), as in Task 2: `~/wifi.yaml` template or missing → leave `/etc/netplan/wifi.yaml` as it is; otherwise it is installed (a hand file too: it is what the next boot installs). Wired: home file when present and not netboot, else `DEFAULT_WIRED`. Check the set with `NETPLAN generate --root-dir <tmp>`; a failure → exit 1, the reason, nothing installed. Then write each into `ETC_NETPLAN` (tmp + rename, mode 600), run `NETPLAN apply` (120 s timeout). Every step logs one line (stderr + journal).

- [ ] **Step 1: Write the failing tests:** `test_apply_installs_home_files_mode_600`, `test_apply_template_leaves_etc_wifi`, `test_apply_dhcp_installs_default_copy`, `test_apply_rejects_bad_home_file` (fake netplan generate fails → `/etc` byte-equal, `apply` never called), `test_apply_refuses_linked_home_file`, `test_apply_netboot_ignores_home_wired`, `test_apply_reports_netplan_apply_failure` (exit 1, reason), `test_apply_needs_root`.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS.**
- [ ] **Step 5: Commit** `thinfloe-network: apply the home's network files`.

### Task 5: The real netplan, in the image

**Files:**
- Create: `test/integration/test-network-netplan.sh`
- Modify: `src/thinfloe-menu/src/netfiles.rs` (a fixture writer test), `Makefile` (`test:` gets `@TF_UBUNTU_IMAGE=$(UBUNTU_IMAGE) test/integration/test-network-netplan.sh` after `test-addon-deps.sh`)

**Interfaces:**
- Consumes: `render_wifi`, `render_wired` (Task 2), `apply` (Task 4).
- Produces: `#[test] fn write_netplan_fixtures()` in `netfiles.rs`: does nothing unless `TF_NETPLAN_FIXTURES` names a dir; then writes `wifi-two.yaml` (one hidden, one open network on `wlp58s0`), `wifi-odd.yaml` (the odd text of Task 2), `wifi-bare.yaml`, and `ethernet-static.yaml` (from the repo's `overlay/etc/netplan/ethernet.yaml`, `192.168.1.10/24`, gateway `192.168.1.1`, DNS `1.1.1.1`). The script runs it with `cargo test --locked -j 4 write_netplan_fixtures`, then mounts the dir into a throw-away container of the built image (`podman run --rm`).

- [ ] **Step 1: Write the cases** (each prints `ok   - …` / `FAIL - …`):
  - `menu wifi gives netplan-wpa-<dev>`: `netplan generate --root-dir` on `wifi-two.yaml` as tfuser → `netplan-wpa-wlp58s0.service`; `wpa-wlp58s0.conf` has `scan_ssid=1` for the hidden one and `key_mgmt=NONE` for the open one.
  - `odd text reaches wpa conf`: `wpa-wlp58s0.conf` from `wifi-odd.yaml` equals the conf from a hand-written file with the same names and passwords in plain double-quoted YAML.
  - `wifi is not waited for`: `systemd-networkd-wait-online.service.d/10-netplan.conf` does not list `wlp58s0`.
  - `bare wifi file passes`.
  - `static wired keeps id eth`: `10-netplan-eth.network` has `Address=192.168.1.10/24`, `Gateway=192.168.1.1`, `DNS=1.1.1.1`, `Name=en*`.
  - `dhcp default is the shipped file`: `/usr/share/thinfloe/netplan/ethernet.yaml` equals `/etc/netplan/ethernet.yaml`, mode 644.
  - `comments-only wifi.yaml passes as it is` (Settled after pushback 11).
  - `apply as root installs`: in the container as root, `TF_NETWORK_HOME` holding `wifi-two.yaml` as `wifi.yaml`, `TF_NETWORK_NETPLAN` a wrapper that passes `generate` through and records `apply` → `/etc/netplan/wifi.yaml` mode 600, equal to the home file, `apply` recorded.
- [ ] **Step 2: Run** `TF_UBUNTU_IMAGE=localhost/thinfloe-ubuntu:<tag> test/integration/test-network-netplan.sh` against an image built from this branch (`make VERSION=<tag> rootfs` through the build lock); expect all `ok`.
- [ ] **Step 3: Commit** `test: the menu's network files pass the image's netplan`.

### Task 6: Tool JSON and form checks (`nettab.rs`); the jobs (`jobs.rs`)

**Files:**
- Create: `src/thinfloe-menu/src/nettab.rs`
- Modify: `src/thinfloe-menu/src/lib.rs`, `src/thinfloe-menu/src/jobs.rs`

**Interfaces:**
- Produces in `nettab.rs`:
  - `pub enum IfKind { Wired, Wifi, Wireguard }`, `pub enum IfState { Connected, NoCable, NotConnected, Off }`, `pub struct Iface { pub name: String, pub kind: IfKind, pub state: IfState, pub addresses: Vec<String>, pub ssid: Option<String>, pub signal_dbm: Option<i32>, pub handshake_age_s: Option<u64> }`; `pub struct InRange { pub ssid: String, pub signal_dbm: i32, pub secured: bool, pub supported: bool }`.
  - `pub fn parse_status(json: &str) -> Result<Vec<Iface>, String>`, `pub fn parse_scan(json: &str) -> Result<Vec<InRange>, String>`, `pub fn parse_result(stdout: &str) -> Result<(), String>` (last JSON line; `failed` → `Err(reason)`). Untrusted input as in `drives.rs`: at most 64 rows, text cut to 64 chars, SSIDs to 32 bytes on a char boundary.
  - `pub fn parse_wired_form(address: &str, gateway: &str, dns: &str) -> Result<netfiles::Wired, String>`: address `a.b.c.d/p`, prefix 1–32, else `Address: use the form 192.168.1.10/24`; gateway empty or IPv4 inside the subnet, else `Gateway: not in the network of the address`; DNS split on commas and spaces, 0–3 IPv4, else `DNS servers: up to three IPv4 addresses`.
  - `pub fn signal_word(dbm: i32) -> &'static str` (call 8); `pub fn iface_line(i: &Iface) -> String`, for example `WiFi wlp58s0 · Home Net · strong · 192.168.1.23/24`, `Wired enp0s31f6 · no cable`, `WireGuard wg0 · connected · handshake 42 s ago · 10.0.0.2/32`.
  - `pub fn adapter_changed(wifi: &netfiles::WifiFile, devices: &[String]) -> bool`: a menu file with a device not in `devices`, and `devices` not empty.
- Produces in `jobs.rs`:
  - `fn capture(cmd: &[String], args: &[String], cap: usize, timeout: Duration, feed: &Arc<Feed>) -> Result<(String, Option<i32>), String>`: today's `upgrade_capture` renamed; the upgrade calls use it.
  - `Paths` gains `network_cmd: Vec<String>` (`sudo -n /usr/bin/thinfloe-network`), `homes: netfiles::Homes`; `pub fn default_network_cmd() -> Vec<String>`.
  - `pub fn network_status(paths: &Paths, feed: &Arc<Feed>) -> Result<Vec<Iface>, String>` (5 s), `network_scan(..) -> Result<Vec<InRange>, String>` (40 s), `pub fn network_apply(paths: &Paths, change: &netfiles::Change, netboot: bool, feed: &Arc<Feed>, on_spawn: impl FnOnce(i32)) -> Result<(), String>`: `netfiles::save`, then, only on success, `apply` in a process group of its own (`on_spawn` gets it; 150 s), `parse_result`.
  - `JobResult` gains `NetLoaded(netfiles::WifiFile, netfiles::WiredFile, Vec<String> /* wifi devices */)`, `NetStatus(Result<Vec<Iface>, String>)`, `NetScan(Result<Vec<InRange>, String>)`, `NetApplyStarted { pgid: i32 }`, `NetApplied(Result<(), String>)`, `NetTakenOver(Result<(), String>)`.

- [ ] **Step 1: Write the failing tests:** in `nettab.rs`: `parse_status_reads_the_contract`, `parse_status_refuses_junk` (not JSON, 65 rows, wrong types), `parse_scan_cuts_long_ssid`, `parse_result_reads_the_last_line`, `parse_wired_form_rules` (each message above), `signal_words`, `iface_lines`, `adapter_changed_rule` (menu file `wlp2s0`, machine `wlp58s0` → true; same device, hand file, or no device at all → false). In `jobs.rs`: `network_apply_skips_apply_after_failed_save` (fake netplan fails; a fake root tool records that it was not called).
- [ ] **Step 2: Run, expect FAIL:** `… cargo test --locked -j 4 nettab:: jobs::`
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS**; whole crate green.
- [ ] **Step 5: Commit** `menu: the network tool's JSON and the address checks`.

### Task 7: The Network tab's state machine (`network.rs`) and its place in `menu.rs`

**Files:**
- Create: `src/thinfloe-menu/src/network.rs`
- Modify: `src/thinfloe-menu/src/settings.rs` (`SettingsTab`, `SettingsEvent`), `src/thinfloe-menu/src/menu.rs`, `src/thinfloe-menu/src/lib.rs`

**Interfaces:**
- Consumes: Tasks 2 and 6; `Menu::request_save`; `addon_id`.
- Produces in `network.rs`:
  - `pub const POLL: Duration = Duration::from_secs(2);`
  - `pub enum NetEvent { WiredMode(bool /* static */), WiredText { address: String, gateway: String, dns: String }, Forget(String), OpenAdd, CloseAdd, Rescan, PickNetwork(String), AddText { ssid: String, password: String, hidden: bool }, Add, Apply, ConfirmApply, CancelApply, TakeOver(&'static str /* "wifi" | "wired" */) }`.
  - `pub enum NetRequest { Load, Status, Scan, Apply(netfiles::Change), TakeOver(&'static str) }`.
  - `pub struct NetTab` with `pub fn new(netboot: bool) -> NetTab`, `pub fn event(&mut self, e: NetEvent, remote: bool) -> Option<NetRequest>`, `pub fn poll(&mut self, now: Instant, open: bool) -> Option<NetRequest>` (`Load` when the tab first opens and after an apply or take-over, then `Status` every `POLL` while open and no status call is in flight), `pub fn loaded(&mut self, wifi: WifiFile, wired: WiredFile, devices: Vec<String>)`, `status_done`, `scan_done`, `pub fn apply_started(&mut self, pgid: i32)`, `pub fn apply_done(&mut self, r: Result<(), String>) -> bool /* true: save the home */`, `pub fn take_over_done(&mut self, r: Result<(), String>) -> bool`, `pub fn dirty(&self) -> bool`, `pub fn applying(&self) -> bool`, `pub fn pgid(&self) -> Option<i32>`, `pub fn drop_draft(&mut self) -> bool /* true: something was dropped */`, plus read accessors for the UI (loaded files, interfaces, draft, add-dialog view, `confirm_open()`, `error()`).
  - Rules: `Apply` with a draft that fails `parse_wired_form`, `check_ssid` or `check_password` sets the error and sends nothing. `Apply` with `remote` opens the confirmation; `ConfirmApply` sends. A hand-owned file has no draft part (its edit events are ignored). `WiredMode(true)` is ignored on netboot. `adapter_changed` makes `dirty()` true with an unchanged list. While `applying()` every edit event is ignored. `Add` replaces a saved network of the same name (call 6). After `apply_done(Ok)` the draft is cleared and a `Load` is due; after `Err` the draft stays and `error()` holds the reason.
- In `settings.rs`: `SettingsTab::Network` between Packages and Screen (`ALL` has 3 entries, label `Network`); `SettingsEvent::Network(NetEvent)`.
- In `menu.rs`: field `pub network: NetTab` (built with the netboot flag `set_usb_boot` already gives); `Job::{NetLoad, NetStatus, NetScan, NetApply { change: netfiles::Change }, NetTakeOver { which: &'static str }}`; `settings()` passes `Network(e)` on with `remote = self.remote_running()`, where `fn remote_running(&self) -> bool` is true when a `Running` job's tile has `addon_id` `10-thinlinc` or `20-oxulnk`; `tick()` asks `network.poll(now, screen == Settings && settings_tab == Network)`; `finished()` hands the `Net*` results to the tab and, on `true`, returns `save_after_change()`. *Back* with `network.drop_draft()` logs `Network changes that were not applied were dropped.` `root_jobs()` adds `Applying network settings` while `applying()`; `root_idle()` and `root_pgids()` include it, so Restart, Power off and Log out wait for it (Settled 10).
  - `fn save_after_change(&mut self) -> Option<Effect>`: the save-or-warn half of today's `screen_written`, moved out unchanged; `screen_written` calls it.

- [ ] **Step 1: Write the failing tests** in `network.rs`: `first_poll_loads_then_status_every_two_seconds`, `no_status_poll_while_closed`, `apply_sends_the_change`, `apply_with_remote_asks_first` (`event(Apply, true)` → `None`, confirm open; `ConfirmApply` → `Some(Apply(..))`; `CancelApply` → nothing, draft kept), `invalid_wired_text_blocks_apply`, `short_password_blocks_add`, `add_replaces_same_name`, `hand_file_ignores_edits`, `netboot_refuses_static`, `edits_ignored_while_applying`, `apply_failure_keeps_the_draft`, `adapter_changed_marks_the_draft_dirty` (`dirty()` true with no edit; `Apply` sends the unchanged list), `scan_failure_shows_in_dialog` (`scan_done(Err("No WiFi adapter found"))` → the dialog view carries it, `CloseAdd` closes it). In `menu.rs`: `settings_has_three_tabs`, `back_drops_the_draft_with_a_line`, `restart_waits_for_network_apply` (`root_jobs() == ["Applying network settings"]`), `network_apply_queues_one_save`, `network_apply_after_failed_restore_does_not_save`, `screen_written_still_saves`.
- [ ] **Step 2: Run, expect FAIL:** `… cargo test --locked -j 4 --lib network:: menu::`
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS**; whole crate green.
- [ ] **Step 5: Commit** `menu: the Network tab's state and its jobs`.

### Task 8: Wiring in `app.rs` and `main.rs`

**Files:**
- Modify: `src/thinfloe-menu/src/app.rs` (job dispatch next to `Job::Drives`, lines ~286 and ~332), `src/thinfloe-menu/src/main.rs` (`Paths`)

**Interfaces:**
- Consumes: Task 6's `jobs::network_*`, `netfiles::{load, take_over, wifi_devices}`, Task 7's `Job::Net*`.
- Each `Job::Net*` runs on a worker thread like `Job::Drives` and sends its `JobResult`; `NetApply` sends `NetApplyStarted { pgid }` from `on_spawn`. `main.rs` fills `network_cmd` from the default, overridable by `TF_NETWORK_CMD` (space-separated, for tests), and `homes` from `$HOME` and the `TF_*` overrides of Task 2.

- [ ] **Step 1: Write the failing test** next to the existing `Job::Drives` dispatch test in `app.rs` (line ~1040): `net_jobs_report_their_results`, with `network_cmd` pointing at a test script that prints the contract JSON for `status` and `scan` and `{"result":"ok","reason":""}` for `apply`, a test home and a fake netplan; expect `NetLoaded(..)`, `NetStatus(Ok(..))`, `NetScan(Ok(..))`, `NetApplyStarted`, then `NetApplied(Ok(()))` and the home file written.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the whole crate, expect PASS.**
- [ ] **Step 5: Commit** `menu: run the network jobs`.

### Task 9: The Network tab on screen

**Files:**
- Create: `src/thinfloe-menu/src/network_ui.rs`
- Modify: `src/thinfloe-menu/src/ui.rs` (`show_settings` dispatches the third tab; shot test), `src/thinfloe-menu/src/lib.rs`, `src/thinfloe-menu/src/brand.rs` (only for new width constants)
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `NetTab` accessors and `NetEvent` (Task 7), `nettab::{iface_line, signal_word}`.
- Produces: `pub fn network_tab(ui: &mut egui::Ui, net: &NetTab, icons: &Icons, width: f32) -> Option<NetEvent>`.
- Content, top to bottom, rows as in the Packages tab:
  1. **Connections now:** one muted row per `iface_line`; none → `No network adapter found`.
  2. **Wired:** a form row `Address` with a select `Automatic (DHCP)` / `Fixed address`; for fixed, form rows `Address`, `Gateway`, `DNS servers` (text inputs, placeholders `192.168.1.10/24`, `192.168.1.1`, `1.1.1.1, 9.9.9.9`). Netboot: no select, the line `A netbooted machine always uses DHCP.` Hand file: the values read-only, `This is set by hand in ~/ethernet.yaml`, and `Let the menu manage it` when `manageable`.
  3. **WiFi networks:** a row per saved network (lock mark when secured, `Hidden` tag) with `Forget`; `Add network` (disabled without an adapter, with `No WiFi adapter found`). `WiFi adapter changed` as a warn line when `adapter_changed`. Hand file: names only, the hand line, the take-over button only when `manageable`.
  4. **Apply** (primary button, enabled when `dirty()` and not `applying()`; `Applying…` while it runs), the error line under it.
- **Add network dialog** (the modal pattern of the end-of-session dialog): the networks in range, strongest first (name, lock, `signal_word`; unsupported ones disabled with `WEP is not supported`), `Scan again`, a `Hidden network` checkbox that shows a `Network name` input, a masked `Password` input (hidden for open networks), the password note (Global Constraints), `Add` and `Cancel`. While the scan runs: `Looking for networks…`.
- **Apply confirmation:** `The remote session may disconnect for a few seconds.` with `Apply` and `Cancel`.

- [ ] **Step 1: Write the failing tests** next to the Settings tests in `ui.rs`: `the_network_tab_lists_connections`, `the_network_tab_shows_fixed_address_fields`, `the_network_tab_hides_fixed_on_netboot`, `the_network_tab_shows_hand_wifi_read_only`, `the_add_dialog_shows_the_password_note`, `the_apply_confirmation_names_the_remote_session`; `settings_draws_nothing_rounded` already loops over `SettingsTab::ALL` and now covers Network, with the add dialog open too. Extend `shot_desktop_and_settings_at_densities` with the Network tab (menu-owned files, two networks, a fixed address) and the open add dialog.
- [ ] **Step 2: Run, expect FAIL:** `… cargo test --locked -j 4 settings` (the target where the `ui.rs` tests live).
- [ ] **Step 3: Implement.** A component shadcn-egui 0.8 lacks (for example a masked input or a checkbox): stop and ask the owner; do not paint one in the menu.
- [ ] **Step 4: Run, expect PASS**; the literal grep guard with its positive control stays clean.
- [ ] **Step 5: Add to `CHANGES.md` under `### New`:** `The Settings screen has a Network tab. It shows each connection with its address (for WiFi the network name and signal, for WireGuard the last handshake), sets the wired address to automatic (DHCP) or fixed, and saves or forgets WiFi networks found by a scan or typed by name; Apply puts the change in force at once and at every boot, and asks first while ThinLinc or OxuLnk runs.` and `A fixed wired address is kept in ~/ethernet.yaml, which the machine installs at boot like ~/wifi.yaml; a netbooted machine ignores it and always uses DHCP. A ~/wifi.yaml or ~/ethernet.yaml written by hand is shown but not changed until you choose "Let the menu manage it".`
- [ ] **Step 6: Commit** `menu: the Network tab`.

### Task 10: Acceptance

- [ ] **Step 1:** Render the shots (`TF_SHOT_DIR=/scratch/oetiker/claude-tmp/thinfloe-tests/c-shots/`), convert to PNG, compare mtimes, look at every picture: all three tabs, the Network tab in its states (menu files, hand files, netboot, no adapter), the add dialog, the confirmation, at 1080p, 4K and 21-inch 4K.
- [ ] **Step 2:** `make VERSION=<tag> test-unit`, `make VERSION=<tag> test`, `make VERSION=<tag> test-runtime`, through the build lock, never while a subagent edits the tree. Expected: all green; the count rises by the new tests only (state the delta).
- [ ] **Step 3:** Add to the handoff's "Not verified on hardware": WiFi association from a menu-written `~/wifi.yaml` at once and after a restart on the Intel 8265; `netplan apply` stops the `netplan-wpa-*` unit of a forgotten last network; wg0 survives an Apply; a fixed address after a restart.
- [ ] **Step 4:** Show the owner the pictures; ask before the fast-forward merge to `main` and the push.
