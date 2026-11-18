//! The public IP address: the settings the Network tab keeps (`public_ip`
//! and `public_ip_url` in `[network]`) and the rule for a lookup address.
//! The lookup is off unless the person switches it on.

use crate::log::{Feed, Kind, Line, Run, local_time};
use crate::netstat;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The lookup address used while the field is empty.
pub const DEFAULT_URL: &str = "https://api.ipify.org";
/// Most bytes of a lookup address.
const URL_MAX: usize = 256;
const SCHEME: &str = "https://";

/// A lookup address the menu may ask: `https://` only, as for the add-on
/// sources fetched without asking, since the answer is fetched in the
/// background and shown as a fact. A host with at least one letter or
/// digit before a `:`, `/`, `?`, `#` or `@` ends it (so `https://:443`
/// and `https://@` are refused), no whitespace or
/// control character (the address becomes a program argument), no `[`,
/// `]`, `{` or `}` (curl's URL globbing would turn one lookup into many;
/// `--globoff` stops that too, and this keeps an IPv6 literal host out
/// with them), at most `URL_MAX` bytes.
pub fn url_ok(url: &str) -> bool {
    let Some(scheme) = url.get(..SCHEME.len()) else {
        return false;
    };
    let rest = &url[SCHEME.len()..];
    let host = rest.split([':', '/', '?', '#', '@']).next().unwrap_or("");
    scheme.eq_ignore_ascii_case(SCHEME)
        && host.chars().any(|c| c.is_ascii_alphanumeric())
        && url.len() <= URL_MAX
        && !url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '[' | ']' | '{' | '}'))
}

/// The address a lookup asks: the one set, or `DEFAULT_URL` when none is.
/// It is not checked here; the lookup checks it before each request.
pub fn effective_url(cfg_url: &str) -> &str {
    if cfg_url.is_empty() { DEFAULT_URL } else { cfg_url }
}

/// What the Network tab shows and saves: the switch and the lookup address
/// as `config.toml` holds them (`""` for the default).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PublicIpPrefs {
    pub on: bool,
    pub url: String,
}

/// What the last lookup gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Addr(IpAddr),
    Failed,
}

/// The most bytes of an answer's body.
pub const BODY_MAX: usize = 64;
/// The most bytes read from curl: the body, a newline and the status code.
const STDOUT_MAX: usize = 128;
/// How long curl may take: its own `--max-time 5` and time to start.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);
/// How often a running lookup looks at what is wanted.
const TICK: Duration = Duration::from_secs(1);
/// How often the address is asked again while nothing else changes.
pub const EVERY: Duration = Duration::from_secs(15 * 60);
/// The least time between a lookup and one a new route asks for: a link
/// that flaps asks at most this often.
pub const ROUTE_PAUSE: Duration = Duration::from_secs(30);

/// The address in a lookup's body: the trimmed text, when it is at most
/// `BODY_MAX` bytes and parses as an IPv4 or IPv6 address. A captive
/// portal's login page or an error text is no answer.
pub fn parse_answer(body: &[u8]) -> Option<IpAddr> {
    if body.len() > BODY_MAX {
        return None;
    }
    std::str::from_utf8(body).ok()?.trim_ascii().parse().ok()
}

/// curl's arguments for one lookup of `url`.
/// `-q` first (curl honours it only there): no `.curlrc` is read, since it
/// lives in the home that travels with the person and could set a proxy,
/// `--insecure` or an output file. `--globoff`: no `[]` or `{}` in the
/// address expands into more requests.
/// `--proto =https` and `--max-redirs 0` without `-L`: no redirect is
/// followed, and none could lead to another scheme. `--url` keeps an
/// address from ever being read as an option. The status code follows the
/// body on a line of its own.
pub fn curl_args(url: &str) -> Vec<String> {
    [
        "-q",
        "-s",
        "--globoff",
        "--proto",
        "=https",
        "--max-redirs",
        "0",
        "--max-time",
        "5",
        "--max-filesize",
        "64",
        "--write-out",
        "\n%{response_code}",
        "--url",
        url,
    ]
    .map(String::from)
    .to_vec()
}

