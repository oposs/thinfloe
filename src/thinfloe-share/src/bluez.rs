//! The AirDrop BLE advertisement over BlueZ's LE advertising API (BLE-1 to
//! BLE-3, BLE-5): the wake-up phones in "Everyone" mode need before they
//! answer `/Discover`.
//!
//! Ported from Owly's `crates/owly-cli/src/bluez.rs` at
//! 21171d8f9a08bfa2c625a1fa338e877a67913e54 (v0.4.3), which carries this notice:
//!
//! MIT License
//!
//! Copyright (c) 2026 T4T5
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.
//!
//! Changes for ThinFloe: [`advertise`] gives `Ok(None)` when BlueZ has no
//! adapter with LE advertising (or bluetoothd is not running),
//! [`Registration::stop`] unregisters before the runtime ends, and the
//! object path is ThinFloe's.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};

use owly_airdrop::proto::ble::{APPLE_COMPANY_ID, Advert, INTERVAL_MS};
use tokio::time::Instant;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const MANAGER: &str = "org.bluez.LEAdvertisingManager1";
const ADAPTER: &str = "org.bluez.Adapter1";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// Advertises through the first adapter BlueZ offers LE advertising on.
pub struct BluezAdvertiser {
    conn: zbus::Connection,
    adapter: OwnedObjectPath,
}

/// `f` before `deadline`; a timeout is an `io::ErrorKind::TimedOut` error naming `what`.
async fn until<T>(
    deadline: Instant,
    what: &str,
    f: impl std::future::Future<Output = io::Result<T>>,
) -> io::Result<T> {
    tokio::time::timeout_at(deadline, f)
        .await
        .unwrap_or_else(|_| {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("BlueZ did not answer ({what})"),
            ))
        })
}

impl BluezAdvertiser {
    /// Connects and finds the adapter, both before `deadline`.
    pub async fn new(deadline: Instant) -> io::Result<Self> {
        let conn = until(deadline, "connect", async {
            zbus::Connection::system().await.map_err(io::Error::other)
        })
        .await?;
        let adapter = until(deadline, "adapters", find_adapter(&conn)).await?;
        Ok(BluezAdvertiser { conn, adapter })
    }
}

/// Whether a call failed because bluetoothd is not on the bus.
fn no_bluez(e: &zbus::fdo::Error) -> bool {
    matches!(
        e,
        zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_)
    )
}

async fn find_adapter(conn: &zbus::Connection) -> io::Result<OwnedObjectPath> {
    let objects = zbus::fdo::ObjectManagerProxy::builder(conn)
        .destination("org.bluez")
        .and_then(|b| b.path("/"))
        .map_err(io::Error::other)?
        .build()
        .await
        .map_err(io::Error::other)?
        .get_managed_objects()
        .await
        .map_err(|e| {
            if no_bluez(&e) {
                io::Error::new(io::ErrorKind::NotFound, "bluetoothd is not running")
            } else {
                io::Error::other(e)
            }
        })?;
    let mut adapters: Vec<_> = objects
        .into_iter()
        .filter(|(_, interfaces)| interfaces.keys().any(|i| i.as_str() == MANAGER))
        .map(|(path, _)| path)
        .collect();
    adapters.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    adapters.into_iter().next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no Bluetooth adapter with LE advertising",
        )
    })
}

/// The `org.bluez.LEAdvertisement1` object BlueZ reads the advert from.
struct Advertisement {
    data: Vec<u8>,
}

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl Advertisement {
    /// BlueZ dropped the advert, e.g. because the adapter went away.
    fn release(&self) {}

    /// BLE-5: connectable, as Apple's own adverts are, though nothing
    /// connects to it.
    #[zbus(property, name = "Type")]
    fn kind(&self) -> &str {
        "peripheral"
    }

    /// BLE-5: the interval current Apple devices answer to.
    #[zbus(property)]
    fn min_interval(&self) -> u32 {
        INTERVAL_MS
    }

    #[zbus(property)]
    fn max_interval(&self) -> u32 {
        INTERVAL_MS
    }

    /// BLE-1: Apple's company ID, holding the AirDrop TLV.
    #[zbus(property)]
    fn manufacturer_data(&self) -> HashMap<u16, Value<'_>> {
        HashMap::from([(APPLE_COMPANY_ID, Value::from(self.data.clone()))])
    }
}

