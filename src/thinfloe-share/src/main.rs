//! FloeDrop: AirDrop with iPhones through Owly.

// Used from the daemon (later tasks); allowed unused until then.
#[allow(dead_code)]
mod names;

/// Owly release this build is pinned to; keep in step with Cargo.toml and the Dockerfile.
const OWLY_VERSION: &str = "0.4.3";
const OWLY_REV: &str = "21171d8";

fn version_line() -> String {
    format!(
        "thinfloe-share {} (owly {} {})",
        env!("CARGO_PKG_VERSION"),
        OWLY_VERSION,
        OWLY_REV
    )
}

fn main() {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("--version") => println!("{}", version_line()),
        _ => {
            eprintln!("usage: thinfloe-share --version");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_names_owly_pin() {
        assert_eq!(version_line(), "thinfloe-share 0.1.0 (owly 0.4.3 21171d8)");
    }
}