/// Ask `url` for the public address with `curl` (the program and what runs
/// it). The address is checked before anything runs.
/// `config.toml` can be edited by hand and travels with the home, so the
/// check of the Network tab is not enough. Only status 200 with an address
/// as the body is an answer. No reason quotes the body: it is the server's
/// text and goes to the log.
pub fn lookup(curl: &[String], url: &str, timeout: Duration) -> Result<IpAddr, String> {
    if !url_ok(url) {
        return Err("the lookup address is not an https address".into());
    }
    let out = crate::quiet::run(curl, &curl_args(url), None, STDOUT_MAX, timeout)?;
    match out.status {
        Some(0) => {}
        Some(n) => return Err(format!("curl exited with {n}")),
        None => return Err("curl was killed".into()),
    }
    let split = out.stdout.iter().rposition(|&b| b == b'\n');
    let Some(at) = split else {
        return Err("no HTTP status".into());
    };
    let (body, code) = (&out.stdout[..at], &out.stdout[at + 1..]);
    if code != b"200" {
        return Err(if code.len() == 3 && code.iter().all(u8::is_ascii_digit) {
            format!("HTTP status {}", String::from_utf8_lossy(code))
        } else {
            "no HTTP status".into()
        });
    }
    parse_answer(body).ok_or_else(|| "the answer is not an address".into())
}

/// What the route of the network sampler's last copy is: interface,
/// gateway and IPv4 address. A change asks the address again.
pub type RouteKey = (Option<String>, Option<Ipv4Addr>, Option<Ipv4Addr>);

pub fn route_key(info: &netstat::Info) -> RouteKey {
    (info.iface.clone(), info.gateway, info.ipv4.map(|(a, _)| a))
}

/// What the app wants of the lookup, set on every frame.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Want {
    pub prefs: PublicIpPrefs,
    /// The connection element has cells on this screen.
    pub placed: bool,
    /// The network sampler reads.
    pub sampling: bool,
    pub route: RouteKey,
}

impl Want {
    /// The switch is on, the element has cells, the sampler reads and the
    /// route has an interface: not while covered, not on a short screen,
    /// not when unticked, not before the sampler's first copy and not while
    /// the link is gone (the lookup could only fail).
    pub fn active(&self) -> bool {
        self.prefs.on && self.placed && self.sampling && self.route.0.is_some()
    }
}

/// When the next lookup is due.
#[derive(Debug, Default)]
pub struct Schedule {
    /// When the last lookup was asked for, and on which route.
    last: Option<(Instant, RouteKey)>,
    /// The lookup address of `last`.
    url: String,
}

impl Schedule {
    /// Whether to ask now, and if so the ask counts from `now`. While
    /// active: the first time, on a new route (at least `ROUTE_PAUSE` after
    /// the last), and `EVERY` after the last.
    /// The switch going off, or a new lookup address, starts over.
    pub fn due(&mut self, want: &Want, now: Instant) -> bool {
        if !want.prefs.on || want.prefs.url != self.url {
            self.last = None;
            self.url.clone_from(&want.prefs.url);
        }
        if !want.active() {
            return false;
        }
        let due = match &self.last {
            None => true,
            Some((at, route)) => {
                let since = now.saturating_duration_since(*at);
                (*route != want.route && since >= ROUTE_PAUSE) || since >= EVERY
            }
        };
        if due {
            self.last = Some((now, want.route.clone()));
        }
        due
    }
}

