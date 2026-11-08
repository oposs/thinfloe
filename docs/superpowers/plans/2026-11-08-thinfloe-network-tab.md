# Network tab Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Settings screen gets a Network tab: it shows the connections, sets the wired address (DHCP or fixed), saves and forgets WiFi networks, and applies the change at once and at the next boot.

**Architecture:** A new Python tool `thinfloe-network` owns everything netplan: it reads and writes `~/wifi.yaml` and `~/ethernet.yaml` (with PyYAML, checked by `netplan generate --root-dir`), lists the interfaces, scans for WiFi, and, as root, installs the home files into `/etc/netplan` and runs `netplan apply`. The menu talks JSON with it and never parses or writes YAML. In the menu, `nettab.rs` holds the data and the checks, `network.rs` the tab's state machine (the pattern of `upgrade.rs`: events in, requests out), `network_ui.rs` draws it. The initramfs installs `~/ethernet.yaml` next to `~/wifi.yaml`.

**Tech Stack:** Python 3 (stdlib + PyYAML 6, both in the image), netplan.io 1.2, `iw`, `ip -j`; Rust (egui 0.37, shadcn-egui 0.8); POSIX sh (initramfs); bash test scripts.

**Spec:** `docs/superpowers/specs/2026-11-02-thinfloe-settings-and-writer-design.md`, §"Part C: Network", §"Who owns which file", "Settled after the builder check" 2, 3, 9, 10, "Settled after the pushback review" 8 (netboot), 9, 11.

## Global Constraints

- Owner header, exact: `# Written by the ThinFloe menu. Edits here are replaced.` (same as `owned::HEADER`).
- Hand-file text: `This is set by hand in ~/<file>`; button `Let the menu manage it`; rename to `<file>.by-hand`; an existing `.by-hand` refuses with `~/<file>.by-hand exists already`.
- The shipped template counts as no file: a `~/wifi.yaml` that contains `device name of your wifi card`, is empty, or holds only comments and blank lines. The menu's own file never contains that string.
- `~/ethernet.yaml` is the shipped `/etc/netplan/ethernet.yaml` with only the addressing changed: id `eth`, `match: name: 'en*'`, `optional: true` stay (the `10-netplan-eth.network.d/50-timezone.conf` drop-in depends on id `eth`).
- WiFi: open, WPA2-PSK and WPA3-SAE. A plain `password` covers WPA2 and WPA3 (netplan 1.2 renders `key_mgmt=WPA-PSK WPA-PSK-SHA256 SAE`), so the menu writes only `password` and `hidden`, never `auth`. Password: 8 to 63 characters, or 64 hex digits.
- Netboot: *Fixed address* is not offered; the tool refuses it; the initramfs does not install `~/ethernet.yaml` on netboot.
- Apply while ThinLinc (`10-thinlinc`) or OxuLnk (`20-oxulnk`) runs asks first: `The remote session may disconnect for a few seconds.`
- Password note under the field, exact: `The password is saved with the home directory. Anyone who has the stick can read it.`
- After an Apply or a take-over the menu queues a home save (the Save-On-Exit queue); none after a failed restore.
- The menu runs as tfuser; root work only through `sudo -n /usr/bin/thinfloe-network`. Passwords travel on stdin, never in argv.
- Style: lyra, nothing rounded, no colour or size literal in UI code (grep guard with its positive control stays clean), body font Ubuntu Light. Never edit the shadcn-egui repository: a missing component means stop and ask the owner.
- Machine: at most 4 cores; menu tests under `systemd-run --user --scope -q -p MemoryMax=4G --`; builds `flock /scratch/oetiker/claude-tmp/dbrrg-build.lock make VERSION=<tag> …` with `TMPDIR=/scratch/oetiker/claude-tmp/thinfloe-tests`; one `VERSION` per checkout; worktree builds use the `artifacts.link` trick from the handoff.

## Checked before writing this plan (2026-11-08, image `thinfloe-ubuntu:3.0.0`)

1. **netplan refuses `match` for WiFi:** `wifi: networkd backend does not support wifi with match:, only by interface name`. So the tool writes the device name (Settled 3, its fallback). With a name, `netplan generate` makes `netplan-wpa-<dev>.service`.
2. `network: {version: 2, renderer: networkd}` with no `wifis` passes; a `wifis` entry with no access points does not. A password of 5 characters fails (`ASCII passphrase must be between 8 and 63 characters`).
3. `netplan generate --root-dir <dir>` works as tfuser and exits 1 on a bad file.

## Calls made in this plan (not in the spec; the owner may overrule)