static NEXT: AtomicU32 = AtomicU32::new(0);

impl BluezAdvertiser {
    /// Transmits `advert` until the registration is dropped. Every BlueZ call of the start
    /// ends before `deadline`; after a failure or timeout the advert is unregistered and an
    /// adapter this turned on is turned off again, within `cleanup` more.
    pub async fn advertise(
        &self,
        advert: Advert,
        deadline: Instant,
        cleanup: std::time::Duration,
    ) -> io::Result<Registration> {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = ObjectPath::try_from(format!(
            "/ch/oetiker/ThinFloe/Share/advert{}_{n}",
            std::process::id()
        ))
        .map_err(io::Error::other)?
        .into();
        let advertisement = Advertisement {
            data: advert.manufacturer_data(),
        };
        until(deadline, "object", async {
            self.conn
                .object_server()
                .at(&path, advertisement)
                .await
                .map_err(io::Error::other)
        })
        .await?;
        let mut link = Link {
            conn: self.conn.clone(),
            adapter: self.adapter.clone(),
            path,
            powered_on: false,
        };
        let (powered, turned_on) =
            power_up(deadline, link.get_powered(), || link.set_powered(true)).await;
        link.powered_on = turned_on;
        if let Err(e) = powered {
            let _ = tokio::time::timeout(cleanup, link.unregister()).await;
            return Err(e);
        }
        if let Err(e) = until(deadline, "register", link.register()).await {
            let _ = tokio::time::timeout(cleanup, link.unregister()).await;
            return Err(e);
        }
        Ok(Registration(Some(link)))
    }
}

/// A registered advert's object, and where it's registered.
struct Link {
    conn: zbus::Connection,
    adapter: OwnedObjectPath,
    path: OwnedObjectPath,
    /// The adapter was off and this turned it on, so it's turned off again
    /// with the advert.
    powered_on: bool,
}

/// BlueZ only advertises on a powered adapter: `get` reads its Powered state and `set`
/// turns it on, both before `deadline`. Gives the result and whether this may have turned
/// the adapter on (then it is turned off with the advert). An unknown state, a read that
/// failed or timed out, never counts as turned on: an adapter the user had on (for a
/// Bluetooth keyboard) is never switched off. A Set that timed out may have reached BlueZ,
/// and the adapter was off before, so it counts; a Set BlueZ refused does not.
async fn power_up<F>(
    deadline: Instant,
    get: impl std::future::Future<Output = io::Result<bool>>,
    set: impl FnOnce() -> F,
) -> (io::Result<()>, bool)
where
    F: std::future::Future<Output = zbus::Result<()>>,
{
    match until(deadline, "power state", get).await {
        Err(e) => return (Err(e), false),
        Ok(true) => return (Ok(()), false),
        Ok(false) => {}
    }
    match until(deadline, "power", async {
        set().await.map_err(io::Error::other)
    })
    .await
    {
        Ok(()) => (Ok(()), true),
        Err(e) if e.kind() == io::ErrorKind::TimedOut => (Err(e), true),
        Err(e) => (
            Err(io::Error::other(format!(
                "Bluetooth is off and couldn't be turned on: {e}"
            ))),
            false,
        ),
    }
}

impl Link {
    /// The adapter's Powered property.
    async fn get_powered(&self) -> io::Result<bool> {
        let reply = self
            .conn
            .call_method(
                Some("org.bluez"),
                self.adapter.as_str(),
                Some(PROPERTIES),
                "Get",
                &(ADAPTER, "Powered"),
            )
            .await
            .map_err(io::Error::other)?;
        let powered: OwnedValue = reply.body().deserialize().map_err(io::Error::other)?;
        bool::try_from(powered).map_err(io::Error::other)
    }

    async fn set_powered(&self, on: bool) -> zbus::Result<()> {
        self.conn
            .call_method(
                Some("org.bluez"),
                self.adapter.as_str(),
                Some(PROPERTIES),
                "Set",
                &(ADAPTER, "Powered", Value::from(on)),
            )
            .await
            .map(|_| ())
    }
    async fn register(&self) -> io::Result<()> {
        let options = HashMap::<&str, Value>::new();
        self.conn
            .call_method(
                Some("org.bluez"),
                self.adapter.as_str(),
                Some(MANAGER),
                "RegisterAdvertisement",
                &(&self.path, options),
            )
            .await
            .map(|_| ())
            .map_err(io::Error::other)
    }

