//! The single writer of `config.toml` and the files derived from it.
//!
//! Order of writes: generated files first, `config.toml` last. A crash
//! in between leaves `config.toml` older than `generated/`; the next
//! start regenerates from `config.toml`. A broken `config.toml` is never
//! replaced by defaults while `generated/` holds files: the settings are
//! rebuilt from those files instead.

use std::fs;
use std::sync::{Arc, Mutex};

use crate::config::{self, ConfigDir, ThinfloeConfig};
use crate::generated::{self, Inputs, Table};
use crate::{atomic, convert};

/// config.toml holds WiFi passwords and is read by nobody else.
const CONFIG_MODE: u32 = 0o600;
/// Far above any real file; keeps a runaway file out of memory.
pub const CONFIG_MAX_BYTES: u64 = 1024 * 1024;

/// Every generated file, in the order they are regenerated.
const ALL_FILES: [&str; 5] = ["environment", "screen", "kanshi", "wifi.yaml", "ethernet.yaml"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    /// The menu: repairs and regenerates.
    Session,
    /// `--check`: reads only.
    ReadOnly,
}

pub struct ConfigStore {
    pub dir: ConfigDir,
    pub cfg: ThinfloeConfig,
    pub inputs: Inputs,
    read_only: bool,
}

pub struct Opened {
    pub store: ConfigStore,
    /// What the user should be told: a broken file, a rebuild.
    pub notes: Vec<String>,
    /// The start-up regeneration changed `generated/kanshi`.
    pub kanshi_changed: bool,
}

pub type Shared = Arc<Mutex<ConfigStore>>;

/// A session store on `root`, for the tests of the code that takes one.
#[cfg(test)]
pub fn test_shared(root: &std::path::Path) -> Shared {
    let dir = ConfigDir {
        root: root.to_path_buf(),
    };
    Arc::new(Mutex::new(open(dir, Inputs::default(), OpenMode::Session, 0).store))
}

/// What `stage` found in a file it replaced.
#[derive(Debug)]
pub enum Old {
    Absent,
    Bytes(Vec<u8>),
    /// The read failed (a link, too big, no permission, an I/O error): the
    /// bytes are not known, so `unstage` writes what the settings render.
    Unreadable,
}

/// What `stage` replaced, for `unstage`.
#[derive(Debug)]
pub struct Staged {
    pub old: Vec<(&'static str, Old)>,
}

/// A refused `stage`. `restore`: the rollback of the files it had already
/// written failed too, with the first file named; those files stay as the
/// stage left them and the caller must treat the home as changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageError {
    pub reason: String,
    pub restore: Option<String>,
}

impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.restore {
            Some(r) => write!(f, "{}; {r}", self.reason),
            None => f.write_str(&self.reason),
        }
    }
}

impl From<String> for StageError {
    fn from(reason: String) -> Self {
        StageError { reason, restore: None }
    }
}

fn read_config(dir: &ConfigDir) -> Result<Option<ThinfloeConfig>, String> {
    match atomic::read(&dir.config_toml(), CONFIG_MAX_BYTES) {
        Ok(None) => Ok(None),
        // Strict: a lossy read would replace a byte of an SSID or a password
        // and write the change back. Invalid UTF-8 is a broken file.
        Ok(Some(b)) => String::from_utf8(b)
            .map_err(|e| format!("config.toml is not valid UTF-8 ({e})"))
            .and_then(|t| config::parse(&t))
            .map(Some),
        Err(e) => Err(format!("config.toml: {e}")),
    }
}

