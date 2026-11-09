//! The Network tab as a state machine: the files as read, the interfaces
//! and the networks in range as the tool reports them, and the draft the
//! person builds until Apply. No drawing here; the caller runs the
//! `NetRequest`s through `jobs` and feeds the results back, as for the
//! Upgrade screen.
//!
//! Changes collect until Apply (plan call 5). A file written by hand has no
//! draft part: it is shown, and can only be taken over. While an Apply
//! runs, every edit is ignored.

use std::time::{Duration, Instant};

use crate::jobs::ApplyFailed;
use crate::netfiles::{self, Change, FileState, Net, WifiFile, Wired, WiredFile};
use crate::nettab::{self, Iface, InRange};

/// The status poll while the tab is open.
pub const POLL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetEvent {
    /// The wired select: `true` is a fixed address.
    WiredMode(bool),
    WiredText {
        address: String,
        gateway: String,
        dns: String,
    },
    /// Forget the saved network of this name.
    Forget(String),
    OpenAdd,
    CloseAdd,
    Rescan,
    /// A network in range was clicked in the add dialog.
    PickNetwork(String),
    AddText {
        ssid: String,
        password: String,
        hidden: bool,
    },
    Add,
    Apply,
    ConfirmApply,
    CancelApply,
    /// *Let the menu manage it*: `"wifi"` or `"wired"`.
    TakeOver(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetRequest {
    Load,
    Status,
    Scan,
    Apply(Change),
    TakeOver(&'static str),
}

/// The wired part as the form shows it: the select and the three texts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WiredForm {
    pub fixed: bool,
    pub address: String,
    pub gateway: String,
    pub dns: String,
}

impl WiredForm {
    fn of(w: &Wired) -> WiredForm {
        match w {
            Wired::Dhcp => WiredForm::default(),
            Wired::Static {
                address,
                prefix,
                gateway,
                dns,
            } => WiredForm {
                fixed: true,
                address: format!("{address}/{prefix}"),
                gateway: gateway.map(|g| g.to_string()).unwrap_or_default(),
                dns: dns.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(", "),
            },
        }
    }

    /// Two forms ask for the same: DHCP ignores the texts.
    fn same(&self, other: &WiredForm) -> bool {
        self.fixed == other.fixed && (!self.fixed || self == other)
    }

    fn wired(&self) -> Result<Wired, String> {
        if self.fixed {
            nettab::parse_wired_form(&self.address, &self.gateway, &self.dns)
        } else {
            Ok(Wired::Dhcp)
        }
    }
}

/// The add dialog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddView {
    /// The networks in range, strongest first.
    pub in_range: Vec<InRange>,
    pub scanning: bool,
    pub scan_error: Option<String>,
    pub ssid: String,
    pub password: String,
    pub hidden: bool,
    /// The name was picked from the scan: `Some(secured)`.
    pub picked: Option<bool>,
    /// The name field was typed in; it stays on screen, even emptied,
    /// until a row is picked or Hidden is unticked.
    pub typing: bool,
    pub error: Option<String>,
}

impl AddView {
    /// A picked open network: no password is asked for.
    pub fn open_network(&self) -> bool {
        self.picked == Some(false)
    }

    /// The picked network stands on screen as a chosen row: the list is
    /// drawn (no scan runs) and still holds the name.
    pub fn picked_row_shown(&self) -> bool {
        !self.scanning && self.picked.is_some() && self.in_range.iter().any(|n| n.ssid == self.ssid)
    }

    /// The name field is on screen: for a hidden network, and for any set
    /// name no chosen row shows. Add acts on `ssid`, so with this the
    /// dialog always shows the name Add saves.
    pub fn name_field_shown(&self) -> bool {
        self.hidden || self.typing || (!self.ssid.is_empty() && !self.picked_row_shown())
    }
}

#[derive(Debug)]
pub struct NetTab {
    netboot: bool,
    wifi: Option<WifiFile>,
    wired: Option<WiredFile>,
    devices: Vec<String>,
    ifaces: Vec<Iface>,
    /// A status call has answered once, with a list or an error.
    status_known: bool,
    status_error: Option<String>,
    /// `None`: unchanged.
    draft_wifi: Option<Vec<Net>>,
    draft_wired: Option<WiredForm>,
    add: Option<AddView>,
    confirm: bool,
    error: Option<String>,
    load_due: bool,
    loading: bool,
    status_busy: bool,
    status_at: Option<Instant>,
    scan_busy: bool,
    applying: bool,
    pgid: Option<i32>,
    /// The file a take-over runs for: `"wifi"` or `"wired"`.
    taking_over: Option<&'static str>,
}

impl NetTab {
    pub fn new(netboot: bool) -> NetTab {
        NetTab {
            netboot,
            wifi: None,
            wired: None,
            devices: Vec::new(),
            ifaces: Vec::new(),
            status_known: false,
            status_error: None,
            draft_wifi: None,
            draft_wired: None,
            add: None,
            confirm: false,
            error: None,
            load_due: true,
            loading: false,
            status_busy: false,
            status_at: None,
            scan_busy: false,
            applying: false,
            pgid: None,
            taking_over: None,
        }
    }

    /// The menu may change this WiFi file (read, and not by hand).
    fn wifi_editable(&self) -> bool {
        self.wifi.as_ref().is_some_and(|w| w.state != FileState::Hand)
    }

    fn wired_editable(&self) -> bool {
        self.wired.as_ref().is_some_and(|w| w.state != FileState::Hand)
    }

    fn saved_networks(&self) -> &[Net] {
        self.wifi.as_ref().map_or(&[], |w| w.networks.as_slice())
    }

    fn saved_form(&self) -> WiredForm {
        self.wired.as_ref().map(|w| WiredForm::of(&w.wired)).unwrap_or_default()
    }

    /// The WiFi list the person changed, if it differs from the file.
    fn wifi_changed(&self) -> bool {
        self.draft_wifi.as_deref().is_some_and(|d| d != self.saved_networks())
    }

    fn wired_changed(&self) -> bool {
        self.draft_wired.as_ref().is_some_and(|d| !d.same(&self.saved_form()))
    }

    /// One click or text change. While an Apply or a take-over runs, and
    /// before the files are read, nothing is changed.
    pub fn event(&mut self, e: NetEvent, remote: bool) -> Option<NetRequest> {
        if self.applying || self.taking_over.is_some() || self.wifi.is_none() || self.wired.is_none() {
            return None;
        }
        match e {
            NetEvent::WiredMode(fixed) => {
                if !self.wired_editable() || (fixed && self.netboot) {
                    return None;
                }
                let mut form = self.wired_form();
                form.fixed = fixed;
                self.draft_wired = Some(form);
                None
            }
            NetEvent::WiredText { address, gateway, dns } => {
                // Netboot has no address fields to type in.
                if !self.wired_editable() || self.netboot {
                    return None;
                }
                let fixed = self.wired_form().fixed;
                self.draft_wired = Some(WiredForm {
                    fixed,
                    address,
                    gateway,
                    dns,
                });
                None
            }
            NetEvent::Forget(ssid) => {
                if self.wifi_editable() {
                    let mut list = self.networks().to_vec();
                    list.retain(|n| n.ssid != ssid);
                    self.draft_wifi = Some(list);
                }
                None
            }
            NetEvent::OpenAdd => {
                if !self.wifi_editable() || self.add.is_some() {
                    return None;
                }
                self.add = Some(AddView {
                    scanning: true,
                    ..AddView::default()
                });
                self.scan()
            }
            NetEvent::CloseAdd => {
                self.add = None;
                None
            }
            NetEvent::Rescan => {
                let view = self.add.as_mut()?;
                if view.scanning {
                    return None;
                }
                view.scanning = true;
                view.scan_error = None;
                self.scan()
            }
            NetEvent::PickNetwork(ssid) => {
                // Ruling 14: a network the menu cannot join is not picked;
                // the form keeps what it had.
                let view = self.add.as_mut()?;
                let found = view.in_range.iter().find(|n| n.ssid == ssid && n.supported)?;
                view.picked = Some(found.secured);
                view.typing = false;
                view.ssid = ssid;
                view.password.clear();
                view.hidden = false;
                view.error = None;
                None
            }
            NetEvent::AddText { ssid, password, hidden } => {
                let view = self.add.as_mut()?;
                // A changed name is no longer the network picked.
                if ssid != view.ssid || hidden {
                    view.picked = None;
                }
                // Unticking Hidden takes the name field off the dialog;
                // the name typed in it goes with it, so Add never acts on
                // a name the dialog does not show.
                let unticked = view.hidden && !hidden;
                // Only the name field changes the name.
                view.typing = !unticked && (view.typing || ssid != view.ssid);
                view.ssid = if unticked { String::new() } else { ssid };
                view.password = password;
                view.hidden = hidden;
                None
            }
            NetEvent::Add => {
                let view = self.add.as_mut()?;
                let net = match add_net(view) {
                    Ok(n) => n,
                    Err(e) => {
                        view.error = Some(e);
                        return None;
                    }
                };
                self.add = None;
                // Plan call 6: one row per name.
                let mut list = self.networks().to_vec();
                match list.iter_mut().find(|n| n.ssid == net.ssid) {
                    Some(old) => *old = net,
                    None => list.push(net),
                }
                self.draft_wifi = Some(list);
                None
            }
            NetEvent::Apply => {
                let change = self.checked_change()?;
                if remote {
                    self.confirm = true;
                    return None;
                }
                Some(self.send(change))
            }
            NetEvent::ConfirmApply => {
                if !self.confirm {
                    return None;
                }
                self.confirm = false;
                let change = self.checked_change()?;
                Some(self.send(change))
            }
            NetEvent::CancelApply => {
                self.confirm = false;
                None
            }
            NetEvent::TakeOver(which) => {
                let (state, manageable) = match which {
                    "wifi" => self.wifi.as_ref().map(|w| (w.state, w.manageable))?,
                    "wired" => self.wired.as_ref().map(|w| (w.state, w.manageable))?,
                    _ => return None,
                };
                if state != FileState::Hand || !manageable {
                    return None;
                }
                self.taking_over = Some(which);
                self.error = None;
                Some(NetRequest::TakeOver(which))
            }
        }
    }

    fn scan(&mut self) -> Option<NetRequest> {
        if self.scan_busy {
            return None;
        }
        self.scan_busy = true;
        Some(NetRequest::Scan)
    }

    /// The change of the draft, checked; `None` with the reason in
    /// `error()` when a part fails, or when nothing changed.
    fn checked_change(&mut self) -> Option<Change> {
        if !self.dirty() {
            return None;
        }
        match self.change() {
            Ok(c) => Some(c),
            Err(e) => {
                self.error = Some(e);
                None
            }
        }
    }

    fn change(&self) -> Result<Change, String> {
        let wired = match &self.draft_wired {
            Some(form) if self.wired_changed() => Some(form.wired()?),
            _ => None,
        };
        // A swapped adapter: the same list goes out, under the new name.
        let adapter = self.adapter_changed();
        let wifi = (self.wifi_changed() || adapter).then(|| self.networks().to_vec());
        for n in wifi.iter().flatten() {
            // Named, as take-over does, so the person knows which to forget.
            let named = |e: String| format!("{}: {e}", n.ssid);
            netfiles::check_ssid(&n.ssid).map_err(named)?;
            if let Some(p) = &n.password {
                netfiles::check_password(p).map_err(named)?;
            }
        }
        Ok(Change { wifi, wired })
    }

    fn send(&mut self, change: Change) -> NetRequest {
        self.error = None;
        self.applying = true;
        self.add = None;
        NetRequest::Apply(change)
    }

    /// Called on every frame. `open`: the tab is on screen. The files are
    /// read when it first opens and after an Apply or a take-over; the
    /// status is asked for every `POLL` while it is open.
    pub fn poll(&mut self, now: Instant, open: bool) -> Option<NetRequest> {
        if !open {
            return None;
        }
        if self.load_due && !self.loading {
            self.load_due = false;
            self.loading = true;
            return Some(NetRequest::Load);
        }
        if !self.status_busy && self.status_at.is_none_or(|at| now >= at) {
            self.status_busy = true;
            self.status_at = Some(now + POLL);
            return Some(NetRequest::Status);
        }
        None
    }

    /// The files as read. A part of the draft whose file is now written
    /// by hand is dropped.
    pub fn loaded(&mut self, wifi: WifiFile, wired: WiredFile, devices: Vec<String>) {
        self.loading = false;
        if wifi.state == FileState::Hand {
            self.draft_wifi = None;
            self.add = None;
        }
        if wired.state == FileState::Hand {
            self.draft_wired = None;
        }
        self.wifi = Some(wifi);
        self.wired = Some(wired);
        self.devices = devices;
    }

    pub fn status_done(&mut self, r: Result<Vec<Iface>, String>) {
        self.status_busy = false;
        self.status_known = true;
        match r {
            Ok(list) => {
                self.ifaces = list;
                self.status_error = None;
            }
            Err(e) => self.status_error = Some(e),
        }
    }

    /// The scan ended; shown only while the dialog is open.
    pub fn scan_done(&mut self, r: Result<Vec<InRange>, String>) {
        self.scan_busy = false;
        let Some(view) = self.add.as_mut() else {
            return;
        };
        view.scanning = false;
        match r {
            Ok(mut list) => {
                list.sort_by_key(|n| std::cmp::Reverse(n.signal_dbm));
                // The picked network as the new list has it: its security
                // may have changed, or the menu can no longer join it. A
                // name the list lacks keeps what was known.
                if view.picked.is_some()
                    && let Some(n) = list.iter().find(|n| n.ssid == view.ssid)
                {
                    view.picked = n.supported.then_some(n.secured);
                }
                view.in_range = list;
                view.scan_error = None;
            }
            Err(e) => view.scan_error = Some(e),
        }
    }

    pub fn apply_started(&mut self, pgid: i32) {
        if self.applying {
            self.pgid = Some(pgid);
        }
    }

    /// The Apply ended. `true`: the home changed and is to be saved, after
    /// a success or when a failed Apply could not put the home back.
    pub fn apply_done(&mut self, r: Result<(), ApplyFailed>) -> bool {
        if !self.applying {
            return false;
        }
        self.applying = false;
        self.pgid = None;
        match r {
            Ok(()) => {
                self.draft_wifi = None;
                self.draft_wired = None;
                self.load_due = true;
                true
            }
            // The home was put back, or names what could not be: read
            // it again; the draft stays for another try.
            Err(e) => {
                self.error = Some(e.reason);
                self.load_due = true;
                e.home_changed
            }
        }
    }

    /// The take-over ended. `true`: the home changed and is to be saved.
    pub fn take_over_done(&mut self, r: Result<(), String>) -> bool {
        if self.taking_over.take().is_none() {
            return false;
        }
        match r {
            Ok(()) => {
                self.load_due = true;
                true
            }
            Err(e) => {
                // It may have failed after the rename: read the files again.
                self.load_due = true;
                self.error = Some(e);
                false
            }
        }
    }

    /// Apply has something to do.
    pub fn dirty(&self) -> bool {
        self.wifi_changed() || self.wired_changed() || self.adapter_changed()
    }

    pub fn applying(&self) -> bool {
        self.applying
    }

    pub fn taking_over(&self) -> bool {
        self.taking_over.is_some()
    }

    /// The home file the running take-over is for.
    pub fn taking_over_file(&self) -> &'static str {
        match self.taking_over {
            Some("wired") => netfiles::WIRED_FILE,
            _ => netfiles::WIFI_FILE,
        }
    }

    /// The process group of the running Apply, once known.
    pub fn pgid(&self) -> Option<i32> {
        self.pgid
    }

    /// *Back*: forget what was not applied. `true`: something typed was
    /// dropped. A draft that is being applied is kept.
    pub fn drop_draft(&mut self) -> bool {
        if self.applying {
            return false;
        }
        let dropped = self.wifi_changed() || self.wired_changed();
        self.draft_wifi = None;
        self.draft_wired = None;
        self.add = None;
        self.confirm = false;
        self.error = None;
        dropped
    }

    pub fn netboot(&self) -> bool {
        self.netboot
    }

    /// The files as read; `None` until the first load.
    pub fn wifi_file(&self) -> Option<&WifiFile> {
        self.wifi.as_ref()
    }

    pub fn wired_file(&self) -> Option<&WiredFile> {
        self.wired.as_ref()
    }

    /// The machine's WiFi adapters.
    pub fn devices(&self) -> &[String] {
        &self.devices
    }

    pub fn ifaces(&self) -> &[Iface] {
        &self.ifaces
    }

    /// The interfaces are known: an empty list means there are none.
    pub fn status_known(&self) -> bool {
        self.status_known
    }

    /// Why the last status call failed.
    pub fn status_error(&self) -> Option<&str> {
        self.status_error.as_deref()
    }

    pub fn confirm_open(&self) -> bool {
        self.confirm
    }

    /// Why the last Apply or take-over did not happen.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn add_view(&self) -> Option<&AddView> {
        self.add.as_ref()
    }

    /// The saved WiFi networks with the draft's changes.
    pub fn networks(&self) -> &[Net] {
        self.draft_wifi.as_deref().unwrap_or_else(|| self.saved_networks())
    }

    /// The wired form with the draft's changes.
    pub fn wired_form(&self) -> WiredForm {
        self.draft_wired.clone().unwrap_or_else(|| self.saved_form())
    }

    /// The menu's WiFi file names an adapter this machine does not have
    /// (plan call 4): Apply writes the current one.
    pub fn adapter_changed(&self) -> bool {
        self.wifi
            .as_ref()
            .is_some_and(|w| nettab::adapter_changed(w, &self.devices))
    }
}