1. **All YAML lives in `thinfloe-network` (Python + PyYAML).** The menu has no YAML library; reading a hand-written file needs a full parser. The ownership rule (header, `.by-hand`, template) therefore exists twice, in `owned.rs` and in the tool, like `idle_timeout` exists in Rust and shell; tests on both sides pin it.
2. **Who runs as root:** `read`, `save`, `take-over` and `status` run as tfuser (no `sudo`, so the 2 s status poll writes nothing to the auth log, and no root code follows a link in the home). `scan` and `apply` run with `sudo -n`. WireGuard state therefore comes from `ip`, not `wg show` (no handshake age).
3. **Apply is two calls in one job:** `thinfloe-network save` (tfuser: checks with `netplan generate`, then writes the home files) and `sudo -n thinfloe-network apply` (root: checks the home files again, installs, `netplan apply`). A file that fails the check is never written to the home.
4. **The default for *Automatic*** is a copy of the shipped file at `/usr/share/thinfloe/netplan/ethernet.yaml`, made by the Dockerfile from `/etc/netplan/ethernet.yaml` (one source). *Automatic* removes a menu-owned `~/ethernet.yaml` and installs that copy.
5. **The menu WiFi file sets `optional: true`** on the WiFi definition: without it netplan adds the WiFi device to `systemd-networkd-wait-online` (the comment in the shipped `ethernet.yaml` shows the 120 s stall), and a home moved to a machine without that device would wait.
6. **Device name:** the first entry, sorted, of `/sys/class/net/*/wireless`. When the file names a device the machine does not have, the tab says `WiFi adapter changed` and Apply rewrites the file with the current one. Networks but no adapter: Apply fails with `No WiFi adapter found` and changes nothing.
7. **The draft:** changes collect until Apply. Switching tabs keeps them; *Back* drops them with the log line `Network changes that were not applied were dropped.`
8. **Saving a network with a name already saved** replaces its password and hidden flag; the list keeps one row per name.
9. **No status file for the network tool:** the menu waits for each call; the result is one JSON line on stdout and the exit code. Log lines go to stderr (the menu log) and to the journal (`syslog`, ident `thinfloe-network`).
10. **Signal words** for WiFi: `strong` (≥ −60 dBm), `good` (≥ −70 dBm), `weak` (below).

## JSON contract (Tasks 2–4 produce it, Task 6 parses it)

- `read` → `{"netboot": bool, "wifi": {"file": "missing|template|menu|hand", "device": str|null, "networks": [{"ssid": str, "hidden": bool, "secured": bool}], "manageable": bool, "error": str|null}, "wired": {"file": "missing|menu|hand", "mode": "dhcp|static", "address": "a.b.c.d/p"|null, "gateway": str|null, "dns": [str], "manageable": bool, "error": str|null}, "wifi_devices": [str]}`. Never a password.
- `status` → `{"interfaces": [{"name": str, "kind": "wired|wifi|wireguard", "state": "connected|no-cable|not-connected|off", "addresses": ["a.b.c.d/p"], "ssid": str|null, "signal_dbm": int|null}]}`.
- `scan` → `{"device": str, "networks": [{"ssid": str, "signal_dbm": int, "secured": bool, "supported": bool}]}`, strongest first, one entry per name, at most 64.
- `save` and `apply` read on stdin `{"wifi": null | {"networks": [{"ssid": str, "hidden": bool, "password": str|null}]}, "wired": null | {"mode": "dhcp"} | {"mode": "static", "address": "a.b.c.d/p", "gateway": str|null, "dns": [str]}}`. `null` leaves that file alone; `"password": null` keeps the password of that name from the current menu file. `apply` reads no stdin: it installs what is in the home.
- `save`, `apply`, `take-over wifi|wired` print `{"result": "ok"|"failed", "reason": str}`. Exit codes: 0 ok, 1 failed, 2 refused (hand-owned file, static on netboot, bad input, the template marker in an SSID).

## Review Focus

1. **Odd characters.** An SSID or password with `"`, `\`, `:`, `#`, a leading space, or non-ASCII letters must reach `wpa-<dev>.conf` exactly as typed. → Task 2 `test_render_round_trips_odd_text`, Task 5 `odd text reaches wpa conf`.
2. **A file that fails `netplan generate`** must change nothing: not the home, not `/etc/netplan`, and the tab must show the reason. → Task 2 `test_save_refuses_what_netplan_refuses`, Task 4 `test_apply_rejects_bad_home_file`, Task 7 `apply_failure_keeps_the_draft`.
3. **A home moved to another machine** (other WiFi device name, or none): boot does not wait, the tab says `WiFi adapter changed`, Apply fixes it. → Task 2 `test_render_wifi_is_optional`, Task 4 `test_apply_rewrites_changed_adapter`, Task 7 `adapter_changed_marks_the_draft_dirty`.
4. **A hand-written or symlinked `~/wifi.yaml` / `~/ethernet.yaml`** is never written, renamed or removed except by *Let the menu manage it*. → Task 2 `test_save_never_touches_hand_or_link`, Task 4 `test_apply_refuses_linked_home_file`.
5. **A scan that cannot run** (no adapter, adapter down, `Device or resource busy`, a hanging `iw`): the dialog shows a reason and can be closed; nothing hangs. → Task 3 `test_scan_brings_link_up_and_retries_busy`, `test_scan_times_out`; Task 7 `scan_failure_shows_in_dialog`.

---

### Task 1: The initramfs installs `~/ethernet.yaml`; the image keeps the DHCP default

**Files:**
- Modify: `overlay/usr/lib/dracut/modules.d/90thinfloe/thinfloe-lib.sh` (`install_local_network`, lines 774–837)
- Modify: `overlay/usr/lib/dracut/modules.d/90thinfloe/setup-overlay.sh:77-79`
- Modify: `containers/ubuntu/Dockerfile` (the overlay `RUN` near line 281)
- Test: `test/integration/test-initramfs-home.sh` (the `install_local_network` block, line 410 on)

**Interfaces:**
- Produces: `install_local_network <newroot> [<ramroot-url>]`. A remote URL (`is_remote_url`) means netboot. `/usr/share/thinfloe/netplan/ethernet.yaml` (mode 644, byte-equal to the shipped `/etc/netplan/ethernet.yaml`).
- Behaviour: when `~/ethernet.yaml` is a regular file (not a link) and the boot is not netboot, copy it over `$root/etc/netplan/ethernet.yaml`, mode 600, log `thinfloe: installing ethernet.yaml into /etc/netplan`. Netboot: log `thinfloe: netboot, ~/ethernet.yaml not installed` and leave the shipped file. Link: `thinfloe: no usable ~/ethernet.yaml, skipping`. Still always returns 0; wifi, ethernet and wg0 halves stay independent. `setup-overlay.sh` passes `"$(cat /tmp/thinfloe-ramroot 2>/dev/null)"`. The comment block names the new file and why netboot skips it.