pub fn open(dir: ConfigDir, inputs: Inputs, mode: OpenMode, now_epoch: i64) -> Opened {
    let mut notes = Vec::new();
    let read = read_config(&dir);
    let mut store = ConfigStore {
        dir,
        cfg: ThinfloeConfig::default(),
        inputs,
        read_only: mode == OpenMode::ReadOnly,
    };
    if mode == OpenMode::ReadOnly {
        match read {
            Ok(Some(c)) => store.cfg = c,
            Ok(None) => notes.push("config.toml is missing; defaults are shown.".into()),
            Err(e) => notes.push(format!("config.toml could not be read ({e}); defaults are shown.")),
        }
        return Opened {
            store,
            notes,
            kanshi_changed: false,
        };
    }

    let (cfg, broken) = match read {
        Ok(Some(c)) => (c, None),
        Ok(None) | Err(_) => {
            let broken = read.err();
            match convert::from_generated(&store.dir.generated()) {
                Ok(Some(c)) => (c, broken),
                Ok(None) => (ThinfloeConfig::default(), broken),
                Err(e) => {
                    // Defaults would erase what generated/ still holds
                    // (a default network writes no wifi.yaml): keep
                    // everything as it is and refuse writes.
                    notes.push(format!(
                        "config.toml cannot be restored ({e}); nothing was changed and settings cannot be saved."
                    ));
                    store.read_only = true;
                    return Opened {
                        store,
                        notes,
                        kanshi_changed: false,
                    };
                }
            }
        }
    };
    if let Some(why) = broken {
        let kept = format!("config.toml.broken-{now_epoch}");
        match fs::rename(store.dir.config_toml(), store.dir.root.join(&kept)) {
            Ok(()) => notes.push(format!(
                "config.toml could not be read ({why}) and was kept as {kept}; the settings were rebuilt from generated/"
            )),
            Err(e) => notes.push(format!(
                "config.toml could not be read ({why}) and could not be kept as {kept} ({e}); the settings were rebuilt from generated/"
            )),
        }
    }
    store.cfg = cfg;

    let mut kanshi_changed = false;
    for name in ALL_FILES {
        let body = match generated::render(&store.cfg, name, &store.inputs) {
            Ok(b) => b,
            Err(e) => {
                notes.push(format!("generated/{name} was not regenerated: {e}"));
                continue;
            }
        };
        match generated::write_if_changed(&store.dir.generated(), name, body.as_deref()) {
            Ok(changed) => kanshi_changed |= changed && name == "kanshi",
            Err(e) => notes.push(format!("generated/{name} could not be written: {e}")),
        }
    }
    let text = config::to_text(&store.cfg);
    let current = atomic::read(&store.dir.config_toml(), CONFIG_MAX_BYTES);
    if !matches!(&current, Ok(Some(b)) if b == text.as_bytes())
        && let Err(e) = atomic::write(&store.dir.config_toml(), text.as_bytes(), CONFIG_MODE)
    {
        notes.push(format!("config.toml could not be written: {e}"));
    }
    Opened {
        store,
        notes,
        kanshi_changed,
    }
}

impl ConfigStore {
    fn write_config(&self, cfg: &ThinfloeConfig) -> Result<(), String> {
        atomic::write(&self.dir.config_toml(), config::to_text(cfg).as_bytes(), CONFIG_MODE)
            .map_err(|e| format!("config.toml could not be written: {e}"))
    }

    pub fn refuse_read_only(&self) -> Result<(), String> {
        if self.read_only {
            Err("The settings are read-only.".into())
        } else {
            Ok(())
        }
    }

    /// Render all files of `table` from `cfg`; one failing render fails
    /// the lot before anything is written.
    fn render_table(&self, table: Table, cfg: &ThinfloeConfig) -> Result<Vec<(&'static str, Option<String>)>, String> {
        table
            .files()
            .iter()
            .map(|&n| generated::render(cfg, n, &self.inputs).map(|b| (n, b)))
            .collect()
    }