/// The network the add dialog describes, checked. A picked open network
/// has no password; a typed name without one is an open network too.
fn add_net(view: &AddView) -> Result<Net, String> {
    netfiles::check_ssid(&view.ssid)?;
    let password = match view.picked {
        Some(false) => None,
        None if view.password.is_empty() => None,
        _ => {
            netfiles::check_password(&view.password)?;
            Some(view.password.clone())
        }
    };
    Ok(Net {
        ssid: view.ssid.clone(),
        hidden: view.hidden,
        password,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(ssid: &str, password: Option<&str>) -> Net {
        Net {
            ssid: ssid.to_string(),
            hidden: false,
            password: password.map(String::from),
        }
    }

    fn wifi(state: FileState, device: Option<&str>, networks: Vec<Net>) -> WifiFile {
        WifiFile {
            state,
            device: device.map(String::from),
            networks,
            manageable: true,
            error: None,
        }
    }

    fn wired(state: FileState, w: Wired) -> WiredFile {
        WiredFile {
            state,
            wired: w,
            manageable: true,
            error: None,
        }
    }

    /// A tab on a stick-booted machine with menu-owned files: one saved
    /// network on adapter `wlp1`, wired by DHCP.
    fn tab() -> NetTab {
        let mut t = NetTab::new(false);
        t.loaded(
            wifi(FileState::Menu, Some("wlp1"), vec![net("Home", Some("secret123"))]),
            wired(FileState::Menu, Wired::Dhcp),
            vec!["wlp1".to_string()],
        );
        t
    }

    fn fixed(address: &str, gateway: &str, dns: &str) -> NetEvent {
        NetEvent::WiredText {
            address: address.into(),
            gateway: gateway.into(),
            dns: dns.into(),
        }
    }

    fn add_text(ssid: &str, password: &str, hidden: bool) -> NetEvent {
        NetEvent::AddText {
            ssid: ssid.into(),
            password: password.into(),
            hidden,
        }
    }

    fn in_range(ssid: &str, dbm: i32, secured: bool, supported: bool) -> InRange {
        InRange {
            ssid: ssid.into(),
            signal_dbm: dbm,
            secured,
            supported,
            name_ok: true,
        }
    }

    /// The tab with a fixed address typed in: a draft that applies.
    fn with_fixed_draft() -> NetTab {
        let mut t = tab();
        assert_eq!(t.event(NetEvent::WiredMode(true), false), None);
        assert_eq!(t.event(fixed("192.168.1.10/24", "192.168.1.1", "1.1.1.1"), false), None);
        t
    }

    fn fixed_change() -> Change {
        Change {
            wifi: None,
            wired: Some(nettab::parse_wired_form("192.168.1.10/24", "192.168.1.1", "1.1.1.1").unwrap()),
        }
    }

    #[test]
    fn first_poll_loads_then_status_every_two_seconds() {
        let mut t = NetTab::new(false);
        let t0 = Instant::now();
        assert_eq!(t.poll(t0, true), Some(NetRequest::Load));
        assert_eq!(t.poll(t0, true), Some(NetRequest::Status));
        assert_eq!(t.poll(t0, true), None, "a status call is in flight");
        t.status_done(Ok(Vec::new()));
        assert_eq!(t.poll(t0 + Duration::from_secs(1), true), None);
        assert_eq!(t.poll(t0 + POLL, true), Some(NetRequest::Status));
        // The answer is late: no second call while one runs.
        assert_eq!(t.poll(t0 + POLL * 3, true), None);
        t.status_done(Err("timeout".into()));
        assert_eq!(t.poll(t0 + POLL * 3, true), Some(NetRequest::Status));
        // Loaded once; not again on its own.
        t.loaded(
            wifi(FileState::Missing, None, Vec::new()),
            wired(FileState::Missing, Wired::Dhcp),
            Vec::new(),
        );
        t.status_done(Ok(Vec::new()));
        assert_eq!(t.poll(t0 + POLL * 4, true), Some(NetRequest::Status));
    }

    #[test]
    fn no_status_poll_while_closed() {
        let mut t = NetTab::new(false);
        let t0 = Instant::now();
        assert_eq!(t.poll(t0, false), None, "nothing before the tab opens");
        assert_eq!(t.poll(t0 + POLL * 10, false), None);
        assert_eq!(t.poll(t0 + POLL * 10, true), Some(NetRequest::Load));
        assert_eq!(t.poll(t0 + POLL * 10, true), Some(NetRequest::Status));
        t.status_done(Ok(Vec::new()));
        assert_eq!(t.poll(t0 + POLL * 20, false), None);
    }

    #[test]
    fn apply_sends_the_change() {
        let mut t = with_fixed_draft();
        assert!(t.dirty());
        assert_eq!(t.event(NetEvent::Apply, false), Some(NetRequest::Apply(fixed_change())));
        assert!(t.applying());
        // A clean draft sends nothing.
        let mut t = tab();
        assert!(!t.dirty());
        assert_eq!(t.event(NetEvent::Apply, false), None);
        // Back to what is saved is not a change: DHCP ignores the texts.
        t.event(NetEvent::WiredMode(true), false);
        t.event(fixed("10.0.0.2/8", "", ""), false);
        t.event(NetEvent::WiredMode(false), false);
        assert!(!t.dirty());
        // Forgetting sends the WiFi list only.
        t.event(NetEvent::Forget("Home".into()), false);
        assert_eq!(
            t.event(NetEvent::Apply, false),
            Some(NetRequest::Apply(Change {
                wifi: Some(Vec::new()),
                wired: None
            }))
        );
    }

    #[test]
    fn apply_with_remote_asks_first() {
        let mut t = with_fixed_draft();
        assert_eq!(t.event(NetEvent::Apply, true), None);
        assert!(t.confirm_open());
        assert!(!t.applying());
        assert_eq!(t.event(NetEvent::CancelApply, true), None);
        assert!(!t.confirm_open());
        assert!(t.dirty(), "the draft is kept");
        assert_eq!(t.event(NetEvent::ConfirmApply, true), None, "nothing to confirm");
        assert_eq!(t.event(NetEvent::Apply, true), None);
        assert_eq!(
            t.event(NetEvent::ConfirmApply, true),
            Some(NetRequest::Apply(fixed_change()))
        );
        assert!(!t.confirm_open());
        assert!(t.applying());
    }

    #[test]
    fn invalid_wired_text_blocks_apply() {
        let mut t = tab();
        t.event(NetEvent::WiredMode(true), false);
        t.event(fixed("192.168.1.10", "", ""), false);
        assert!(t.dirty());
        assert_eq!(t.event(NetEvent::Apply, false), None);
        assert_eq!(t.error(), Some("Address: use the form 192.168.1.10/24"));
        assert!(!t.applying());
        // Also with a remote session: no question for a draft that fails.
        assert_eq!(t.event(NetEvent::Apply, true), None);
        assert!(!t.confirm_open());
        t.event(fixed("192.168.1.10/24", "10.0.0.1", ""), false);
        assert_eq!(t.event(NetEvent::Apply, false), None);
        assert_eq!(t.error(), Some("Gateway: not in the network of the address"));
        t.event(fixed("192.168.1.10/24", "", ""), false);
        assert!(t.event(NetEvent::Apply, false).is_some());
        assert_eq!(t.error(), None);
    }

    #[test]
    fn invalid_saved_network_blocks_apply() {
        // A menu file read with a password the checks refuse: the list is
        // checked before it is sent.
        let mut t = NetTab::new(false);
        t.loaded(
            wifi(
                FileState::Menu,
                Some("wlp1"),
                vec![net("Old", Some("short")), net("B", None)],
            ),
            wired(FileState::Menu, Wired::Dhcp),
            vec!["wlp1".to_string()],
        );
        t.event(NetEvent::Forget("B".into()), false);
        assert_eq!(t.event(NetEvent::Apply, false), None);
        // The refusal names the network, so the person knows what to forget.
        assert_eq!(t.error(), Some("Old: The password needs 8 to 63 characters."));
    }

    #[test]
    fn short_password_blocks_add() {
        let mut t = tab();
        assert_eq!(t.event(NetEvent::OpenAdd, false), Some(NetRequest::Scan));
        t.event(add_text("Office", "short", false), false);
        assert_eq!(t.event(NetEvent::Add, false), None);
        let view = t.add_view().expect("the dialog stays open");
        assert_eq!(view.error.as_deref(), Some("The password needs 8 to 63 characters."));
        assert!(!t.dirty());
        t.event(add_text("", "longenough", true), false);
        t.event(NetEvent::Add, false);
        assert_eq!(
            t.add_view().unwrap().error.as_deref(),
            Some("The name needs 1 to 32 bytes.")
        );
        t.event(add_text("Office", "longenough", true), false);
        t.event(NetEvent::Add, false);
        assert!(t.add_view().is_none(), "added: the dialog closes");
        assert_eq!(
            t.networks(),
            [
                net("Home", Some("secret123")),
                Net {
                    ssid: "Office".into(),
                    hidden: true,
                    password: Some("longenough".into())
                }
            ]
        );
        assert!(t.dirty());
    }

    /// Unticking Hidden drops the name typed for the hidden network: the
    /// dialog no longer shows a field for it, so Add must not use it.
    #[test]
    fn unticking_hidden_drops_the_name() {
        let mut t = tab();
        t.event(NetEvent::OpenAdd, false);
        t.scan_done(Ok(vec![in_range("Office", -50, true, true)]));
        t.event(NetEvent::PickNetwork("Office".into()), false);
        assert_eq!(t.add_view().unwrap().picked, Some(true));
        // Tick: the picked name moves into the name field.
        t.event(add_text("Office", "", true), false);
        let v = t.add_view().unwrap();
        assert_eq!((v.ssid.as_str(), v.picked, v.hidden), ("Office", None, true));
        // Untick: no row is picked and no name field shows; no name is left.
        t.event(add_text("Office", "", false), false);
        let v = t.add_view().unwrap();
        assert_eq!((v.ssid.as_str(), v.picked, v.hidden), ("", None, false));
        // Add saves nothing, least of all Office as an open network.
        assert_eq!(t.event(NetEvent::Add, false), None);
        assert_eq!(t.networks(), [net("Home", Some("secret123"))]);
    }

    /// A rescan brings the picked name back with other security: Add acts
    /// on the new list, and a network now unsupported is no longer picked.
    #[test]
    fn a_rescan_takes_the_picked_network_from_the_new_list() {
        let mut t = tab();
        t.event(NetEvent::OpenAdd, false);
        t.scan_done(Ok(vec![in_range("Office", -50, true, true)]));
        t.event(NetEvent::PickNetwork("Office".into()), false);
        assert!(!t.add_view().unwrap().open_network());
        // Office is open now: no password is asked for, and Add saves it open.
        t.event(NetEvent::Rescan, false);
        t.scan_done(Ok(vec![in_range("Office", -50, false, true)]));
        let v = t.add_view().unwrap();
        assert_eq!(v.picked, Some(false));
        assert!(v.open_network() && v.picked_row_shown() && !v.name_field_shown());
        // And secured again: the password is asked for.
        t.event(NetEvent::Rescan, false);
        t.scan_done(Ok(vec![in_range("Office", -50, true, true)]));
        assert_eq!(t.add_view().unwrap().picked, Some(true));
        // Now unsupported: not picked; the name stays in the name field.
        t.event(NetEvent::Rescan, false);
        t.scan_done(Ok(vec![in_range("Office", -50, true, false)]));
        let v = t.add_view().unwrap();
        assert_eq!(v.picked, None);
        assert!(!v.picked_row_shown() && v.name_field_shown() && v.ssid == "Office");
        // A typed name is not a pick, and a rescan does not make it one.
        let mut t = tab();
        t.event(NetEvent::OpenAdd, false);
        t.event(add_text("Office", "", true), false);
        t.scan_done(Ok(vec![in_range("Office", -50, false, true)]));
        assert_eq!(t.add_view().unwrap().picked, None);
    }

    /// Every way to set a name leaves it on screen: a chosen row, or the
    /// name field.
    #[test]
    fn the_name_add_saves_is_always_shown() {
        let shown = |t: &NetTab| {
            let v = t.add_view().unwrap();
            v.ssid.is_empty() || v.picked_row_shown() || v.name_field_shown()
        };
        let mut t = tab();
        t.event(NetEvent::OpenAdd, false);
        assert!(shown(&t));
        t.scan_done(Ok(vec![in_range("Office", -50, true, true)]));
        t.event(NetEvent::PickNetwork("Office".into()), false);
        assert!(t.add_view().unwrap().picked_row_shown());
        assert!(!t.add_view().unwrap().name_field_shown());
        // A rescan: while it runs the list is not drawn, so the field shows.
        t.event(NetEvent::Rescan, false);
        assert!(!t.add_view().unwrap().picked_row_shown());
        assert!(t.add_view().unwrap().name_field_shown());
        // The new list has Office again: back to the chosen row.
        t.scan_done(Ok(vec![in_range("Office", -50, true, true)]));
        assert!(t.add_view().unwrap().picked_row_shown());
        // A new list without Office: the field, with the name.
        t.event(NetEvent::Rescan, false);
        t.scan_done(Ok(vec![in_range("Cafe", -60, false, true)]));
        let v = t.add_view().unwrap();
        assert!(!v.picked_row_shown() && v.name_field_shown() && v.ssid == "Office");
        assert!(shown(&t));
        // Hidden ticked and unticked.
        t.event(add_text("Office", "", true), false);
        assert!(t.add_view().unwrap().name_field_shown());
        t.event(add_text("Office", "", false), false);
        assert!(shown(&t));
        // Cancel and reopen: an empty form.
        t.event(NetEvent::CloseAdd, false);
        t.event(NetEvent::OpenAdd, false);
        assert!(t.add_view().unwrap().ssid.is_empty());
    }

    /// Clearing the name field to type another name keeps the field: it
    /// goes only with a picked row or an unticked Hidden.
    #[test]
    fn a_cleared_name_field_stays_on_screen() {
        let mut t = tab();
        t.event(NetEvent::OpenAdd, false);
        t.scan_done(Ok(vec![in_range("Office", -50, true, true)]));
        t.event(NetEvent::PickNetwork("Office".into()), false);
        t.event(NetEvent::Rescan, false);
        t.scan_done(Ok(vec![in_range("Cafe", -60, false, true)]));
        assert!(t.add_view().unwrap().name_field_shown());
        t.event(add_text("O", "", false), false);
        t.event(add_text("", "", false), false);
        let v = t.add_view().unwrap();
        assert!(v.ssid.is_empty() && v.name_field_shown());
        t.event(add_text("Lab", "", false), false);
        assert!(t.add_view().unwrap().name_field_shown());
        // Picking a row takes the field away again.
        t.event(NetEvent::PickNetwork("Cafe".into()), false);
        let v = t.add_view().unwrap();
        assert!(v.picked_row_shown() && !v.name_field_shown());
        // As does unticking Hidden, which drops the name typed there.
        t.event(add_text("Cafe", "", true), false);
        t.event(add_text("", "", true), false);
        t.event(add_text("", "", false), false);
        assert!(!t.add_view().unwrap().name_field_shown());
    }

    #[test]
    fn add_replaces_same_name() {
        let mut t = tab();
        t.event(NetEvent::OpenAdd, false);
        t.event(add_text("Home", "newsecret", true), false);
        t.event(NetEvent::Add, false);
        assert_eq!(
            t.networks(),
            [Net {
                ssid: "Home".into(),
                hidden: true,
                password: Some("newsecret".into())
            }],
            "one row per name, password and hidden flag replaced"
        );
    }

    #[test]
    fn pick_fills_the_form_for_supported_networks_only() {
        let mut t = tab();
        t.event(NetEvent::OpenAdd, false);
        t.scan_done(Ok(vec![
            in_range("Weak", -80, true, true),
            in_range("Cafe", -50, false, true),
            in_range("Old WEP", -40, true, false),
        ]));
        let names: Vec<&str> = t.add_view().unwrap().in_range.iter().map(|n| n.ssid.as_str()).collect();
        assert_eq!(names, ["Old WEP", "Cafe", "Weak"], "strongest first");
        // Ruling 14: an unsupported network is not picked; the form stays.
        t.event(add_text("typed", "", false), false);
        assert_eq!(t.event(NetEvent::PickNetwork("Old WEP".into()), false), None);
        assert_eq!(t.add_view().unwrap().ssid, "typed");
        assert_eq!(t.event(NetEvent::PickNetwork("Not in range".into()), false), None);
        assert_eq!(t.add_view().unwrap().ssid, "typed");
        // An open network needs no password.
        t.event(NetEvent::PickNetwork("Cafe".into()), false);
        let v = t.add_view().unwrap();
        assert_eq!((v.ssid.as_str(), v.open_network(), v.hidden), ("Cafe", true, false));
        t.event(NetEvent::Add, false);
        assert!(t.networks().contains(&net("Cafe", None)));
        // A secured one does.
        t.event(NetEvent::OpenAdd, false);
        t.scan_done(Ok(vec![in_range("Weak", -80, true, true)]));
        t.event(NetEvent::PickNetwork("Weak".into()), false);
        t.event(NetEvent::Add, false);
        assert_eq!(
            t.add_view().unwrap().error.as_deref(),
            Some("The password needs 8 to 63 characters.")
        );
    }

    #[test]
    fn hand_file_ignores_edits() {
        let mut t = NetTab::new(false);
        t.loaded(
            wifi(FileState::Hand, Some("wlp1"), vec![net("Home", None)]),
            wired(FileState::Hand, Wired::Dhcp),
            vec!["wlp1".to_string()],
        );
        t.event(NetEvent::WiredMode(true), false);
        t.event(fixed("192.168.1.10/24", "", ""), false);
        t.event(NetEvent::Forget("Home".into()), false);
        assert_eq!(t.event(NetEvent::OpenAdd, false), None);
        assert!(t.add_view().is_none());
        assert!(!t.dirty());
        assert_eq!(t.event(NetEvent::Apply, false), None);
        assert_eq!(t.networks(), [net("Home", None)]);
        // Only the take-over is offered.
        assert_eq!(
            t.event(NetEvent::TakeOver("wired"), false),
            Some(NetRequest::TakeOver("wired"))
        );
        assert_eq!(t.event(NetEvent::TakeOver("wifi"), false), None, "one at a time");
        assert!(t.take_over_done(Ok(())), "a take-over saves the home");
        assert_eq!(t.poll(Instant::now(), true), Some(NetRequest::Load));
        assert_eq!(
            t.event(NetEvent::TakeOver("wifi"), false),
            Some(NetRequest::TakeOver("wifi"))
        );
        assert!(!t.take_over_done(Err("~/wifi.yaml: busy".into())));
        assert_eq!(t.error(), Some("~/wifi.yaml: busy"));
        // A failed take-over may have changed the files: read them again.
        assert!(t.load_due);
        // A menu file or an unknown name is not taken over.
        let mut m = tab();
        assert_eq!(m.event(NetEvent::TakeOver("wifi"), false), None);
        assert_eq!(m.event(NetEvent::TakeOver("other"), false), None);
        // Nor a hand file the menu cannot manage.
        let mut n = NetTab::new(false);
        let mut w = wired(FileState::Hand, Wired::Dhcp);
        w.manageable = false;
        n.loaded(wifi(FileState::Missing, None, Vec::new()), w, Vec::new());
        assert_eq!(n.event(NetEvent::TakeOver("wired"), false), None);
    }

    #[test]
    fn netboot_refuses_static() {
        let mut t = NetTab::new(true);
        t.loaded(
            wifi(FileState::Missing, None, Vec::new()),
            wired(FileState::Missing, Wired::Dhcp),
            Vec::new(),
        );
        assert_eq!(t.event(NetEvent::WiredMode(true), false), None);
        assert!(!t.wired_form().fixed);
        t.event(fixed("192.168.1.10/24", "", ""), false);
        assert!(!t.dirty());
        assert_eq!(t.event(NetEvent::Apply, false), None);
        // A fixed address left in the home from a stick boot: no text
        // edit turns it into a change; going back to DHCP is one.
        let mut t = NetTab::new(true);
        let kept = nettab::parse_wired_form("192.168.1.10/24", "", "").unwrap();
        t.loaded(
            wifi(FileState::Missing, None, Vec::new()),
            wired(FileState::Menu, kept),
            Vec::new(),
        );
        t.event(fixed("10.0.0.2/8", "", ""), false);
        assert!(!t.dirty());
        t.event(NetEvent::WiredMode(false), false);
        assert_eq!(
            t.event(NetEvent::Apply, false),
            Some(NetRequest::Apply(Change {
                wifi: None,
                wired: Some(Wired::Dhcp)
            }))
        );
    }

    #[test]
    fn edits_ignored_while_applying() {
        let mut t = with_fixed_draft();
        assert!(t.event(NetEvent::Apply, false).is_some());
        assert_eq!(t.pgid(), None);
        t.apply_started(4242);
        assert_eq!(t.pgid(), Some(4242));
        let before = t.wired_form();
        for e in [
            NetEvent::WiredMode(false),
            fixed("10.0.0.2/8", "", ""),
            NetEvent::Forget("Home".into()),
            NetEvent::OpenAdd,
            NetEvent::Apply,
            NetEvent::ConfirmApply,
            NetEvent::TakeOver("wifi"),
        ] {
            assert_eq!(t.event(e.clone(), false), None, "{e:?}");
        }
        assert_eq!(t.wired_form(), before);
        assert_eq!(t.networks(), [net("Home", Some("secret123"))]);
        assert!(t.add_view().is_none());
        assert!(!t.drop_draft(), "the draft in flight is not dropped");
        // Done: the draft is cleared, a Load is due, the home is saved.
        assert!(t.apply_done(Ok(())));
        assert!(!t.applying());
        assert_eq!(t.pgid(), None);
        assert!(!t.dirty());
        assert_eq!(t.poll(Instant::now(), true), Some(NetRequest::Load));
    }

    #[test]
    fn apply_failure_keeps_the_draft() {
        let mut t = with_fixed_draft();
        t.event(NetEvent::Apply, false);
        t.apply_started(7);
        assert!(!t.apply_done(Err(ApplyFailed {
            reason: "netplan apply failed".into(),
            home_changed: false
        })));
        assert!(!t.applying());
        assert_eq!(t.pgid(), None);
        assert_eq!(t.error(), Some("netplan apply failed"));
        assert!(t.dirty());
        assert_eq!(t.wired_form().address, "192.168.1.10/24");
        // Tried again, the same change goes out.
        assert_eq!(t.event(NetEvent::Apply, false), Some(NetRequest::Apply(fixed_change())));
        assert_eq!(t.error(), None);
    }

    #[test]
    fn apply_failure_reloads_the_files() {
        let mut t = with_fixed_draft();
        let now = Instant::now();
        assert_eq!(t.poll(now, true), Some(NetRequest::Load));
        t.loaded(
            wifi(FileState::Menu, Some("wlp1"), vec![net("Home", Some("secret123"))]),
            wired(FileState::Menu, Wired::Dhcp),
            vec!["wlp1".to_string()],
        );
        t.event(NetEvent::Apply, false);
        assert!(!t.apply_done(Err(ApplyFailed {
            reason: "x".into(),
            home_changed: false
        })));
        assert_eq!(t.poll(now, true), Some(NetRequest::Load), "the home was put back");
        assert!(t.dirty(), "the draft is kept");
        assert_eq!(t.error(), Some("x"));
        t.loaded(
            wifi(FileState::Menu, Some("wlp1"), vec![net("Home", Some("secret123"))]),
            wired(FileState::Menu, Wired::Dhcp),
            vec!["wlp1".to_string()],
        );
        assert!(t.dirty(), "the draft outlives the reload");
        // The home could not be put back: it changed, and is saved.
        t.event(NetEvent::Apply, false);
        assert!(t.apply_done(Err(ApplyFailed {
            reason: "x; ~/wifi.yaml could not be put back: busy".into(),
            home_changed: true
        })));
        assert_eq!(t.poll(now, true), Some(NetRequest::Load));
    }

    #[test]
    fn adapter_changed_marks_the_draft_dirty() {
        let mut t = NetTab::new(false);
        let saved = vec![net("Home", Some("secret123"))];
        t.loaded(
            wifi(FileState::Menu, Some("wlp2s0"), saved.clone()),
            wired(FileState::Menu, Wired::Dhcp),
            vec!["wlp58s0".to_string()],
        );
        assert!(t.adapter_changed());
        assert!(t.dirty());
        assert!(!t.drop_draft(), "nothing typed, nothing dropped");
        assert!(t.dirty());
        assert_eq!(
            t.event(NetEvent::Apply, false),
            Some(NetRequest::Apply(Change {
                wifi: Some(saved),
                wired: None
            }))
        );
    }

    #[test]
    fn scan_failure_shows_in_dialog() {
        let mut t = tab();
        assert_eq!(t.event(NetEvent::OpenAdd, false), Some(NetRequest::Scan));
        assert!(t.add_view().unwrap().scanning);
        assert_eq!(t.event(NetEvent::Rescan, false), None, "a scan runs");
        t.scan_done(Err("No WiFi adapter found".into()));
        let v = t.add_view().unwrap();
        assert!(!v.scanning);
        assert_eq!(v.scan_error.as_deref(), Some("No WiFi adapter found"));
        assert_eq!(t.event(NetEvent::Rescan, false), Some(NetRequest::Scan));
        assert_eq!(t.add_view().unwrap().scan_error, None);
        t.scan_done(Ok(vec![in_range("A", -50, true, true)]));
        assert_eq!(t.event(NetEvent::CloseAdd, false), None);
        assert!(t.add_view().is_none());
        // A scan that ends after the dialog closed is dropped.
        t.event(NetEvent::OpenAdd, false);
        t.event(NetEvent::CloseAdd, false);
        t.scan_done(Ok(vec![in_range("A", -50, true, true)]));
        assert!(t.add_view().is_none());
    }

    #[test]
    fn drop_draft_says_whether_there_was_one() {
        let mut t = with_fixed_draft();
        t.event(NetEvent::Apply, true);
        assert!(t.drop_draft());
        assert!(!t.dirty());
        assert!(!t.confirm_open());
        assert!(!t.drop_draft());
    }

    #[test]
    fn nothing_before_the_files_are_loaded() {
        let mut t = NetTab::new(false);
        assert_eq!(t.event(NetEvent::WiredMode(true), false), None);
        assert_eq!(t.event(NetEvent::OpenAdd, false), None);
        assert!(!t.dirty());
        assert_eq!(t.event(NetEvent::Apply, false), None);
    }
}