    async fn unregister(self) {
        let _ = self
            .conn
            .call_method(
                Some("org.bluez"),
                self.adapter.as_str(),
                Some(MANAGER),
                "UnregisterAdvertisement",
                &(&self.path,),
            )
            .await;
        let _ = self
            .conn
            .object_server()
            .remove::<Advertisement, _>(&self.path)
            .await;
        if self.powered_on {
            let _ = self.set_powered(false).await;
        }
    }
}

/// Unregisters the advert when dropped.
pub struct Registration(Option<Link>);

impl Registration {
    /// Unregisters the advert now, and turns the adapter off again if the
    /// advert turned it on.
    pub async fn stop(mut self) {
        if let Some(link) = self.0.take() {
            link.unregister().await;
        }
    }
}

/// Transmits `advert` through the first adapter with LE advertising until
/// the registration is stopped or dropped; `Ok(None)` when BlueZ has none.
/// The whole start takes at most `wait`; the cleanup after a failed start
/// (unregister, and power off what this turned on) at most `wait` more.
pub async fn advertise(
    advert: Advert,
    wait: std::time::Duration,
) -> io::Result<Option<Registration>> {
    let deadline = Instant::now() + wait;
    let advertiser = match BluezAdvertiser::new(deadline).await {
        Ok(a) => a,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    advertiser.advertise(advert, deadline, wait).await.map(Some)
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let (Some(link), Ok(runtime)) = (self.0.take(), tokio::runtime::Handle::try_current()) {
            runtime.spawn(link.unregister());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::pending;
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(10);

    fn refused() -> zbus::Error {
        zbus::Error::Failure("org.bluez.Error.Blocked".into())
    }

    #[tokio::test(start_paused = true)]
    async fn a_power_read_that_hangs_never_switches_the_adapter_off() {
        let (r, turned_on) = power_up(Instant::now() + WAIT, pending(), || async {
            panic!("no Set after an unknown state")
        })
        .await;
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(!turned_on);
    }

    #[tokio::test(start_paused = true)]
    async fn a_power_read_that_fails_never_switches_the_adapter_off() {
        let (r, turned_on) = power_up(
            Instant::now() + WAIT,
            async { Err(io::Error::other("no such property")) },
            || async { panic!("no Set after an unknown state") },
        )
        .await;
        assert!(r.is_err());
        assert!(!turned_on);
    }

    #[tokio::test(start_paused = true)]
    async fn an_adapter_that_was_on_is_left_alone() {
        let (r, turned_on) = power_up(Instant::now() + WAIT, async { Ok(true) }, || async {
            panic!("no Set for an adapter that is on")
        })
        .await;
        assert!(r.is_ok());
        assert!(!turned_on);
    }

    #[tokio::test(start_paused = true)]
    async fn an_adapter_turned_on_is_turned_off_again() {
        let (r, turned_on) = power_up(Instant::now() + WAIT, async { Ok(false) }, || async {
            Ok(())
        })
        .await;
        assert!(r.is_ok());
        assert!(turned_on);
    }

    #[tokio::test(start_paused = true)]
    async fn a_power_set_that_hangs_counts_as_turned_on() {
        let (r, turned_on) = power_up(Instant::now() + WAIT, async { Ok(false) }, pending).await;
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(turned_on);
    }

    #[tokio::test(start_paused = true)]
    async fn a_power_set_refused_does_not_count_as_turned_on() {
        let (r, turned_on) = power_up(Instant::now() + WAIT, async { Ok(false) }, || async {
            Err(refused())
        })
        .await;
        assert!(r.unwrap_err().to_string().contains("couldn't be turned on"));
        assert!(!turned_on);
    }

    #[tokio::test(start_paused = true)]
    async fn the_read_and_the_set_share_one_deadline() {
        let start = Instant::now();
        let get = async {
            tokio::time::sleep(Duration::from_secs(7)).await;
            Ok(false)
        };
        let (r, _) = power_up(start + WAIT, get, pending).await;
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert_eq!(start.elapsed(), WAIT);
    }
}