    /// Apply `f` to a copy, regenerate the files of `table`, then write
    /// `config.toml`. Returns the generated files that changed. When a
    /// write fails the in-memory settings stay as they were.
    pub fn update(&mut self, table: Table, f: impl FnOnce(&mut ThinfloeConfig)) -> Result<Vec<&'static str>, String> {
        self.refuse_read_only()?;
        let mut cand = self.cfg.clone();
        f(&mut cand);
        let files = self.render_table(table, &cand)?;
        let mut changed = Vec::new();
        for (name, body) in files {
            let did = generated::write_if_changed(&self.dir.generated(), name, body.as_deref())
                .map_err(|e| format!("generated/{name} could not be written: {e}"))?;
            if did {
                changed.push(name);
            }
        }
        self.write_config(&cand)?;
        self.cfg = cand;
        Ok(changed)
    }

    /// Write the files of `table` as `candidate` renders them; `config.toml`
    /// stays. `unstage` undoes it, `commit` makes it final.
    pub fn stage(&self, table: Table, candidate: &ThinfloeConfig) -> Result<Staged, StageError> {
        self.refuse_read_only()?;
        let files = self.render_table(table, candidate)?;
        let mut staged = Staged { old: Vec::new() };
        for (name, body) in files {
            let path = self.dir.generated_file(name);
            // An old file that cannot be read is replaced like any other
            // (a link is replaced, never followed): refusing would block
            // every network Apply for good. `unstage` puts back what the
            // committed settings render, never nothing.
            let old = match atomic::read(&path, generated::MAX_BYTES) {
                Ok(Some(b)) => Old::Bytes(b),
                Ok(None) => Old::Absent,
                Err(_) => Old::Unreadable,
            };
            staged.old.push((name, old));
            let step = generated::write_if_changed(&self.dir.generated(), name, body.as_deref())
                .map(|_| ())
                .map_err(|e| format!("generated/{name} could not be written: {e}"));
            if let Err(reason) = step {
                let restore = self.unstage(staged).err();
                return Err(StageError { reason, restore });
            }
        }
        Ok(staged)
    }

    /// Put the files `stage` replaced back. Goes on after a failure and
    /// names the first file that could not be restored.
    pub fn unstage(&self, s: Staged) -> Result<(), String> {
        let mut first = None;
        for (name, old) in s.old {
            let path = self.dir.generated_file(name);
            let r = match old {
                Old::Bytes(b) => atomic::write(&path, &b, generated::mode_of(name)),
                Old::Absent => atomic::remove(&path),
                Old::Unreadable => match generated::render(&self.cfg, name, &self.inputs) {
                    Ok(Some(body)) => atomic::write(&path, body.as_bytes(), generated::mode_of(name)),
                    Ok(None) => atomic::remove(&path),
                    Err(e) => Err(std::io::Error::other(e)),
                },
            };
            if let Err(e) = r
                && first.is_none()
            {
                first = Some(format!("generated/{name} could not be restored: {e}"));
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// Make a staged candidate the settings: writes `config.toml` only.
    pub fn commit(&mut self, candidate: ThinfloeConfig) -> Result<(), String> {
        self.refuse_read_only()?;
        self.write_config(&candidate)?;
        self.cfg = candidate;
        Ok(())
    }

    pub fn has_generated(&self) -> bool {
        ALL_FILES
            .iter()
            .any(|n| fs::symlink_metadata(self.dir.generated_file(n)).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{NetCfg, parse, to_text};
    use crate::generated::{HEADER, write_if_changed};
    use crate::testdir::TestDir;
    use std::fs;

    const NOW: i64 = 1_700_000_000;

    fn inputs() -> Inputs {
        Inputs {
            default_wired: include_str!("../../../overlay/etc/netplan/ethernet.yaml").into(),
            netboot: false,
        }
    }

    fn cdir(d: &TestDir) -> ConfigDir {
        ConfigDir {
            root: d.join("thinfloe"),
        }
    }

    fn open_session(d: &TestDir) -> Opened {
        open(cdir(d), inputs(), OpenMode::Session, NOW)
    }

    fn one_net() -> ThinfloeConfig {
        let mut c = ThinfloeConfig::default();
        c.network.networks = vec![NetCfg {
            ssid: "home".into(),
            hidden: false,
            password: Some("secret-pw".into()),
        }];
        c.network.wifi_device = Some("wlan0".into());
        c
    }

    fn put_generated(d: &TestDir, name: &str, body: &str) {
        let g = cdir(d).generated();
        write_if_changed(&g, name, Some(body)).unwrap();
    }

    fn gen_text(d: &TestDir, name: &str) -> Option<String> {
        fs::read_to_string(cdir(d).generated_file(name)).ok()
    }

    #[test]
    fn a_fresh_home_gets_defaults_and_no_network_files() {
        let d = TestDir::new("store", "fresh");
        let o = open_session(&d);
        assert!(o.notes.is_empty(), "{:?}", o.notes);
        assert_eq!(o.store.cfg, ThinfloeConfig::default());
        assert!(cdir(&d).config_toml().exists());
        assert!(gen_text(&d, "environment").is_some());
        assert!(gen_text(&d, "screen").is_some());
        assert!(gen_text(&d, "wifi.yaml").is_none());
        assert!(gen_text(&d, "ethernet.yaml").is_none());
    }

    #[test]
    fn read_only_open_writes_nothing() {
        let d = TestDir::new("store", "readonly");
        let o = open(cdir(&d), inputs(), OpenMode::ReadOnly, NOW);
        assert_eq!(o.store.cfg, ThinfloeConfig::default());
        assert_eq!(fs::read_dir(&*d).unwrap().count(), 0);
        let before = o.store.cfg.clone();
        let mut o = o;
        assert!(o.store.update(Table::Screen, |c| c.screen.idle_timeout = 5).is_err());
        assert_eq!(o.store.cfg, before);
        assert_eq!(fs::read_dir(&*d).unwrap().count(), 0);
    }

    #[test]
    fn a_broken_config_is_kept_and_rebuilt_from_generated() {
        let d = TestDir::new("store", "broken");
        fs::create_dir_all(cdir(&d).root).unwrap();
        fs::write(cdir(&d).config_toml(), "garbage[").unwrap();
        let want = one_net();
        put_generated(
            &d,
            "wifi.yaml",
            &crate::generated::render(&want, "wifi.yaml", &inputs())
                .unwrap()
                .unwrap(),
        );
        let o = open_session(&d);
        let kept = cdir(&d).root.join("config.toml.broken-1700000000");
        assert_eq!(fs::read_to_string(kept).unwrap(), "garbage[");
        assert_eq!(o.store.cfg.network.networks, want.network.networks);
        assert_eq!(o.notes.len(), 1, "{:?}", o.notes);
        assert!(o.notes[0].contains("config.toml.broken-1700000000"), "{:?}", o.notes);
        assert!(parse(&fs::read_to_string(cdir(&d).config_toml()).unwrap()).is_ok());
    }

    #[test]
    fn a_broken_config_never_becomes_defaults_while_generated_has_files() {
        let d = TestDir::new("store", "neverdefaults");
        fs::create_dir_all(cdir(&d).root).unwrap();
        fs::write(cdir(&d).config_toml(), "garbage[").unwrap();
        let body = crate::generated::render(&one_net(), "wifi.yaml", &inputs())
            .unwrap()
            .unwrap();
        put_generated(&d, "wifi.yaml", &body);
        let _ = open_session(&d);
        assert_eq!(gen_text(&d, "wifi.yaml").unwrap(), body);
    }

    #[test]
    fn start_up_regeneration_repairs_newer_generated_files() {
        let d = TestDir::new("store", "repair");
        let mut c = ThinfloeConfig::default();
        c.screen.idle_timeout = 300;
        fs::create_dir_all(cdir(&d).root).unwrap();
        fs::write(cdir(&d).config_toml(), to_text(&c)).unwrap();
        put_generated(
            &d,
            "screen",
            &crate::generated::render(
                &{
                    let mut x = c.clone();
                    x.screen.idle_timeout = 900;
                    x
                },
                "screen",
                &inputs(),
            )
            .unwrap()
            .unwrap(),
        );
        let o = open_session(&d);
        let want = crate::generated::render(&c, "screen", &inputs()).unwrap().unwrap();
        assert_eq!(gen_text(&d, "screen").unwrap(), want);
        assert!(!o.kanshi_changed);
    }

    #[test]
    fn kanshi_changed_is_reported() {
        let d = TestDir::new("store", "kanshi");
        let mut c = ThinfloeConfig::default();
        c.monitors.kept = Some("profile { output * enable }\n".into());
        fs::create_dir_all(cdir(&d).root).unwrap();
        fs::write(cdir(&d).config_toml(), to_text(&c)).unwrap();
        put_generated(&d, "kanshi", &format!("{HEADER}\nsomething else\n"));
        let o = open_session(&d);
        assert!(o.kanshi_changed);
        assert!(gen_text(&d, "kanshi").unwrap().contains("profile"));
    }

    #[test]
    fn update_regenerates_only_its_table() {
        let d = TestDir::new("store", "only");
        let mut o = open_session(&d);
        let cand = one_net();
        o.store.stage(Table::Network, &cand).unwrap();
        let staged = gen_text(&d, "wifi.yaml").unwrap();
        let changed = o
            .store
            .update(Table::Keyboard, |c| c.keyboard.layout = "ch".into())
            .unwrap();
        assert_eq!(changed, ["environment"]);
        assert_eq!(gen_text(&d, "wifi.yaml").unwrap(), staged);
        assert!(gen_text(&d, "environment").unwrap().contains("XKB_DEFAULT_LAYOUT=ch"));
        assert_eq!(o.store.cfg.keyboard.layout, "ch");
        let on_disk = parse(&fs::read_to_string(cdir(&d).config_toml()).unwrap()).unwrap();
        assert_eq!(on_disk.keyboard.layout, "ch");
    }

    #[test]
    fn generated_files_are_written_before_config_toml() {
        let d = TestDir::new("store", "order");
        let mut o = open_session(&d);
        fs::remove_file(cdir(&d).config_toml()).unwrap();
        fs::create_dir(cdir(&d).config_toml()).unwrap();
        let r = o.store.update(Table::Keyboard, |c| c.keyboard.layout = "ch".into());
        assert!(r.is_err());
        assert!(gen_text(&d, "environment").unwrap().contains("XKB_DEFAULT_LAYOUT=ch"));
        assert_eq!(o.store.cfg.keyboard.layout, "us");
    }

    #[test]
    fn unstage_puts_the_old_bytes_back() {
        let d = TestDir::new("store", "unstage");
        let mut o = open_session(&d);
        put_generated(&d, "ethernet.yaml", &format!("{HEADER}\nold wired\n"));
        let wired_before = fs::read(cdir(&d).generated_file("ethernet.yaml")).unwrap();
        let mut cand = one_net();
        cand.network.kept_wired = Some("new wired\n".into());
        let s = o.store.stage(Table::Network, &cand).unwrap();
        assert!(gen_text(&d, "wifi.yaml").is_some());
        assert!(gen_text(&d, "ethernet.yaml").unwrap().contains("new wired"));
        o.store.unstage(s).unwrap();
        assert!(gen_text(&d, "wifi.yaml").is_none());
        assert_eq!(
            fs::read(cdir(&d).generated_file("ethernet.yaml")).unwrap(),
            wired_before
        );
        // config.toml untouched by stage/unstage
        o.store.commit(cand.clone()).unwrap();
        assert_eq!(o.store.cfg, cand);
        assert!(o.store.has_generated());
    }

    #[test]
    fn stage_replaces_an_old_file_it_cannot_read() {
        let d = TestDir::new("store", "unreadable-old");
        let o = open_session(&d);
        let gdir = cdir(&d).generated();
        let wifi = cdir(&d).generated_file("wifi.yaml");
        // A link, and a file too big, are no old bytes to keep: replaced.
        let target = d.join("elsewhere.yaml");
        fs::write(&target, "elsewhere: 1\n").unwrap();
        std::os::unix::fs::symlink(&target, &wifi).unwrap();
        let big = cdir(&d).generated_file("ethernet.yaml");
        fs::write(&big, vec![b'x'; generated::MAX_BYTES as usize + 1]).unwrap();
        let cand = one_net();
        let s = o.store.stage(Table::Network, &cand).unwrap();
        assert!(!fs::symlink_metadata(&wifi).unwrap().file_type().is_symlink());
        assert!(gen_text(&d, "wifi.yaml").unwrap().contains("secret-pw"));
        assert_eq!(
            fs::read(&target).unwrap(),
            b"elsewhere: 1\n",
            "the link was not followed"
        );
        // ethernet.yaml renders to nothing for DHCP: the big file is gone.
        assert!(fs::symlink_metadata(&big).is_err());
        // With nothing readable to put back, the replacements go.
        o.store.unstage(s).unwrap();
        assert!(fs::symlink_metadata(&wifi).is_err() && fs::symlink_metadata(&big).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"elsewhere: 1\n");
        assert!(gdir.exists());
    }

    #[test]
    fn invalid_utf8_in_config_takes_the_broken_path() {
        let d = TestDir::new("store", "utf8");
        let want = one_net();
        let mut bytes = to_text(&want).into_bytes();
        let at = bytes.windows(4).position(|w| w == b"home").unwrap();
        bytes[at + 2] = 0xff;
        fs::create_dir_all(cdir(&d).root).unwrap();
        fs::write(cdir(&d).config_toml(), &bytes).unwrap();
        let body = crate::generated::render(&want, "wifi.yaml", &inputs())
            .unwrap()
            .unwrap();
        put_generated(&d, "wifi.yaml", &body);
        let o = open_session(&d);
        let kept = cdir(&d).root.join("config.toml.broken-1700000000");
        assert_eq!(fs::read(kept).unwrap(), bytes, "the bytes are kept as they were");
        assert_eq!(o.store.cfg.network.networks, want.network.networks);
        assert!(o.notes[0].contains("UTF-8"), "{:?}", o.notes);
        assert_eq!(gen_text(&d, "wifi.yaml").unwrap(), body);
        assert!(parse(&fs::read_to_string(cdir(&d).config_toml()).unwrap()).is_ok());
        // Read-only: told, not rewritten.
        let d = TestDir::new("store", "utf8-ro");
        fs::create_dir_all(cdir(&d).root).unwrap();
        fs::write(cdir(&d).config_toml(), &bytes).unwrap();
        let o = open(cdir(&d), inputs(), OpenMode::ReadOnly, NOW);
        assert!(o.notes[0].contains("UTF-8"), "{:?}", o.notes);
        assert_eq!(fs::read(cdir(&d).config_toml()).unwrap(), bytes);
    }

    /// Review Focus 5: a keyboard change between a network stage and its
    /// undo leaves the staged network files alone, and the undo puts back
    /// exactly the network files.
    #[test]
    fn a_keyboard_change_during_a_stage_leaves_the_staged_files() {
        let d = TestDir::new("store", "keyboard-while-staged");
        let mut o = open_session(&d);
        put_generated(&d, "ethernet.yaml", &format!("{HEADER}\nold wired\n"));
        let wired_before = fs::read(cdir(&d).generated_file("ethernet.yaml")).unwrap();
        let s = o.store.stage(Table::Network, &one_net()).unwrap();
        let staged = gen_text(&d, "wifi.yaml").unwrap();
        o.store
            .update(Table::Keyboard, |c| c.keyboard.layout = "ch".into())
            .unwrap();
        assert_eq!(gen_text(&d, "wifi.yaml").unwrap(), staged);
        o.store.unstage(s).unwrap();
        assert!(gen_text(&d, "wifi.yaml").is_none());
        assert_eq!(
            fs::read(cdir(&d).generated_file("ethernet.yaml")).unwrap(),
            wired_before
        );
        assert!(gen_text(&d, "environment").unwrap().contains("XKB_DEFAULT_LAYOUT=ch"));
        assert_eq!(o.store.cfg.keyboard.layout, "ch");
        assert!(o.store.cfg.network.networks.is_empty());
    }

    #[test]
    fn a_rollback_that_fails_is_reported_with_the_file() {
        let d = TestDir::new("store", "rollback-fails");
        let mut o = open_session(&d);
        o.store
            .update(Table::Network, |c| c.network = one_net().network)
            .unwrap();
        let wifi_before = fs::read(cdir(&d).generated_file("wifi.yaml")).unwrap();
        let g = cdir(&d).generated();
        // The WiFi file renders as it is, so it is not written; its
        // rollback needs the temporary file the directory blocks. The wired
        // write fails on the directory in its place.
        fs::create_dir_all(g.join("wifi.yaml.tmp/x")).unwrap();
        fs::create_dir_all(g.join("ethernet.yaml/x")).unwrap();
        let mut cand = one_net();
        cand.network.kept_wired = Some("new wired\n".into());
        let e = o.store.stage(Table::Network, &cand).unwrap_err();
        assert!(e.reason.contains("ethernet.yaml"), "{e:?}");
        let r = e.restore.as_deref().unwrap_or_default();
        assert!(r.starts_with("generated/wifi.yaml could not be restored"), "{e:?}");
        assert_eq!(fs::read(g.join("wifi.yaml")).unwrap(), wifi_before);
    }

    #[test]
    fn an_unreadable_old_file_is_put_back_as_the_settings_render_it() {
        use std::os::unix::fs::PermissionsExt;
        let d = TestDir::new("store", "unreadable-put-back");
        let mut o = open_session(&d);
        o.store
            .update(Table::Network, |c| c.network = one_net().network)
            .unwrap();
        let wifi = cdir(&d).generated_file("wifi.yaml");
        let before = fs::read(&wifi).unwrap();
        fs::set_permissions(&wifi, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read(&wifi).is_ok() {
            return; // root reads anything: the case cannot be made
        }
        let mut cand = one_net();
        cand.network.networks[0].ssid = "cafe".into();
        let s = o.store.stage(Table::Network, &cand).unwrap();
        assert!(gen_text(&d, "wifi.yaml").unwrap().contains("cafe"));
        o.store.unstage(s).unwrap();
        // Not removed: the committed settings still say "home".
        assert_eq!(fs::read(&wifi).unwrap(), before);
        assert_eq!(fs::metadata(&wifi).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
