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
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const MANAGER: &str = "org.bluez.LEAdvertisingManager1";
const ADAPTER: &str = "org.bluez.Adapter1";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// Advertises through the first adapter BlueZ offers LE advertising on.
pub struct BluezAdvertiser {
    conn: zbus::Connection,
    adapter: OwnedObjectPath,
}

/// `f` within `wait`; a timeout is an `io::ErrorKind::TimedOut` error naming `what`.
async fn bounded<T>(
    wait: std::time::Duration,
    what: &str,
    f: impl std::future::Future<Output = io::Result<T>>,
) -> io::Result<T> {
    tokio::time::timeout(wait, f).await.unwrap_or_else(|_| {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("BlueZ did not answer ({what})"),
        ))
    })
}

impl BluezAdvertiser {
    pub async fn new(wait: std::time::Duration) -> io::Result<Self> {
        let conn = bounded(wait, "connect", async {
            zbus::Connection::system().await.map_err(io::Error::other)
        })
        .await?;
        let adapter = bounded(wait, "adapters", find_adapter(&conn)).await?;
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
    /// Transmits `advert` until the registration is dropped. Each BlueZ call takes at most
    /// `wait`; after a failure or timeout the advert is unregistered and an adapter this
    /// turned on is turned off again (those calls bounded by `wait` too).
    pub async fn advertise(
        &self,
        advert: Advert,
        wait: std::time::Duration,
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
        bounded(wait, "object", async {
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
        if let Err(e) = bounded(wait, "power", link.power_on()).await {
            // A timeout may come after BlueZ turned the adapter on: off again.
            if e.kind() == io::ErrorKind::TimedOut {
                link.powered_on = true;
            }
            let _ = tokio::time::timeout(wait, link.unregister()).await;
            return Err(e);
        }
        if let Err(e) = bounded(wait, "register", link.register()).await {
            let _ = tokio::time::timeout(wait, link.unregister()).await;
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

impl Link {
    /// BlueZ only advertises on a powered adapter.
    async fn power_on(&mut self) -> io::Result<()> {
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
        if bool::try_from(powered).unwrap_or(false) {
            return Ok(());
        }
        self.set_powered(true).await.map_err(|e| {
            io::Error::other(format!("Bluetooth is off and couldn't be turned on: {e}"))
        })?;
        self.powered_on = true;
        Ok(())
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
pub async fn advertise(
    advert: Advert,
    wait: std::time::Duration,
) -> io::Result<Option<Registration>> {
    let advertiser = match BluezAdvertiser::new(wait).await {
        Ok(a) => a,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    advertiser.advertise(advert, wait).await.map(Some)
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let (Some(link), Ok(runtime)) = (self.0.take(), tokio::runtime::Handle::try_current()) {
            runtime.spawn(link.unregister());
        }
    }
}