- [ ] **Step 1: Write the failing cases** in `test-initramfs-home.sh`: `ethernet.yaml installed with mode 600`, `ethernet.yaml symlink refused (shipped file unchanged)`, `no ethernet.yaml leaves the shipped file`, `netboot skips ethernet.yaml but installs wifi.yaml`, `a failing ethernet copy still installs wg0`. (The Dockerfile copy is checked in the built image by Task 5, `dhcp default is the shipped file`.)
- [ ] **Step 2: Run, expect the new cases to FAIL:** `test/integration/test-initramfs-home.sh`
- [ ] **Step 3: Implement**; Dockerfile: `&& install -D -m 644 /etc/netplan/ethernet.yaml /usr/share/thinfloe/netplan/ethernet.yaml` after the `chmod 600`.
- [ ] **Step 4: Run, expect all `ok`.** `sh -n` on both shell files.
- [ ] **Step 5: Commit** `initramfs: install ~/ethernet.yaml, except on netboot`.

### Task 2: `thinfloe-network` files: render, read, save, take-over

**Files:**
- Create: `overlay/usr/bin/thinfloe-network` (mode 755, `#!/usr/bin/python3 -I`, importable like `thinfloe-addon`: nothing runs on import)
- Create: `test/unit/test_network.py` (loads the tool through `importlib`, as `test_addon.py` does)

