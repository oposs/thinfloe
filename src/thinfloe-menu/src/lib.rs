//! thinfloe-menu: the tile grid that is the body of the thinfloe session.

/// Hash of the sources this binary was built from (`--source-key`), set by
/// the image build. It lives in the library, which holds the code: as an env
/// dependency it makes cargo rebuild the library when the sources change,
/// whatever their mtimes, and the binary with it. In main.rs it rebuilt only
/// main.rs and linked a stale library.
pub const SOURCE_KEY: &str = match option_env!("TF_SOURCE_KEY") {
    Some(key) => key,
    None => "none",
};

pub mod addons;
pub mod app;
pub mod atomic;
pub mod battery;
pub mod bounded;
pub mod brand;
pub mod config;
pub mod convert;
pub mod daemon_sup;
pub mod damage;
pub mod density;
pub mod deskflow;
pub mod deskflow_ui;
pub mod deskflowcfg;
pub mod deskflowtab;
pub mod desktop;
pub mod drives;
pub mod eject_ui;
pub mod generated;
pub mod graphics;
pub mod hardware;
pub mod header;
pub mod heads;
pub mod icons;
pub mod infopanel;
pub mod jobs;
pub mod kanshi;
pub mod kbregistry;
pub mod keyboard;
pub mod keyboardtab_ui;
pub mod log;
pub mod machine;
pub mod machine_ui;
pub mod menu;
pub mod menu_move;
pub mod migrate;
pub mod monitors;
pub mod monitors_ui;
pub mod netdetail;
pub mod netfiles;
pub mod netstat;
pub mod nettab;
pub mod network;
pub mod network_ui;
pub mod notes;
pub mod osd;
pub mod outputs;
pub mod pciids;
pub mod power;
pub mod publicip;
pub mod quiet;
pub mod raster;
pub mod release;
pub mod removable;
pub mod screentab;
pub mod settings;
pub mod share_state;
pub mod sharecfg;
pub mod soundtab;
pub mod soundtab_ui;
pub mod speed;
pub mod status;
pub mod statustab;
pub mod statustab_ui;
pub mod store;
#[cfg(test)]
mod testdir;
pub mod tiles;
pub mod timeband;
pub mod timecfg;
pub mod timetab;
pub mod ui;
pub mod upgrade;
pub mod upgrade_proto;
pub mod upgrade_ui;
pub mod windows;