/// Start the lookup thread. It looks at `want` every second, asks when the
/// schedule says so and keeps the last answer in the copy it returns.
/// The answer is cleared while the switch is off, so turning it on shows
/// nothing old. `wake` is called when the answer changes; one log line goes
/// out when the answer turns to `Failed`.
pub fn spawn(
    curl: Vec<String>,
    want: Arc<Mutex<Want>>,
    feed: Arc<Feed>,
    wake: impl Fn() + Send + 'static,
) -> Arc<Mutex<Option<Answer>>> {
    let answer: Arc<Mutex<Option<Answer>>> = Arc::default();
    let shared = answer.clone();
    std::thread::spawn(move || {
        let mut schedule = Schedule::default();
        let wanted = || want.lock().unwrap_or_else(|e| e.into_inner()).clone();
        loop {
            let w = wanted();
            let new = if !w.prefs.on {
                None
            } else if schedule.due(&w, Instant::now()) {
                // Outside every lock: the lookup takes seconds.
                let r = lookup(&curl, effective_url(&w.prefs.url), LOOKUP_TIMEOUT);
                // Switched off or pointed elsewhere meanwhile: the answer
                // is for a question no longer asked.
                let now = wanted();
                if now.prefs != w.prefs {
                    continue;
                }
                Some(r)
            } else {
                std::thread::sleep(TICK);
                continue;
            };
            let next = new.as_ref().map(|r| match r {
                Ok(a) => Answer::Addr(*a),
                Err(_) => Answer::Failed,
            });
            // A poisoned lock still holds a usable answer; ending the
            // thread would stop the lookup silently.
            let mut held = shared.lock().unwrap_or_else(|e| e.into_inner());
            let changed = *held != next;
            if next == Some(Answer::Failed)
                && *held != Some(Answer::Failed)
                && let Some(Err(reason)) = &new
            {
                feed.push(Line {
                    time: local_time(),
                    source: Some("public-ip".into()),
                    runs: vec![Run::plain(format!("public IP lookup failed: {reason}"))],
                    kind: Kind::Warn,
                });
            }
            *held = next;
            drop(held);
            if changed {
                wake();
            }
            std::thread::sleep(TICK);
        }
    });
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    #[test]
    fn only_https_lookup_addresses_are_accepted() {
        for good in [
            "https://api.ipify.org",
            "HTTPS://ip.example.net/plain",
            "https://ip.example.net:8443/",
        ] {
            assert!(url_ok(good), "{good}");
        }
        let long = format!("https://{}", "a".repeat(257 - "https://".len()));
        assert_eq!(long.len(), 257);
        for bad in [
            "http://api.ipify.org",
            "https://",
            "file:///etc/passwd",
            "https://a b",
            "https://a\n",
            "-K/etc/x",
            "https://:443",
            "https://@",
            "https://?x",
            "https://#x",
            "https://x.example/[1-100000]",
            "https://x.example/{a,b}",
            "https://x.example/]",
            "https://x.example/}",
            "https://./",
            long.as_str(),
        ] {
            assert!(!url_ok(bad), "{bad:?}");
        }
        assert!(url_ok(&long[..256]));
    }

    #[test]
    fn an_empty_lookup_address_is_the_default() {
        assert_eq!(effective_url(""), DEFAULT_URL);
        assert_eq!(DEFAULT_URL, "https://api.ipify.org");
        assert_eq!(effective_url("https://ip.example.net"), "https://ip.example.net");
    }

    #[test]
    fn public_ip_answers_parse_only_as_an_address() {
        assert_eq!(parse_answer(b" 203.0.113.7\n"), Some(IpAddr::from([203, 0, 113, 7])));
        assert_eq!(parse_answer(b"2001:db8::1"), Some("2001:db8::1".parse().unwrap()));
        let digits = [b'1'; 65];
        for bad in [
            &b"<html><body>Login</body></html>"[..],
            b"",
            &digits[..],
            b"203.0.113.7 extra",
            b"\xff203.0.113.7",
        ] {
            assert_eq!(parse_answer(bad), None, "{:?}", String::from_utf8_lossy(bad));
        }
    }

    /// A fake curl: a shell script run as `curl` that writes `out` and
    /// touches `marker` when it runs.
    fn fake_curl(marker: &std::path::Path, out: &str) -> Vec<String> {
        let script = format!("touch '{}'; printf '{out}'", marker.display());
        ["/bin/sh", "-c", &script, "curl"].map(String::from).to_vec()
    }

    #[test]
    fn a_lookup_refuses_a_bad_address_without_running_curl() {
        let d = TestDir::new("publicip", "refuse");
        let marker = d.join("ran");
        let curl = fake_curl(&marker, "203.0.113.7\\n200");
        for bad in ["http://x", "file:///etc/passwd", "-K/etc/x"] {
            assert!(lookup(&curl, bad, Duration::from_secs(5)).is_err(), "{bad}");
            assert!(!marker.exists(), "{bad}: curl ran");
        }
    }

    #[test]
    fn the_lookup_follows_no_redirect() {
        let args = curl_args("https://ip.example.net");
        let has = |pair: [&str; 2]| args.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1]);
        assert!(has(["--max-redirs", "0"]), "{args:?}");
        assert!(has(["--proto", "=https"]), "{args:?}");
        assert!(has(["--url", "https://ip.example.net"]), "{args:?}");
        assert_eq!(args.last().map(String::as_str), Some("https://ip.example.net"));
        assert!(!args.iter().any(|a| a == "-L" || a == "--location"), "{args:?}");
        assert_eq!(
            args,
            [
                "-q",
                "-s",
                "--globoff",
                "--proto",
                "=https",
                "--max-redirs",
                "0",
                "--max-time",
                "5",
                "--max-filesize",
                "64",
                "--write-out",
                "\n%{response_code}",
                "--url",
                "https://ip.example.net",
            ]
        );
        // `-q` reaches curl as its very first argument, where it counts.
        let d = TestDir::new("publicip", "first-arg");
        let first = d.join("first");
        let script = format!("printf '%s' \"$1\" > '{}'; printf '203.0.113.7\\n200'", first.display());
        let curl = ["/bin/sh", "-c", &script, "curl"].map(String::from).to_vec();
        assert!(lookup(&curl, "https://ip.example.net", Duration::from_secs(5)).is_ok());
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "-q");
        let d = TestDir::new("publicip", "redirect");
        let marker = d.join("ran");
        let url = "https://ip.example.net";
        let moved = lookup(&fake_curl(&marker, "203.0.113.7\\n301"), url, Duration::from_secs(5));
        assert!(moved.is_err(), "{moved:?}");
        assert!(marker.exists());
        assert_eq!(
            lookup(&fake_curl(&marker, "203.0.113.7\\n200"), url, Duration::from_secs(5)),
            Ok(IpAddr::from([203, 0, 113, 7]))
        );
    }

    #[test]
    fn a_lookup_takes_only_a_clean_answer() {
        let d = TestDir::new("publicip", "clean");
        let marker = d.join("ran");
        let url = "https://ip.example.net";
        let t = Duration::from_secs(5);
        // A failed curl, a page that is no address, a body past 64 bytes.
        let failed = format!("{}; exit 7", fake_curl(&marker, "\\n000")[2]);
        let curl = ["/bin/sh", "-c", &failed, "curl"].map(String::from).to_vec();
        assert!(lookup(&curl, url, t).is_err());
        assert!(lookup(&fake_curl(&marker, "<html>Login</html>\\n200"), url, t).is_err());
        let long = format!("{}\\n200", "1".repeat(65));
        assert!(lookup(&fake_curl(&marker, &long), url, t).is_err());
        assert!(
            lookup(&fake_curl(&marker, "203.0.113.7"), url, t).is_err(),
            "no status line"
        );
    }

    #[test]
    fn the_route_key_is_the_interface_the_gateway_and_the_address() {
        let info = netstat::Info {
            iface: Some("eth0".into()),
            ipv4: Some((Ipv4Addr::new(192, 168, 1, 23), 24)),
            gateway: Some(Ipv4Addr::new(192, 168, 1, 1)),
            ..netstat::Info::default()
        };
        assert_eq!(
            route_key(&info),
            (
                Some("eth0".into()),
                Some(Ipv4Addr::new(192, 168, 1, 1)),
                Some(Ipv4Addr::new(192, 168, 1, 23))
            )
        );
    }

    fn on() -> PublicIpPrefs {
        PublicIpPrefs {
            on: true,
            url: String::new(),
        }
    }

    fn active() -> Want {
        Want {
            prefs: on(),
            placed: true,
            sampling: true,
            route: (Some("eth0".into()), None, None),
        }
    }

    /// The seconds in the first hour at which a lookup is due, with
    /// `want(t)` the wish at second `t`.
    fn due_at(want: impl Fn(u64) -> Want) -> Vec<u64> {
        let start = Instant::now();
        let mut s = Schedule::default();
        (0..3600)
            .filter(|&t| s.due(&want(t), start + Duration::from_secs(t)))
            .collect()
    }

    #[test]
    fn the_lookup_runs_only_while_wanted() {
        assert!(active().active());
        let off = Want {
            prefs: PublicIpPrefs::default(),
            ..active()
        };
        assert!(!off.active());
        assert_eq!(due_at(|_| off.clone()), Vec::<u64>::new());
        let unplaced = Want {
            placed: false,
            ..active()
        };
        assert_eq!(due_at(|_| unplaced.clone()), Vec::<u64>::new());
        let resting = Want {
            sampling: false,
            ..active()
        };
        assert_eq!(due_at(|_| resting.clone()), Vec::<u64>::new());
        assert_eq!(due_at(|_| active()), [0, 900, 1800, 2700]);
        // A new route at 5 min asks at once, and the quarter hours count
        // from there.
        let moved = |t: u64| Want {
            route: if t < 300 {
                active().route
            } else {
                (Some("wlan0".into()), None, None)
            },
            ..active()
        };
        assert_eq!(due_at(moved), [0, 300, 1200, 2100, 3000]);
        // Off at 20 min, on at 21 min: asked again at once.
        let toggled = |t: u64| {
            if (1200..1260).contains(&t) {
                off.clone()
            } else {
                active()
            }
        };
        assert_eq!(due_at(toggled), [0, 900, 1260, 2160, 3060]);
        // A new lookup address asks at once too.
        let other = |t: u64| Want {
            prefs: PublicIpPrefs {
                on: true,
                url: if t < 600 {
                    String::new()
                } else {
                    "https://ip.example.net".into()
                },
            },
            ..active()
        };
        assert_eq!(due_at(other), [0, 600, 1500, 2400, 3300]);
        // No interface: before the sampler's first copy, or with the link
        // gone, nothing is asked; the link back asks at once.
        let no_link = Want {
            route: (None, None, None),
            ..active()
        };
        assert!(!no_link.active());
        assert_eq!(due_at(|_| no_link.clone()), Vec::<u64>::new());
        let lost = |t: u64| {
            if (100..400).contains(&t) {
                no_link.clone()
            } else {
                active()
            }
        };
        assert_eq!(
            due_at(lost),
            [0, 900, 1800, 2700],
            "the same route back is no new route"
        );
        let late = |t: u64| if t < 120 { no_link.clone() } else { active() };
        assert_eq!(due_at(late), [120, 1020, 1920, 2820]);
    }

    #[test]
    fn a_flapping_link_asks_at_most_every_30_s() {
        // The route changes every 5 s for the first 10 minutes: a lookup
        // starts at most every `ROUTE_PAUSE`, then the quarter hours count
        // from the last one.
        let flap = |t: u64| Want {
            route: if t < 600 && (t / 5) % 2 == 1 {
                (Some("wlan0".into()), None, None)
            } else {
                active().route
            },
            ..active()
        };
        let got = due_at(flap);
        for w in got.windows(2) {
            assert!(w[1] - w[0] >= 30, "{got:?}");
        }
        // Without the pause it would be 120 lookups in those 10 minutes.
        let flapping = got.iter().filter(|&&t| t < 600).count();
        assert!(flapping <= 600 / 30, "{got:?}");
        // The last flap (wlan0 at 595) is followed by the route back at 600,
        // asked 30 s later, then every quarter hour.
        assert_eq!(
            got.iter().copied().filter(|&t| t >= 600).collect::<Vec<_>>(),
            [625, 1525, 2425, 3325]
        );
        assert_eq!(ROUTE_PAUSE, Duration::from_secs(30));
    }

    /// Wait up to 20 s for `cond`: the thread looks once a second, and a
    /// loaded machine may be slow to run the fake curl.
    fn wait_for(what: &str, cond: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(20);
        while !cond() {
            assert!(Instant::now() < until, "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn the_lookup_thread_logs_a_failure_once_and_clears_when_off() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let d = TestDir::new("publicip", "thread");
        let runs = d.join("runs");
        let script = format!("echo x >> '{}'; printf '\\n000'; exit 7", runs.display());
        let curl = ["/bin/sh", "-c", &script, "curl"].map(String::from).to_vec();
        let want = Arc::new(Mutex::new(active()));
        let feed = Arc::new(Feed::default());
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let answer = spawn(curl, want.clone(), feed.clone(), move || {
            w.fetch_add(1, Ordering::SeqCst);
        });
        let ran = || std::fs::read_to_string(&runs).map_or(0, |t| t.lines().count());
        let got = || answer.lock().unwrap().clone();
        wait_for("the first lookup fails", || got() == Some(Answer::Failed));
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        // A new lookup address asks at once (a new route would wait
        // `ROUTE_PAUSE`).
        let url = |w: &Arc<Mutex<Want>>, u: &str| w.lock().unwrap().prefs.url = u.into();
        // Two more failures: the thread is sequential, so once the third has
        // run the second's answer is in.
        url(&want, "https://a.example");
        wait_for("the second lookup runs", || ran() == 2);
        url(&want, "https://b.example");
        wait_for("the third lookup runs", || ran() == 3);
        let lines = feed.drain();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0].source.as_deref(), Some("public-ip"));
        assert_eq!(lines[0].kind, Kind::Warn);
        assert_eq!(lines[0].plain(), "public IP lookup failed: curl exited with 7");
        // Switched off: the answer goes, and the change wakes the loop.
        want.lock().unwrap().prefs.on = false;
        wait_for("the answer is cleared", || got().is_none());
        wait_for("the clearing wakes", || wakes.load(Ordering::SeqCst) == 2);
    }
}