**Interfaces:**
- Produces (module level, Tasks 3–4 use them):
  - `HEADER`, `TEMPLATE_MARKER = "device name of your wifi card"`, `WIFI_FILE = "wifi.yaml"`, `WIRED_FILE = "ethernet.yaml"`, `DEFAULT_WIRED = os.environ.get("TF_NETWORK_DEFAULT_WIRED", "/usr/share/thinfloe/netplan/ethernet.yaml")`, `HOME_DIR = os.environ.get("TF_NETWORK_HOME")` (unset: `getpwnam("tfuser").pw_dir`), `SYS_NET = os.environ.get("TF_NETWORK_SYS", "/sys/class/net")`, `ESP_DIR = os.environ.get("TF_NETWORK_ESP", "/run/thinfloe/storage/efi")`, `NETPLAN = os.environ.get("TF_NETWORK_NETPLAN", "netplan")`.
  - `owner(text: Optional[str], template_rule: bool) -> str`: `"missing"`, `"template"` (only with `template_rule`), `"menu"`, `"hand"`.
  - `read_home(name: str) -> Optional[str]`: `O_NOFOLLOW`, 64 KiB cap; a link raises `NetError("~/<name> is a link")`.
  - `wifi_devices() -> List[str]`; `netboot() -> bool` (`not os.path.ismount(ESP_DIR)`).
  - `render_wifi(device: Optional[str], networks: List[dict]) -> str` (header + YAML; no networks or no device: `network: {version: 2, renderer: networkd}` only; else one definition `<device>` with `dhcp4: true`, `optional: true`, `access-points`; an AP is `{}`, or holds `password` and/or `hidden: true`).
  - `render_wired(wired: dict) -> Optional[str]`: `None` for `dhcp`; for `static`, `DEFAULT_WIRED` loaded with `yaml.safe_load`, `dhcp4: false`, `addresses: [cidr]`, `routes: [{to: default, via: gw}]` when a gateway is given, `nameservers: {addresses: [...]}` when DNS is given.
  - `describe() -> dict`: the `read` JSON. `manageable` for WiFi (Settled after pushback 9): top keys ⊆ `{network}`, network ⊆ `{version, renderer, wifis}`, exactly one WiFi definition with keys ⊆ `{dhcp4, optional, access-points}`, each AP `None`, `{}` or keys ⊆ `{password, hidden}`. For wired: one `ethernets` entry `eth` with keys ⊆ `{match, dhcp4, addresses, routes, nameservers, optional}`.
  - `check_netplan(files: Dict[str, str]) -> Optional[str]`: writes them (mode 600) into a fresh `tempfile.mkdtemp()` root under `etc/netplan/`, always including the wired file in force (the menu's, else `DEFAULT_WIRED`), runs `NETPLAN generate --root-dir <root>`, returns `None` or the first `ERROR:` line (else the last stderr line); removes the root.
  - `save(request: dict) -> Tuple[int, str]`; `take_over(what: str) -> Tuple[int, str]`; `main(argv) -> int` with subcommands `read`, `save`, `take-over wifi|wired` (Task 3 adds `status`, `scan`; Task 4 `apply`).
  - `class NetError(Exception)`.
- Save rules: validate input (SSID 1–32 bytes, no `TEMPLATE_MARKER`; password `None`, 8–63 chars or 64 hex; IPv4 CIDR prefix 1–32; gateway and DNS IPv4; at most 3 DNS) else exit 2. A hand-owned target exits 2 with `~/<file> is set by hand`. Missing passwords come from the current menu file. Static on netboot exits 2. Render, `check_netplan`, then write each file as `<file>.tmp` (`O_CREAT|O_EXCL|O_NOFOLLOW`, mode 600, fsync) and rename; `dhcp` removes a menu-owned `~/ethernet.yaml`.
- `take_over`: only when `manageable`; renames to `<file>.by-hand` (refuse if it exists), writes the menu file with the same content (networks, passwords, device), checked like `save`.

- [ ] **Step 1: Write the failing tests** (a temp home via `TF_NETWORK_HOME`, a fake `/sys/class/net` via `TF_NETWORK_SYS`, a fake `netplan` script on `TF_NETWORK_NETPLAN` that exits by a flag file):
  - `test_owner_rules`: missing; template by marker, empty, comments-only; menu by header in line 1; hand otherwise; the template rule off for wired.
  - `test_render_wifi_is_optional`: one device, two networks → `optional: true`, `dhcp4: true`, no `auth`, header first, no marker anywhere.
  - `test_render_wifi_without_networks_is_bare`.
  - `test_render_round_trips_odd_text`: SSIDs `Café "Nord"`, ` lead`, `a:b #c`, `back\slash`; password `p@ss: "x" #1\\`; `yaml.safe_load(render_wifi(...))` gives them back exactly.
  - `test_render_wired_keeps_eth_match_optional`: static → id `eth`, `match.name == "en*"`, `optional is True`, `dhcp4 is False`, `routes == [{"to": "default", "via": "192.168.1.1"}]`.
  - `test_describe_hides_passwords` and `test_describe_manageable_rules` (an extra AP key `auth` → not manageable; two WiFi definitions → not manageable; names still listed).
  - `test_save_keeps_password_by_name`, `test_save_replaces_same_name`.
  - `test_save_refuses_what_netplan_refuses`: fake netplan fails → exit 1, reason from its `ERROR:` line, home unchanged.
  - `test_save_never_touches_hand_or_link`: hand file and symlinked file both byte-equal after `save`, exit 2.
  - `test_save_static_refused_on_netboot`, `test_save_dhcp_removes_menu_wired_only`.
  - `test_take_over_keeps_networks_and_passwords`, `test_take_over_refuses_unmanageable`, `test_take_over_refuses_existing_by_hand`.
  - `test_input_checks`: password of 7 and of 64 non-hex characters, SSID of 33 bytes, SSID with the marker, prefix 33, 4 DNS servers → exit 2 each.
- [ ] **Step 2: Run, expect FAIL:** `python3 -B -m unittest test.unit.test_network -v` (from the repo root; the suite is also found by `make test-unit`).
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS.**
- [ ] **Step 5: Commit** `thinfloe-network: read and write ~/wifi.yaml and ~/ethernet.yaml`.

### Task 3: `thinfloe-network status` and `scan`

**Files:**
- Modify: `overlay/usr/bin/thinfloe-network`
- Modify: `test/unit/test_network.py`
- Create: `test/unit/network-fixtures/` (`ip-link.json`, `ip-addr.json`, `iw-link-connected.txt`, `iw-link-not.txt`, `iw-scan.txt`), captured from a real machine or written to match `iw` 6.x output.

**Interfaces:**
- Produces: `parse_ip(link_json: str, addr_json: str, sys_net: str) -> List[dict]`, `parse_iw_link(text: str) -> Tuple[Optional[str], Optional[int]]`, `parse_scan(text: str) -> List[dict]`, `status() -> dict`, `scan() -> dict`; commands from `IP = os.environ.get("TF_NETWORK_IP", "ip")`, `IW = os.environ.get("TF_NETWORK_IW", "iw")`.
- Kind: `wifi` when `<sys>/<if>/wireless` exists; `wired` when `<sys>/<if>/device` exists and `type` is 1; `wireguard` when `ip -j -d link` reports `linkinfo.info_kind == "wireguard"`; everything else is left out. State: `off` without `UP` in flags; wired `connected` with `LOWER_UP`, else `no-cable`; WiFi `connected` when `iw dev <if> link` names an SSID, else `not-connected`; WireGuard `connected` with `UP`. Addresses: IPv4 `local/prefixlen`.
- `scan` (root): no WiFi device → exit 1 `No WiFi adapter found`. Link down → `ip link set <if> up` first. `iw dev <if> scan`, 20 s timeout; `Device or resource busy` → wait 2 s, one retry. Parse BSS blocks: `SSID:` (decode `\xNN`; empty or all-NUL names dropped), `signal: -NN.NN dBm`, `RSN:` or `WPA:` → secured; `capability: … Privacy` without RSN/WPA → `supported: false` (WEP). Keep the strongest per name, sort strongest first, cut at 64.

- [ ] **Step 1: Write the failing tests:** `test_parse_ip_kinds_and_states` (wired up, wired no cable, WiFi, wg0, `lo` and a bridge left out), `test_parse_iw_link`, `test_parse_scan_dedupes_and_sorts`, `test_parse_scan_marks_wep_and_open`, `test_parse_scan_decodes_escapes`, `test_scan_brings_link_up_and_retries_busy` (fake `ip`/`iw` scripts that record calls; the first scan prints `command failed: Device or resource busy (-16)`), `test_scan_times_out` (fake `iw` sleeps 60 s; `TF_NETWORK_SCAN_TIMEOUT=1` → exit 1 within 3 s), `test_scan_without_adapter`.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS.**
- [ ] **Step 5: Commit** `thinfloe-network: list interfaces and scan for WiFi`.

### Task 4: `thinfloe-network apply`

**Files:**
- Modify: `overlay/usr/bin/thinfloe-network`
- Modify: `test/unit/test_network.py`

**Interfaces:**
- Produces: `apply() -> Tuple[int, str]`; `ETC_NETPLAN = os.environ.get("TF_NETWORK_ETC", "/etc/netplan")`; `LOCK = os.environ.get("TF_NETWORK_LOCK", "/run/thinfloe/network.lock")`.
- Behaviour: must run as root (else exit 2 `apply needs root`). Holds `LOCK` (`flock`, exclusive). Reads `~/wifi.yaml` and `~/ethernet.yaml` with `read_home` (a link fails the whole apply, nothing changed). WiFi file: `template` or `missing` → leave `/etc/netplan/wifi.yaml` as it is; otherwise it is installed (a hand file too: it is what the next boot installs). A menu WiFi file whose device is not in `wifi_devices()` is rewritten in the home with the current device first (exit 1 `No WiFi adapter found` when it has networks and there is none). Wired file: present → it; absent → `DEFAULT_WIRED`; on netboot always `DEFAULT_WIRED`. `check_netplan` on the set; a failure → exit 1, reason, nothing installed. Then write each into `ETC_NETPLAN` (tmp + rename, mode 600) and run `NETPLAN apply` (120 s timeout). Every step logs one line (stderr + journal).

- [ ] **Step 1: Write the failing tests** (root is faked: `TF_NETWORK_ASSUME_ROOT=1`, read only by the tests' environment and documented as such): `test_apply_installs_home_files_mode_600`, `test_apply_template_leaves_etc_wifi`, `test_apply_dhcp_installs_default_copy`, `test_apply_rejects_bad_home_file` (fake netplan generate fails → `/etc` byte-equal, `apply` never called), `test_apply_refuses_linked_home_file`, `test_apply_rewrites_changed_adapter`, `test_apply_netboot_ignores_home_wired`, `test_apply_reports_netplan_apply_failure` (exit 1, reason), `test_apply_needs_root` (without the test flag and not uid 0 → exit 2).
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS.**
- [ ] **Step 5: Commit** `thinfloe-network: apply the home's network files`.

### Task 5: The real netplan, in the image

**Files:**
- Create: `test/integration/test-network-netplan.sh`
- Modify: `Makefile` (`test:` gets `@TF_UBUNTU_IMAGE=$(UBUNTU_IMAGE) test/integration/test-network-netplan.sh`, after `test-addon-deps.sh`)

**Interfaces:**
- Consumes: `render_wifi`, `render_wired`, `save`, `apply` from Tasks 2–4, run inside a throw-away container of the built image (`podman run --rm`, the tool from the image, not from the checkout, plus `--user tfuser` for `save`).

- [ ] **Step 1: Write the cases** (each prints `ok   - …` / `FAIL - …`, as the other scripts do):
  - `menu wifi gives netplan-wpa-<dev>`: a fake `TF_NETWORK_SYS` with `wlp58s0/wireless`; `save` two networks (one hidden, one open) as tfuser; `netplan generate --root-dir` on the result → `netplan-wpa-wlp58s0.service` exists, `wpa-wlp58s0.conf` has `scan_ssid=1` for the hidden one and `key_mgmt=NONE` for the open one.
  - `odd text reaches wpa conf`: the SSID and password of Task 2's odd-text test appear in `wpa-wlp58s0.conf` as netplan writes them (compare with the conf of a hand-written equivalent file).
  - `wifi is not waited for`: `systemd-networkd-wait-online.service.d/10-netplan.conf` does not list `wlp58s0`.
  - `static wired keeps id eth`: `10-netplan-eth.network` has `Address=192.168.1.10/24`, `Gateway=192.168.1.1`, `DNS=1.1.1.1`, `[Match] Name=en*`.
  - `dhcp default is the shipped file`: `/usr/share/thinfloe/netplan/ethernet.yaml` equals `/etc/netplan/ethernet.yaml`.
  - `comments-only wifi.yaml passes as it is` (Settled after pushback 11).
  - `apply as root installs`: in the container as root with `netplan apply` replaced by a stub (`TF_NETWORK_NETPLAN` wrapper that passes `generate` through and records `apply`) → `/etc/netplan/wifi.yaml` mode 600 and equal to the home file.
- [ ] **Step 2: Run** `TF_UBUNTU_IMAGE=localhost/thinfloe-ubuntu:<tag> test/integration/test-network-netplan.sh` against an image built from this branch; expect all `ok`. (It needs the image of Task 1, so it is built here: `make VERSION=<tag> rootfs` through the build lock.)
- [ ] **Step 3: Commit** `test: the network tool's files pass the image's netplan`.

### Task 6: Menu data and checks (`nettab.rs`) and the tool calls (`jobs.rs`)

**Files:**
- Create: `src/thinfloe-menu/src/nettab.rs`
- Modify: `src/thinfloe-menu/src/lib.rs`, `src/thinfloe-menu/src/jobs.rs`

**Interfaces:**
- Produces in `nettab.rs`:
  - `pub enum FileState { Missing, Template, Menu, Hand }`; `pub struct WifiFile { pub file: FileState, pub device: Option<String>, pub networks: Vec<SavedNet>, pub manageable: bool, pub error: Option<String> }`; `pub struct SavedNet { pub ssid: String, pub hidden: bool, pub secured: bool }`; `pub enum Wired { Dhcp, Static { address: Ipv4Addr, prefix: u8, gateway: Option<Ipv4Addr>, dns: Vec<Ipv4Addr> } }`; `pub struct WiredFile { pub file: FileState, pub wired: Wired, pub manageable: bool, pub error: Option<String> }`; `pub struct Files { pub netboot: bool, pub wifi: WifiFile, pub wired: WiredFile, pub wifi_devices: Vec<String> }`.
  - `pub enum IfKind { Wired, Wifi, Wireguard }`, `pub enum IfState { Connected, NoCable, NotConnected, Off }`, `pub struct Iface { pub name, pub kind, pub state, pub addresses: Vec<String>, pub ssid: Option<String>, pub signal_dbm: Option<i32> }`; `pub struct InRange { pub ssid: String, pub signal_dbm: i32, pub secured: bool, pub supported: bool }`.
  - `pub fn parse_files(json: &str) -> Result<Files, String>`, `pub fn parse_status(json: &str) -> Result<Vec<Iface>, String>`, `pub fn parse_scan(json: &str) -> Result<Vec<InRange>, String>`, `pub fn parse_result(stdout: &str) -> Result<(), String>` (last JSON line; `failed` → `Err(reason)`). Untrusted input as in `drives.rs`: at most 64 rows, text cut to 64 chars (SSIDs to 32 bytes on a char boundary).
  - `pub struct NewNet { pub ssid: String, pub hidden: bool, pub password: Option<String> }`; `pub struct Draft { pub wifi: Option<Vec<NewNet>>, pub wired: Option<Wired> }` (`None` = unchanged); `pub fn request_json(d: &Draft) -> String` (the stdin of `save`; kept networks carry `"password": null`).
  - `pub fn check_password(p: &str) -> Result<(), String>`: `The password needs 8 to 63 characters.` (64 hex digits pass).
  - `pub fn parse_wired(address: &str, gateway: &str, dns: &str) -> Result<Wired, String>`: address `a.b.c.d/p`, prefix 1–32, else `Address: use the form 192.168.1.10/24`; gateway empty or IPv4 inside the subnet, else `Gateway: not in the network of the address`; DNS split on commas and spaces, 0–3 IPv4, else `DNS servers: up to three IPv4 addresses`.
  - `pub fn signal_word(dbm: i32) -> &'static str` (`strong`, `good`, `weak`, call 10); `pub fn iface_line(i: &Iface) -> String`, for example `WiFi wlp58s0 · Home Net · strong · 192.168.1.23/24`, `Wired enp0s31f6 · no cable`, `WireGuard wg0 · connected · 10.0.0.2/32`.
  - `pub fn adapter_changed(f: &Files) -> bool`: menu file with a device not in `wifi_devices`, and `wifi_devices` not empty.
- Produces in `jobs.rs`:
  - `fn capture(cmd: &[String], args: &[String], stdin: Option<&[u8]>, cap: usize, timeout: Duration, feed: &Arc<Feed>) -> Result<(String, Option<i32>), String>`: today's `upgrade_capture` with optional stdin; `upgrade_capture` calls it.
  - `Paths` gains `network_cmd: Vec<String>` (`sudo -n /usr/bin/thinfloe-network`) and `network_user_cmd: Vec<String>` (`/usr/bin/thinfloe-network`); `pub fn default_network_cmd()`, `pub fn default_network_user_cmd()`.
  - `pub fn network_read(paths, feed) -> Result<Files, String>`, `network_status(paths, feed) -> Result<Vec<Iface>, String>` (5 s timeout), `network_scan(paths, feed) -> Result<Vec<InRange>, String>` (40 s), `network_take_over(paths, what: &str, feed) -> Result<(), String>`, `network_apply(paths, request: &str, feed, on_spawn: impl FnOnce(i32)) -> Result<(), String>`: runs `save` with `request` on stdin, then, only on success, `apply` in a process group of its own (`on_spawn` gets it; 150 s timeout), each with `parse_result`.
  - `JobResult` gains `NetRead(Result<Files, String>)`, `NetStatus(Result<Vec<Iface>, String>)`, `NetScan(Result<Vec<InRange>, String>)`, `NetApplyStarted { pgid: i32 }`, `NetApplied(Result<(), String>)`, `NetTakenOver(Result<(), String>)`.

- [ ] **Step 1: Write the failing tests:** in `nettab.rs`: `parse_files_reads_the_contract` (a full JSON from the contract above), `parse_files_refuses_junk` (not JSON, 65 networks, wrong types), `parse_status_kinds`, `parse_scan_cuts_long_ssid`, `request_json_keeps_passwords_out_of_kept_networks`, `check_password_limits` (7 → Err, 8 → Ok, 63 → Ok, 64 hex → Ok, 64 other → Err), `parse_wired_rules` (each message above), `signal_words`, `iface_lines`, `adapter_changed_rule` (menu file naming `wlp2s0` on a machine with `wlp58s0` → true; same device, hand file, or no device at all → false). In `jobs.rs`: `capture_feeds_stdin` (a `sh -c 'cat'` command returns the stdin), `network_apply_skips_apply_after_failed_save` (fake tool scripts in a test dir that record their calls).
- [ ] **Step 2: Run, expect FAIL:** `cd src/thinfloe-menu && systemd-run --user --scope -q -p MemoryMax=4G -- cargo test --locked -j 4 nettab:: jobs::`
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS**; the whole crate stays green.
- [ ] **Step 5: Commit** `menu: the network tool's JSON and the address checks`.

### Task 7: The Network tab's state machine (`network.rs`) and its place in `menu.rs`

**Files:**
- Create: `src/thinfloe-menu/src/network.rs`
- Modify: `src/thinfloe-menu/src/settings.rs` (`SettingsTab`, `SettingsEvent`), `src/thinfloe-menu/src/menu.rs`, `src/thinfloe-menu/src/lib.rs`

**Interfaces:**
- Consumes: Task 6's types; `Menu::request_save`; `addon_id`.
- Produces in `network.rs`:
  - `pub const POLL: Duration = Duration::from_secs(2);`
  - `pub enum NetEvent { WiredMode(bool /* static */), WiredText { address: String, gateway: String, dns: String }, Forget(String), OpenAdd, CloseAdd, Rescan, PickNetwork(String), AddText { ssid: String, password: String, hidden: bool }, Add, Apply, ConfirmApply, CancelApply, TakeOver(&'static str /* "wifi" | "wired" */) }`.
  - `pub enum NetRequest { Read, Status, Scan, Apply(String /* request_json */), TakeOver(&'static str) }`.
  - `pub struct NetTab` with `pub fn new() -> NetTab`, `pub fn event(&mut self, e: NetEvent, remote: bool) -> Option<NetRequest>`, `pub fn poll(&mut self, now: Instant, open: bool) -> Option<NetRequest>` (first `Read` when the tab first opens, then `Status` every `POLL` while open and no status call is in flight), `pub fn read_done(&mut self, r: Result<Files, String>)`, `status_done`, `scan_done`, `pub fn apply_started(&mut self, pgid: i32)`, `pub fn apply_done(&mut self, r: Result<(), String>) -> bool /* true: ok, save the home */`, `pub fn take_over_done(&mut self, r: Result<(), String>) -> bool`, `pub fn dirty(&self) -> bool`, `pub fn applying(&self) -> bool`, `pub fn pgid(&self) -> Option<i32>`, `pub fn drop_draft(&mut self) -> bool /* true: something was dropped */`, plus read accessors the UI needs (`files()`, `ifaces()`, `draft view`, `add dialog view`, `confirm_open()`, `error()`).
  - Rules: `Apply` with a draft that fails `parse_wired` or a password check sets the error and sends nothing. `Apply` with `remote` opens the confirmation; `ConfirmApply` sends. A hand-owned file has no draft part (its events are ignored). `WiredMode(true)` is ignored on netboot. `adapter_changed` makes `dirty()` true with an unchanged WiFi list. While `applying()` every edit event is ignored. `Add` replaces a saved network of the same name (call 8). After `apply_done(Ok)` the draft is cleared and a `Read` is due; after `Err` the draft stays and `error()` holds the reason.
- In `settings.rs`: `SettingsTab::Network` between Packages and Screen (`ALL` has 3 entries, label `Network`); `SettingsEvent::Network(NetEvent)`.
- In `menu.rs`: field `pub network: NetTab`; `Job::{NetRead, NetStatus, NetScan, NetApply { request: String }, NetTakeOver { what: &'static str }}`; `settings()` passes `Network(e)` on with `remote = self.remote_running()`, where `fn remote_running(&self) -> bool` is true when a `Running` job's tile has `addon_id` `10-thinlinc` or `20-oxulnk`; `tick()` asks `network.poll(now, screen == Settings && settings_tab == Network)`; `finished()` hands the `Net*` results to the tab and, on `true`, returns `save_after_change()`. `Back` with `network.drop_draft()` logs `Network changes that were not applied were dropped.` `root_jobs()` adds `Applying network settings` while `applying()`; `root_idle()` and `root_pgids()` include it, so Restart, Power off and Log out wait for it (Settled 10).
  - `fn save_after_change(&mut self) -> Option<Effect>`: the save-or-warn half of today's `screen_written`, moved out unchanged; `screen_written` calls it.

- [ ] **Step 1: Write the failing tests** in `network.rs`: `first_poll_reads_then_status_every_two_seconds`, `no_status_poll_while_closed`, `apply_sends_the_draft_json`, `apply_with_remote_asks_first` (`event(Apply, true)` → `None`, confirm open; `ConfirmApply` → `Some(Apply(..))`; `CancelApply` → nothing, draft kept), `invalid_wired_text_blocks_apply`, `short_password_blocks_add`, `add_replaces_same_name`, `hand_file_ignores_edits`, `netboot_refuses_static`, `edits_ignored_while_applying`, `apply_failure_keeps_the_draft`, `adapter_changed_marks_the_draft_dirty` (`dirty()` true with no edit; `Apply` sends the unchanged list), `scan_failure_shows_in_dialog` (`scan_done(Err("No WiFi adapter found"))` → the dialog view carries the text and `CloseAdd` closes it). In `menu.rs`: `settings_has_three_tabs`, `back_drops_the_draft_with_a_line`, `restart_waits_for_network_apply` (`root_jobs() == ["Applying network settings"]`), `network_apply_queues_one_save`, `network_apply_after_failed_restore_does_not_save`, `screen_written_still_saves` (the move of `save_after_change` changed nothing).
- [ ] **Step 2: Run, expect FAIL:** `… cargo test --locked -j 4 --lib network:: menu::`
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS**; whole crate green.
- [ ] **Step 5: Commit** `menu: the Network tab's state and its jobs`.

### Task 8: Wiring in `app.rs` and `main.rs`

**Files:**
- Modify: `src/thinfloe-menu/src/app.rs` (job dispatch next to `Job::Drives`, lines ~286 and ~332), `src/thinfloe-menu/src/main.rs` (`Paths`)

**Interfaces:**
- Consumes: Task 6's `jobs::network_*`, Task 7's `Job::Net*`.
- Each `Job::Net*` runs on a worker thread like `Job::Drives` and sends its `JobResult`; `NetApply` sends `NetApplyStarted { pgid }` from `on_spawn`. `main.rs` fills `network_cmd`/`network_user_cmd` from the defaults, overridable by `TF_NETWORK_CMD` / `TF_NETWORK_USER_CMD` (space-separated, for tests), the way the add-on command is set.

- [ ] **Step 1: Write the failing test** next to the existing `Job::Drives` dispatch test in `app.rs` (line ~1040): `net_jobs_report_their_results`, with `network_user_cmd`/`network_cmd` pointing at a test script that prints the contract JSON for `read`, `status`, `scan`, and `{"result":"ok","reason":""}` for `save`/`apply`; expect `NetRead(Ok(..))`, `NetStatus(Ok(..))`, `NetScan(Ok(..))`, `NetApplyStarted`, then `NetApplied(Ok(()))`.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the whole crate, expect PASS.**
- [ ] **Step 5: Commit** `menu: run the network tool's jobs`.

### Task 9: The Network tab on screen

**Files:**
- Create: `src/thinfloe-menu/src/network_ui.rs`
- Modify: `src/thinfloe-menu/src/ui.rs` (`show_settings` dispatches the third tab; shot test), `src/thinfloe-menu/src/lib.rs`, `src/thinfloe-menu/src/brand.rs` (only for new width constants)
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `NetTab` accessors and `NetEvent` (Task 7), `nettab::{iface_line, signal_word, check_password}`.
- Produces: `pub fn network_tab(ui: &mut egui::Ui, net: &NetTab, icons: &Icons, width: f32) -> Option<NetEvent>`.
- Content, top to bottom, rows as in the Packages tab:
  1. **Connections now:** one muted row per `iface_line`; none → `No network adapter found`.
  2. **Wired:** a form row `Address` with a select `Automatic (DHCP)` / `Fixed address`; for fixed, form rows `Address`, `Gateway`, `DNS servers` (text inputs, placeholders `192.168.1.10/24`, `192.168.1.1`, `1.1.1.1, 9.9.9.9`). Netboot: no select, the line `A netbooted machine always uses DHCP.` Hand file: the values read-only, `This is set by hand in ~/ethernet.yaml`, and `Let the menu manage it` when `manageable`.
  3. **WiFi networks:** a row per saved network (lock mark when secured, `Hidden` tag) with `Forget`; `Add network` (disabled without an adapter, with `No WiFi adapter found`). `WiFi adapter changed` as a warn line when `adapter_changed`. Hand file: names only, the hand line, the take-over button only when `manageable`.
  4. **Apply** (primary button, enabled when `dirty()` and not `applying()`; `Applying…` while it runs), the error line under it.
- **Add network dialog** (the modal pattern of the end-of-session dialog): the networks in range, strongest first (name, lock, `signal_word`; unsupported ones disabled with `WEP is not supported`), `Scan again`, a `Hidden network` checkbox that shows a `Network name` input, a masked `Password` input (hidden for open networks), the password note (Global Constraints), `Add` and `Cancel`. While the scan runs: `Looking for networks…`.
- **Apply confirmation:** the text `The remote session may disconnect for a few seconds.` with `Apply` and `Cancel`.

- [ ] **Step 1: Write the failing tests** next to the Settings tests in `ui.rs`: `the_network_tab_lists_connections`, `the_network_tab_shows_fixed_address_fields`, `the_network_tab_hides_fixed_on_netboot`, `the_network_tab_shows_hand_wifi_read_only`, `the_add_dialog_shows_the_password_note`, `the_apply_confirmation_names_the_remote_session`; `settings_draws_nothing_rounded` already loops over `SettingsTab::ALL` and now covers Network, with the add dialog open too. Extend `shot_desktop_and_settings_at_densities` with the Network tab (menu-owned file with two networks and a fixed address) and the open add dialog.
- [ ] **Step 2: Run, expect FAIL:** `… cargo test --locked -j 4 settings` (the target where the `ui.rs` tests live).
- [ ] **Step 3: Implement.** Missing component in shadcn-egui 0.8 (for example a masked input or a checkbox): stop and ask the owner; do not paint one in the menu.
- [ ] **Step 4: Run, expect PASS**; the literal grep guard with its positive control stays clean.
- [ ] **Step 5: Add to `CHANGES.md` under `### New`:** `The Settings screen has a Network tab. It shows each connection with its address (and for WiFi the network name and signal), sets the wired address to automatic (DHCP) or fixed, and saves or forgets WiFi networks found by a scan or typed by name; Apply puts the change in force at once and at every boot, and asks first while ThinLinc or OxuLnk runs.` and `A fixed wired address is kept in ~/ethernet.yaml, which the machine installs at boot like ~/wifi.yaml; a netbooted machine ignores it and always uses DHCP. A ~/wifi.yaml or ~/ethernet.yaml written by hand is shown but not changed until you choose "Let the menu manage it".`
- [ ] **Step 6: Commit** `menu: the Network tab`.

### Task 10: Acceptance

- [ ] **Step 1:** Render the shots (`TF_SHOT_DIR=/scratch/oetiker/claude-tmp/thinfloe-tests/c-shots/`), convert to PNG, compare mtimes, look at every picture: all three tabs, the Network tab in its states (menu files, hand files, netboot, no adapter), the add dialog, the confirmation, at 1080p, 4K and 21-inch 4K.
- [ ] **Step 2:** `make VERSION=<tag> test-unit`, `make VERSION=<tag> test`, `make VERSION=<tag> test-runtime`, through the build lock, never while a subagent edits the tree. Expected: all green; the count rises by the new tests only (state the delta).
- [ ] **Step 3:** Add to the handoff's "Not verified on hardware": WiFi association from a menu-written `~/wifi.yaml` at once and after a restart on the Intel 8265; `netplan apply` stops the `netplan-wpa-*` unit of a forgotten last network; wg0 survives an Apply; a fixed address after a restart.
- [ ] **Step 4:** Show the owner the pictures; ask before the fast-forward merge to `main` and the push.
