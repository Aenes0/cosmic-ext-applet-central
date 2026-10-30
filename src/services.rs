// SPDX-License-Identifier: GPL-3.0-only

//! The system services behind the controls: NetworkManager, BlueZ, UPower,
//! power-profiles-daemon, PipeWire (through `wpctl`), cosmic-settings-daemon
//! for brightness, and COSMIC's own settings.
//!
//! Each service is read again whenever it says something changed.

use std::{collections::HashMap, time::Duration};

use cosmic::{
    cosmic_config::{Config, ConfigGet, ConfigSet},
    iced::{Subscription, stream},
};
use futures::{SinkExt, StreamExt, channel::mpsc};
use tokio::{io::AsyncBufReadExt, process::Command};
use tokio::sync::OnceCell;

use crate::latest::Latest;
use zbus::{
    MatchRule, MessageStream, fdo,
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value},
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Network {
    pub wifi_enabled: bool,
    /// Name and kind of the connection in use: `802-11-wireless`, `802-3-ethernet`…
    pub connection: Option<(String, String)>,
    /// The VPNs set up, WireGuard's too, the last used first.
    pub vpns: Vec<Vpn>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vpn {
    /// Its saved connection.
    pub path: OwnedObjectPath,
    pub name: String,
    /// The active connection, while on or connecting.
    pub active: Option<OwnedObjectPath>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bluetooth {
    pub adapter: Option<OwnedObjectPath>,
    pub powered: bool,
    pub connected: Vec<String>,
    /// Paired devices.
    pub devices: Vec<BluetoothDevice>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BluetoothDevice {
    pub path: OwnedObjectPath,
    pub name: String,
    pub icon: String,
    pub connected: bool,
    /// Percent, when the device tells BlueZ.
    pub battery: Option<u8>,
}

/// A wireless mouse, keyboard, headset… with a battery.
#[derive(Clone, Debug, PartialEq)]
pub struct Peripheral {
    pub name: String,
    pub icon: &'static str,
    pub percentage: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Battery {
    pub percentage: f64,
    pub charging: bool,
    pub icon: String,
    /// Seconds until empty, or until full when charging, once UPower knows.
    pub time_left: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Volume {
    /// 0.0 to 1.0, more when boosted.
    pub level: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PowerProfiles {
    /// Power-profiles-daemon's, or [`SUPER_SAVER`] while that's on.
    pub active: String,
    /// With [`SUPER_SAVER`] once its helper is installed.
    pub available: Vec<String>,
}

/// Our own profile past power-saver: the CPU without boost and the GPU at its
/// lowest power limit, by the `super-economia` helper
/// (github.com/Aenes0/super-economia), which needs root.
pub const SUPER_SAVER: &str = "super-saver";
const SUPER_SAVER_HELPER: &str = "/usr/local/bin/super-economia";
/// What the helper emits when it changes, as power-saver may stay put.
const SUPER_SAVER_INTERFACE: &str = "io.github.Aenes0.SuperEconomia";

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct Sink {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    Network(Network),
    Bluetooth(Bluetooth),
    Battery(Option<Battery>),
    Volume(Option<Volume>),
    /// Current and maximum.
    Brightness(Option<(i32, i32)>),
    PowerProfiles(Option<PowerProfiles>),
    /// Whether the default microphone is muted; `None` without one.
    Microphone(Option<bool>),
    Peripherals(Vec<Peripheral>),
    /// Apps' streams or the microphones changed.
    Audio,
}

#[derive(Clone, Copy, Debug, Hash)]
enum Service {
    Network,
    Bluetooth,
    Battery,
    Peripherals,
    PowerProfiles,
    Brightness,
}

impl Service {
    fn name(self) -> &'static str {
        match self {
            Self::Network => "org.freedesktop.NetworkManager",
            Self::Bluetooth => "org.bluez",
            Self::Battery | Self::Peripherals => "org.freedesktop.UPower",
            Self::PowerProfiles => "net.hadess.PowerProfiles",
            Self::Brightness => "com.system76.CosmicSettingsDaemon",
        }
    }

    fn rules(self) -> Vec<MatchRule<'static>> {
        let name = self.name();
        let mut rules = vec![
            match self {
                // Devices come and go, too.
                Self::Bluetooth | Self::Peripherals => signals_from(name),
                _ => properties_changed(name),
            },
            // It starting, late at login (COSMIC's settings daemon often
            // comes up after the panel), or again after a crash: it says
            // nothing else then.
            owner_changed(name),
        ];
        if let Self::PowerProfiles = self {
            rules.push(
                MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .interface(SUPER_SAVER_INTERFACE)
                    .unwrap()
                    .build(),
            );
        }
        rules
    }

    async fn bus(self) -> zbus::Result<zbus::Connection> {
        match self {
            Self::Brightness => zbus::Connection::session().await,
            _ => zbus::Connection::system().await,
        }
    }

    async fn read(self, conn: &zbus::Connection) -> Update {
        match self {
            Self::Network => Update::Network(network(conn).await.unwrap_or_default()),
            Self::Bluetooth => Update::Bluetooth(bluetooth(conn).await.unwrap_or_default()),
            Self::Battery => Update::Battery(battery(conn).await.ok().flatten()),
            Self::Peripherals => Update::Peripherals(peripherals(conn).await.unwrap_or_default()),
            Self::PowerProfiles => Update::PowerProfiles(power_profiles(conn).await.ok()),
            Self::Brightness => Update::Brightness(brightness(conn).await.ok().flatten()),
        }
    }
}

/// Read a service now and again whenever it says something changed.
fn watch(service: Service) -> Subscription<Update> {
    Subscription::run_with(service, |service| {
        let service = *service;
        stream::channel(4, move |mut output: mpsc::Sender<Update>| async move {
            let Ok(conn) = service.bus().await else {
                tracing::warn!("{service:?}: no bus");
                return futures::future::pending().await;
            };
            let mut streams = Vec::new();
            for rule in service.rules() {
                match MessageStream::for_match_rule(rule, &conn, Some(64)).await {
                    Ok(stream) => streams.push(stream),
                    Err(err) => tracing::warn!("{service:?}: {err}"),
                }
            }
            let mut changes = futures::stream::select_all(streams);
            loop {
                if output.send(service.read(&conn).await).await.is_err() {
                    return;
                }
                if changes.next().await.is_none() {
                    return futures::future::pending().await;
                }
                // Changes come in bursts.
                tokio::time::sleep(Duration::from_millis(100)).await;
                while futures::FutureExt::now_or_never(changes.next())
                    .flatten()
                    .is_some()
                {}
            }
        })
    })
}

fn properties_changed(sender: &'static str) -> MatchRule<'static> {
    MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(sender)
        .unwrap()
        .interface("org.freedesktop.DBus.Properties")
        .unwrap()
        .member("PropertiesChanged")
        .unwrap()
        .build()
}

fn owner_changed(name: &'static str) -> MatchRule<'static> {
    MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")
        .unwrap()
        .interface("org.freedesktop.DBus")
        .unwrap()
        .member("NameOwnerChanged")
        .unwrap()
        .arg(0, name)
        .unwrap()
        .build()
}

fn signals_from(sender: &'static str) -> MatchRule<'static> {
    MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(sender)
        .unwrap()
        .build()
}

pub fn subscription() -> Subscription<Update> {
    Subscription::batch([
        watch(Service::Network),
        watch(Service::Bluetooth),
        watch(Service::Battery),
        watch(Service::Peripherals),
        watch(Service::PowerProfiles),
        watch(Service::Brightness),
        volume_subscription(),
    ])
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
trait NetworkManager {
    #[zbus(property)]
    fn wireless_enabled(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_wireless_enabled(&self, enabled: bool) -> zbus::Result<()>;
    #[zbus(property)]
    fn primary_connection(&self) -> zbus::Result<OwnedObjectPath>;
    #[zbus(property)]
    fn active_connections(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Connection.Active",
    default_service = "org.freedesktop.NetworkManager"
)]
trait ActiveConnection {
    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;
    #[zbus(property, name = "Type")]
    fn kind(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn connection(&self) -> zbus::Result<OwnedObjectPath>;
}

async fn network(conn: &zbus::Connection) -> zbus::Result<Network> {
    let nm = NetworkManagerProxy::new(conn).await?;
    let wifi_enabled = nm.wireless_enabled().await?;
    let primary = nm.primary_connection().await?;
    let connection = if primary.as_str() == "/" {
        None
    } else {
        let active = ActiveConnectionProxy::builder(conn)
            .path(primary)?
            .build()
            .await?;
        Some((active.id().await?, active.kind().await?))
    };
    Ok(Network {
        wifi_enabled,
        connection,
        vpns: vpns(conn, &nm).await.unwrap_or_default(),
    })
}

async fn vpns(conn: &zbus::Connection, nm: &NetworkManagerProxy<'_>) -> zbus::Result<Vec<Vpn>> {
    // Saved connection to active one.
    let mut active = HashMap::new();
    for path in nm.active_connections().await? {
        let proxy = ActiveConnectionProxy::builder(conn).path(path.clone())?.build().await?;
        if let Ok(connection) = proxy.connection().await {
            active.insert(connection, path);
        }
    }
    let mut vpns = Vec::new();
    for path in NmSettingsProxy::new(conn).await?.list_connections().await? {
        let connection = NmConnectionProxy::builder(conn).path(path.clone())?.build().await?;
        let Ok(settings) = connection.get_settings().await else {
            continue;
        };
        let Some(about) = settings.get("connection") else {
            continue;
        };
        let text = |key: &str| about.get(key).and_then(|v| String::try_from(v.clone()).ok());
        if !matches!(text("type").as_deref(), Some("vpn" | "wireguard")) {
            continue;
        }
        let used = about
            .get("timestamp")
            .and_then(|v| u64::try_from(v.clone()).ok())
            .unwrap_or(0);
        vpns.push((
            used,
            Vpn {
                active: active.remove(&path),
                name: text("id").unwrap_or_default(),
                path,
            },
        ));
    }
    vpns.sort_by_key(|(used, vpn)| (std::cmp::Reverse(*used), vpn.name.clone()));
    Ok(vpns.into_iter().map(|(_, vpn)| vpn).collect())
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
trait NetworkManagerMethods {
    fn get_devices(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
    fn deactivate_connection(&self, active: &ObjectPath<'_>) -> zbus::Result<()>;
    fn activate_connection(
        &self,
        connection: &ObjectPath<'_>,
        device: &ObjectPath<'_>,
        specific_object: &ObjectPath<'_>,
    ) -> zbus::Result<OwnedObjectPath>;
    fn add_and_activate_connection(
        &self,
        connection: HashMap<&str, HashMap<&str, Value<'_>>>,
        device: &ObjectPath<'_>,
        specific_object: &ObjectPath<'_>,
    ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Device",
    default_service = "org.freedesktop.NetworkManager"
)]
trait NmDevice {
    #[zbus(property)]
    fn device_type(&self) -> zbus::Result<u32>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Device.Wireless",
    default_service = "org.freedesktop.NetworkManager"
)]
trait NmWireless {
    fn request_scan(&self, options: HashMap<&str, Value<'_>>) -> zbus::Result<()>;
    #[zbus(property)]
    fn access_points(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
    #[zbus(property)]
    fn active_access_point(&self) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.AccessPoint",
    default_service = "org.freedesktop.NetworkManager"
)]
trait NmAccessPoint {
    #[zbus(property)]
    fn ssid(&self) -> zbus::Result<Vec<u8>>;
    #[zbus(property)]
    fn strength(&self) -> zbus::Result<u8>;
    #[zbus(property)]
    fn flags(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn wpa_flags(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn rsn_flags(&self) -> zbus::Result<u32>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Settings",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager/Settings"
)]
trait NmSettings {
    fn list_connections(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Settings.Connection",
    default_service = "org.freedesktop.NetworkManager"
)]
trait NmConnection {
    fn get_settings(&self) -> zbus::Result<HashMap<String, HashMap<String, OwnedValue>>>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccessPoint {
    pub path: OwnedObjectPath,
    pub ssid: String,
    pub strength: u8,
    pub secure: bool,
    pub active: bool,
    /// A saved connection for it.
    pub saved: Option<OwnedObjectPath>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WifiNetworks {
    pub device: OwnedObjectPath,
    pub networks: Vec<AccessPoint>,
}

async fn wifi_device(conn: &zbus::Connection) -> zbus::Result<Option<OwnedObjectPath>> {
    for device in NetworkManagerMethodsProxy::new(conn).await?.get_devices().await? {
        let proxy = NmDeviceProxy::builder(conn).path(device.clone())?.build().await?;
        // NM_DEVICE_TYPE_WIFI
        if proxy.device_type().await? == 2 {
            return Ok(Some(device));
        }
    }
    Ok(None)
}

/// Saved Wi-Fi connections, by SSID.
async fn saved_wifi(conn: &zbus::Connection) -> zbus::Result<HashMap<String, OwnedObjectPath>> {
    let mut saved = HashMap::new();
    for path in NmSettingsProxy::new(conn).await?.list_connections().await? {
        let connection = NmConnectionProxy::builder(conn).path(path.clone())?.build().await?;
        let Ok(settings) = connection.get_settings().await else {
            continue;
        };
        let ssid = settings
            .get("802-11-wireless")
            .and_then(|w| w.get("ssid"))
            .and_then(|v| Vec::<u8>::try_from(v.clone()).ok());
        if let Some(ssid) = ssid {
            saved.insert(String::from_utf8_lossy(&ssid).into_owned(), path);
        }
    }
    Ok(saved)
}

/// The networks in reach, the one in use first, then saved ones, then by signal.
pub async fn wifi_networks(scan: bool) -> zbus::Result<Option<WifiNetworks>> {
    let conn = zbus::Connection::system().await?;
    let Some(device) = wifi_device(&conn).await? else {
        return Ok(None);
    };
    let wireless = NmWirelessProxy::builder(&conn)
        .path(device.clone())?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await?;
    if scan {
        // Fails when a scan ran a moment ago; the list is fresh then anyway.
        _ = wireless.request_scan(HashMap::new()).await;
    }
    let active = wireless.active_access_point().await?;
    let saved = saved_wifi(&conn).await.unwrap_or_default();
    let mut networks: Vec<AccessPoint> = Vec::new();
    for path in wireless.access_points().await? {
        let ap = NmAccessPointProxy::builder(&conn).path(path.clone())?.build().await?;
        let ssid = String::from_utf8_lossy(&ap.ssid().await.unwrap_or_default()).into_owned();
        if ssid.is_empty() {
            continue;
        }
        let strength = ap.strength().await.unwrap_or(0);
        let secure = ap.flags().await.unwrap_or(0) & 1 != 0
            || ap.wpa_flags().await.unwrap_or(0) != 0
            || ap.rsn_flags().await.unwrap_or(0) != 0;
        let is_active = path == active;
        // One entry per network, from its strongest access point.
        if let Some(existing) = networks.iter_mut().find(|n| n.ssid == ssid) {
            existing.active |= is_active;
            if strength > existing.strength {
                existing.strength = strength;
                existing.path = path;
            }
            continue;
        }
        networks.push(AccessPoint {
            saved: saved.get(&ssid).cloned(),
            path,
            ssid,
            strength,
            secure,
            active: is_active,
        });
    }
    networks.sort_by_key(|n| (!n.active, n.saved.is_none(), std::cmp::Reverse(n.strength)));
    Ok(Some(WifiNetworks { device, networks }))
}

pub enum Joined {
    Connecting,
    /// Needs a password, which only COSMIC Settings asks for.
    NeedsSettings,
}

pub async fn join_wifi(device: OwnedObjectPath, network: AccessPoint) -> zbus::Result<Joined> {
    let conn = zbus::Connection::system().await?;
    let nm = NetworkManagerMethodsProxy::new(&conn).await?;
    if let Some(saved) = &network.saved {
        nm.activate_connection(saved, &device, &network.path).await?;
        return Ok(Joined::Connecting);
    }
    if network.secure {
        return Ok(Joined::NeedsSettings);
    }
    nm.add_and_activate_connection(HashMap::new(), &device, &network.path)
        .await?;
    Ok(Joined::Connecting)
}

/// Turns a VPN on, from its saved connection, or off, from its active one.
pub async fn connect_vpn(vpn: Vpn) -> zbus::Result<()> {
    let conn = zbus::Connection::system().await?;
    let nm = NetworkManagerMethodsProxy::new(&conn).await?;
    let none = ObjectPath::from_static_str_unchecked("/");
    match &vpn.active {
        Some(active) => nm.deactivate_connection(active).await,
        None => nm.activate_connection(&vpn.path, &none, &none).await.map(|_| ()),
    }
}

pub async fn set_wifi(enabled: bool) -> zbus::Result<()> {
    let conn = zbus::Connection::system().await?;
    NetworkManagerProxy::new(&conn)
        .await?
        .set_wireless_enabled(enabled)
        .await
}

type ManagedObjects =
    HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

async fn bluetooth(conn: &zbus::Connection) -> zbus::Result<Bluetooth> {
    let manager = fdo::ObjectManagerProxy::builder(conn)
        .destination("org.bluez")?
        .path("/")?
        .build()
        .await?;
    let objects: ManagedObjects = manager
        .get_managed_objects()
        .await?
        .into_iter()
        .map(|(path, interfaces)| {
            let interfaces = interfaces
                .into_iter()
                .map(|(name, props)| (name.to_string(), props))
                .collect();
            (path, interfaces)
        })
        .collect();
    let flag = |props: &HashMap<String, OwnedValue>, key: &str| {
        props
            .get(key)
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false)
    };
    let mut status = Bluetooth::default();
    for (path, interfaces) in &objects {
        if let Some(adapter) = interfaces.get("org.bluez.Adapter1") {
            if status.adapter.is_none() {
                status.adapter = Some(path.clone());
                status.powered = flag(adapter, "Powered");
            }
        }
        if let Some(device) = interfaces.get("org.bluez.Device1") {
            let text = |key: &str| {
                device
                    .get(key)
                    .and_then(|v| String::try_from(v.clone()).ok())
            };
            let name = text("Alias").or_else(|| text("Name")).unwrap_or_default();
            let connected = flag(device, "Connected");
            if connected {
                status.connected.push(name.clone());
            }
            if flag(device, "Paired") {
                let battery = interfaces
                    .get("org.bluez.Battery1")
                    .and_then(|b| b.get("Percentage"))
                    .and_then(|v| u8::try_from(v).ok());
                status.devices.push(BluetoothDevice {
                    path: path.clone(),
                    name,
                    icon: text("Icon").unwrap_or_else(|| "bluetooth".to_owned()),
                    connected,
                    battery,
                });
            }
        }
    }
    status.devices.sort_by(|a, b| (!a.connected, &a.name).cmp(&(!b.connected, &b.name)));
    Ok(status)
}

pub async fn set_bluetooth(adapter: OwnedObjectPath, powered: bool) -> zbus::Result<()> {
    let conn = zbus::Connection::system().await?;
    let properties = fdo::PropertiesProxy::builder(&conn)
        .destination("org.bluez")?
        .path(adapter)?
        .build()
        .await?;
    properties
        .set(
            "org.bluez.Adapter1".try_into()?,
            "Powered",
            zbus::zvariant::Value::Bool(powered),
        )
        .await?;
    Ok(())
}

#[zbus::proxy(interface = "org.bluez.Device1", default_service = "org.bluez")]
trait BluezDevice {
    fn connect(&self) -> zbus::Result<()>;
    fn disconnect(&self) -> zbus::Result<()>;
}

pub async fn connect_bluetooth(device: OwnedObjectPath, connect: bool) -> zbus::Result<()> {
    let conn = zbus::Connection::system().await?;
    let device = BluezDeviceProxy::builder(&conn).path(device)?.build().await?;
    match connect {
        true => device.connect().await,
        false => device.disconnect().await,
    }
}

#[zbus::proxy(
    interface = "org.freedesktop.UPower.Device",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower/devices/DisplayDevice"
)]
trait Device {
    #[zbus(property)]
    fn is_present(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn icon_name(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn time_to_empty(&self) -> zbus::Result<i64>;
    #[zbus(property)]
    fn time_to_full(&self) -> zbus::Result<i64>;
}

#[zbus::proxy(
    interface = "org.freedesktop.UPower",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower"
)]
trait UPower {
    fn enumerate_devices(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

#[zbus::proxy(
    interface = "org.freedesktop.UPower.Device",
    default_service = "org.freedesktop.UPower"
)]
trait UPowerDevice {
    #[zbus(property, name = "Type")]
    fn kind(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn power_supply(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn is_present(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn model(&self) -> zbus::Result<String>;
}

/// The icon of a kind of UPower device.
fn peripheral_icon(kind: u32) -> &'static str {
    match kind {
        5 => "input-mouse-symbolic",
        6 => "input-keyboard-symbolic",
        8 => "phone-symbolic",
        12 | 22 => "input-tablet-symbolic",
        17 => "audio-headset-symbolic",
        18 => "audio-speakers-symbolic",
        19 => "audio-headphones-symbolic",
        21 => "input-gaming-symbolic",
        23 => "input-touchpad-symbolic",
        _ => "battery-symbolic",
    }
}

async fn peripherals(conn: &zbus::Connection) -> zbus::Result<Vec<Peripheral>> {
    let mut peripherals: Vec<Peripheral> = Vec::new();
    for path in UPowerProxy::new(conn).await?.enumerate_devices().await? {
        let device = UPowerDeviceProxy::builder(conn)
            .path(path)?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await?;
        // Not the computer's own battery or charger.
        if device.power_supply().await.unwrap_or(true) || !device.is_present().await.unwrap_or(false) {
            continue;
        }
        let name = device.model().await.unwrap_or_default();
        // The same device through two ways, say Bluetooth and Logitech's protocol.
        if peripherals.iter().any(|p| p.name == name) {
            continue;
        }
        peripherals.push(Peripheral {
            icon: peripheral_icon(device.kind().await.unwrap_or(0)),
            percentage: device.percentage().await.unwrap_or(0.0),
            name,
        });
    }
    peripherals.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(peripherals)
}

async fn battery(conn: &zbus::Connection) -> zbus::Result<Option<Battery>> {
    let device = DeviceProxy::builder(conn)
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await?;
    if !device.is_present().await? {
        return Ok(None);
    }
    let state = device.state().await?;
    // 1 charging, 2 discharging, 4 fully charged
    let time_left = match state {
        1 => device.time_to_full().await.ok(),
        2 => device.time_to_empty().await.ok(),
        _ => None,
    }
    .filter(|t| *t > 0);
    Ok(Some(Battery {
        percentage: device.percentage().await?,
        charging: matches!(state, 1 | 4),
        icon: device.icon_name().await?,
        time_left,
    }))
}

#[zbus::proxy(
    interface = "net.hadess.PowerProfiles",
    default_service = "net.hadess.PowerProfiles",
    default_path = "/net/hadess/PowerProfiles"
)]
trait PowerProfilesDaemon {
    #[zbus(property)]
    fn active_profile(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn set_active_profile(&self, profile: &str) -> zbus::Result<()>;
    #[zbus(property)]
    fn profiles(&self) -> zbus::Result<Vec<HashMap<String, OwnedValue>>>;
}

async fn power_profiles(conn: &zbus::Connection) -> zbus::Result<PowerProfiles> {
    let daemon = PowerProfilesDaemonProxy::builder(conn)
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await?;
    let mut available: Vec<String> = daemon
        .profiles()
        .await?
        .iter()
        .filter_map(|p| p.get("Profile").and_then(|v| String::try_from(v.clone()).ok()))
        .collect();
    let mut active = daemon.active_profile().await?;
    if available.iter().any(|p| p == "power-saver")
        && let Some(on) = super_saver().await
    {
        available.push(SUPER_SAVER.to_owned());
        if on {
            active = SUPER_SAVER.to_owned();
        }
    }
    Ok(PowerProfiles { active, available })
}

/// Whether super saver is on; `None` without its helper.
async fn super_saver() -> Option<bool> {
    let output = crate::host::command(SUPER_SAVER_HELPER, &[]).arg("status").output().await.ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim() == "on")
}

/// As root, through the polkit rule that lets this user run it.
async fn run_super_saver(on: bool) -> zbus::Result<()> {
    let status = crate::host::command("pkexec", &[])
        .args([SUPER_SAVER_HELPER, if on { "on" } else { "off" }])
        .status()
        .await
        .map_err(|err| zbus::Error::Failure(err.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(zbus::Error::Failure(format!("super-economia: {status}")))
    }
}

pub async fn set_power_profile(profile: String) -> zbus::Result<()> {
    if profile == SUPER_SAVER {
        // It sets power-saver itself.
        return run_super_saver(true).await;
    }
    if super_saver().await == Some(true) {
        run_super_saver(false).await?;
    }
    let conn = zbus::Connection::system().await?;
    PowerProfilesDaemonProxy::new(&conn)
        .await?
        .set_active_profile(&profile)
        .await
}

#[zbus::proxy(
    interface = "com.system76.CosmicSettingsDaemon",
    default_service = "com.system76.CosmicSettingsDaemon",
    default_path = "/com/system76/CosmicSettingsDaemon"
)]
trait SettingsDaemon {
    #[zbus(property)]
    fn display_brightness(&self) -> zbus::Result<i32>;
    #[zbus(property)]
    fn set_display_brightness(&self, value: i32) -> zbus::Result<()>;
    #[zbus(property)]
    fn max_display_brightness(&self) -> zbus::Result<i32>;
}

async fn brightness(conn: &zbus::Connection) -> zbus::Result<Option<(i32, i32)>> {
    let daemon = SettingsDaemonProxy::builder(conn)
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await?;
    let max = daemon.max_display_brightness().await?;
    if max <= 0 {
        return Ok(None);
    }
    Ok(Some((daemon.display_brightness().await?, max)))
}

pub async fn set_brightness(value: i32) {
    static LATEST: Latest<(), i32> = Latest::new();
    LATEST
        .send((), value, |(), value| async move {
            let set = async {
                let conn = zbus::Connection::session().await?;
                SettingsDaemonProxy::new(&conn)
                    .await?
                    .set_display_brightness(value)
                    .await
            };
            if let Err(err) = set.await {
                tracing::warn!("cannot set brightness: {err}");
            }
        })
        .await;
}

const SINK: &str = "@DEFAULT_AUDIO_SINK@";
const SOURCE: &str = "@DEFAULT_AUDIO_SOURCE@";

/// `Volume: 0.45 [MUTED]`
fn parse_volume(output: &str) -> Option<Volume> {
    let rest = output.trim().strip_prefix("Volume:")?.trim();
    let level = rest.split_whitespace().next()?.parse().ok()?;
    Some(Volume {
        level,
        muted: rest.contains("[MUTED]"),
    })
}

async fn volume() -> Option<Volume> {
    get_volume(SINK).await
}

async fn microphone() -> Option<bool> {
    get_volume(SOURCE).await.map(|v| v.muted)
}

async fn get_volume(node: &str) -> Option<Volume> {
    let output = crate::host::command("wpctl", &[])
        .args(["get-volume", node])
        .output()
        .await
        .ok()?;
    parse_volume(&String::from_utf8_lossy(&output.stdout))
}

pub async fn set_volume(level: f32) {
    static LATEST: Latest<(), f32> = Latest::new();
    LATEST
        .send((), level, |(), level| async move {
            let level = format!("{:.2}", level.clamp(0.0, 1.0));
            _ = crate::host::command("wpctl", &[])
                .args(["set-volume", SINK, &level])
                .status()
                .await;
        })
        .await;
}

pub async fn toggle_mute() {
    _ = crate::host::command("wpctl", &[])
        .args(["set-mute", SINK, "toggle"])
        .status()
        .await;
}

pub async fn toggle_microphone() {
    _ = crate::host::command("wpctl", &[])
        .args(["set-mute", SOURCE, "toggle"])
        .status()
        .await;
}

pub(crate) fn pactl() -> Command {
    // Its JSON breaks on translated messages.
    crate::host::command("pactl", &[("LC_ALL", "C")])
}

/// `pactl subscribe`, which ends with the applet: left behind, each would
/// hold one of the few connections PipeWire allows.
/// PipeWire's change events, from one `pactl subscribe` the applet shares
/// between those listening, started with the first of them and again if it
/// goes away (say PipeWire restarted).
pub(crate) fn pactl_events() -> tokio::sync::broadcast::Receiver<String> {
    use std::sync::OnceLock;
    static EVENTS: OnceLock<tokio::sync::broadcast::Sender<String>> = OnceLock::new();
    let sender = EVENTS.get_or_init(|| {
        let (tx, _) = tokio::sync::broadcast::channel(64);
        let sender = tx.clone();
        tokio::spawn(async move {
            loop {
                if let Ok(mut child) = pactl_subscribe()
                    .stdout(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()
                    && let Some(stdout) = child.stdout.take()
                {
                    let mut lines = tokio::io::BufReader::new(stdout).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        // Each pactl and wpctl run, ours too, comes and goes
                        // as a client: passed on, they filled the channel,
                        // whose lagging reads again with more of them, and
                        // three applets kept each other at it, hundreds a
                        // second.
                        if !line.contains(" on client") {
                            _ = sender.send(line);
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
        tx
    });
    sender.subscribe()
}

pub(crate) fn pactl_subscribe() -> Command {
    let mut pactl = pactl();
    pactl.arg("subscribe");
    // SAFETY: prctl is async-signal-safe.
    unsafe {
        pactl.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    pactl
}

/// Audio outputs, and the name of the one in use.
pub async fn sinks() -> Option<(Vec<Sink>, String)> {
    let list = pactl()
        .args(["-f", "json", "list", "sinks"])
        .output()
        .await
        .ok()?;
    let list: Vec<serde_json::Value> = serde_json::from_slice(&list.stdout).ok()?;
    let default = pactl().arg("get-default-sink").output().await.ok()?;
    let default = String::from_utf8_lossy(&default.stdout).trim().to_owned();
    let sinks = list
        .iter()
        .filter_map(|sink| {
            let name = sink["name"].as_str()?;
            let active = sink["active_port"].as_str();
            let port = sink["ports"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|port| port["name"].as_str() == active);
            // Nothing plugged into it, like an HDMI port without a screen.
            let unplugged = port.is_some_and(|p| p["availability"] == "not available");
            if unplugged && name != default {
                return None;
            }
            // The port says it shortest, "Speaker" or "HDMI / DisplayPort 1
            // Output", for a sound card's outputs; a device by its name.
            let description = port
                .filter(|_| !name.starts_with("bluez"))
                .and_then(|p| p["description"].as_str())
                .or(sink["description"].as_str())?;
            Some(Sink {
                name: name.to_owned(),
                description: description.to_owned(),
            })
        })
        .collect();
    Some((sinks, default))
}

pub async fn set_sink(name: String) {
    _ = pactl().args(["set-default-sink", &name]).status().await;
}

/// PipeWire tells about changes through its PulseAudio side.
fn volume_subscription() -> Subscription<Update> {
    Subscription::run_with("volume", |_| {
        stream::channel(4, |mut output: mpsc::Sender<Update>| async move {
            let mut events = pactl_events();
            loop {
                _ = output.send(Update::Volume(volume().await)).await;
                _ = output.send(Update::Microphone(microphone().await)).await;
                loop {
                    let line = match events.recv().await {
                        Ok(line) => line,
                        // Fell behind: what's now, read again.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break,
                        Err(_) => return futures::future::pending().await,
                    };
                    if line.contains("sink") || line.contains("server") {
                        if output.send(Update::Volume(volume().await)).await.is_err() {
                            return;
                        }
                    }
                    if line.contains("sink-input") || line.contains("on source") {
                        if output.send(Update::Audio).await.is_err() {
                            return;
                        }
                    }
                    if line.contains("source") || line.contains("server") {
                        if output.send(Update::Microphone(microphone().await)).await.is_err() {
                            return;
                        }
                    }
                }
            }
        })
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Display {
    /// Connector, like `eDP-1` or `HDMI-A-1`.
    pub name: String,
    pub make: String,
    pub model: String,
    pub enabled: bool,
    pub width: u32,
    pub height: u32,
    /// Millihertz.
    pub refresh: u32,
    pub scale: f32,
    /// Its own brightness control over DDC/CI: display number, current, maximum.
    pub ddc: Option<(u32, u32, u32)>,
    /// Turned a quarter, standing: as tall as it's wide.
    pub turned: bool,
}

impl Display {
    /// The laptop's own screen.
    pub fn internal(&self) -> bool {
        self.name.starts_with("eDP") || self.name.starts_with("LVDS") || self.name.starts_with("DSI")
    }
}

/// `cosmic-randr list --kdl`, which reads:
///
/// ```text
/// output "eDP-1" enabled=#true {
///   description make="AU Optronics" model="0xE3AC"
///   scale 1.25
///   modes {
///     mode 1920 1200 60001 current=#true preferred=#true
/// ```
fn parse_displays(kdl: &str) -> Vec<Display> {
    let quoted = |line: &str, key: &str| -> Option<String> {
        let rest = line.split(&format!("{key}=\"")).nth(1)?;
        Some(rest.split('"').next()?.to_owned())
    };
    let mut displays: Vec<Display> = Vec::new();
    for line in kdl.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("output \"") {
            displays.push(Display {
                name: rest.split('"').next().unwrap_or_default().to_owned(),
                enabled: line.contains("enabled=#true"),
                scale: 1.0,
                ..Display::default()
            });
            continue;
        }
        let Some(display) = displays.last_mut() else {
            continue;
        };
        if line.starts_with("description") {
            display.make = quoted(line, "make").unwrap_or_default();
            display.model = quoted(line, "model").unwrap_or_default();
        } else if let Some(transform) = line.strip_prefix("transform ") {
            display.turned = ["90", "270"].iter().any(|t| transform.contains(t));
        } else if let Some(scale) = line.strip_prefix("scale ") {
            display.scale = scale.trim().parse().unwrap_or(1.0);
        } else if line.starts_with("mode ") && line.contains("current=#true") {
            let numbers: Vec<u32> = line
                .split_whitespace()
                .skip(1)
                .take(3)
                .filter_map(|n| n.parse().ok())
                .collect();
            if let [width, height, refresh] = numbers[..] {
                (display.width, display.height, display.refresh) = (width, height, refresh);
            }
        }
    }
    displays
}

/// `ddcutil` with these arguments, one at a time across the applet's
/// instances (one per panel): two at once on the same bus fail.
fn ddcutil<I, S>(args: I) -> tokio::process::Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let lock = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()) + "/central-ddc.lock";
    let mut command = crate::host::command("flock", &[]);
    command.arg(lock).arg("ddcutil").args(args);
    // Only when nothing else wants the CPU. (The I²C's hold on the GPU is
    // the kernel's: this doesn't shorten it, only keeps the rest ahead.)
    // SAFETY: only sched_setscheduler, async-signal-safe, in the child.
    unsafe {
        command.pre_exec(|| {
            let param = libc::sched_param { sched_priority: 0 };
            libc::sched_setscheduler(0, libc::SCHED_IDLE, &param);
            Ok(())
        });
    }
    command
}

/// Where what is known of the monitors' DDC is kept across sessions: which
/// have it (finding out holds the GPU's I²C), their levels last read or set.
fn ddc_state_file(name: &str) -> Option<std::path::PathBuf> {
    user_dir("STATE", ".local/state").map(|dir| dir.join("cosmic-central").join(name))
}

/// This session's file of an older version, read if there's nothing kept.
fn ddc_runtime_file(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into())).join(name)
}

fn write_ddc_state(name: &str, text: &str) {
    let Some(path) = ddc_state_file(name) else {
        return;
    };
    if let Some(dir) = path.parent()
        && let Err(err) = std::fs::create_dir_all(dir)
    {
        tracing::warn!("{} not made: {err}", dir.display());
        return;
    }
    if let Err(err) = std::fs::write(&path, text) {
        tracing::warn!("{} not written: {err}", path.display());
    }
}

/// `connector number` a line.
fn parse_ddc_displays(text: &str) -> HashMap<String, u32> {
    text.lines()
        .filter_map(|line| {
            let (name, number) = line.trim().split_once(' ')?;
            Some((name.to_owned(), number.trim().parse().ok()?))
        })
        .collect()
}

/// The monitors known to have DDC, as kept (none asked).
fn known_ddc_displays() -> HashMap<String, u32> {
    let kept = ddc_state_file("ddc-displays").and_then(|path| std::fs::read_to_string(path).ok());
    let text = kept
        .filter(|text| !text.trim().is_empty())
        .or_else(|| std::fs::read_to_string(ddc_runtime_file("central-ddc-displays")).ok())
        .unwrap_or_default();
    parse_ddc_displays(&text)
}

/// Finds which monitors have DDC if none is known yet: once, in the
/// background, never on opening a panel (it holds the GPU's I²C, and with
/// it the compositor's rendering, for a moment). Whether any is known now.
pub async fn detect_ddc_if_unknown() -> bool {
    if !known_ddc_displays().is_empty() {
        return true;
    }
    !ddc_displays().await.is_empty()
}

/// A monitor's brightness, `VCP 10`: current and maximum. DDC sometimes
/// doesn't answer the first time.
async fn ddc_brightness(number: u32) -> Option<(u32, u32)> {
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let Ok(vcp) = ddcutil(["--display", &number.to_string(), "getvcp", "10", "--brief"])
            .output()
            .await
        else {
            return None;
        };
        if let Some(brightness) = parse_vcp(&String::from_utf8_lossy(&vcp.stdout)) {
            return Some(brightness);
        }
    }
    None
}

/// External monitors `ddcutil` can talk to, by connector: their display
/// numbers. Detecting them keeps the GPU's I²C busy for half a second, which
/// on NVIDIA stutters the whole desktop, the pointer too: so it's done once,
/// and kept for this session in a file the other screens' applets read.
async fn ddc_displays() -> HashMap<String, u32> {
    static KNOWN: std::sync::Mutex<Option<HashMap<String, u32>>> = std::sync::Mutex::new(None);
    if let Some(known) = KNOWN.lock().unwrap().clone() {
        return known;
    }
    let known = known_ddc_displays();
    if !known.is_empty() {
        *KNOWN.lock().unwrap() = Some(known.clone());
        return known;
    }
    let found = detect_ddc().await;
    if !found.is_empty() {
        let text: String = found.iter().map(|(name, n)| format!("{name} {n}\n")).collect();
        write_ddc_state("ddc-displays", &text);
        *KNOWN.lock().unwrap() = Some(found.clone());
    }
    found
}

async fn detect_ddc() -> HashMap<String, u32> {
    let Ok(output) = ddcutil(["detect", "--brief"])
        .output()
        .await
    else {
        return HashMap::new();
    };
    let mut displays = HashMap::new();
    let mut number = None;
    for line in String::from_utf8_lossy(&output.stdout).lines().map(str::trim) {
        if let Some(n) = line.strip_prefix("Display ") {
            number = n.trim().parse().ok();
        } else if let Some(connector) = line.strip_prefix("DRM connector:") {
            // `card1-HDMI-A-1`
            let connector = connector.trim();
            let connector = connector.split_once('-').map_or(connector, |(_, c)| c);
            if let Some(n) = number {
                displays.insert(connector.to_owned(), n);
            }
        }
    }
    displays
}

/// `VCP 10 C 50 100`: current and maximum.
fn parse_vcp(output: &str) -> Option<(u32, u32)> {
    let fields: Vec<&str> = output.split_whitespace().collect();
    match fields[..] {
        ["VCP", _, "C", current, max, ..] => Some((current.parse().ok()?, max.parse().ok()?)),
        _ => None,
    }
}

/// Whose brightness to read over DDC, each read being slow and heavy: a
/// read holds the GPU's I²C, and with it the compositor's render threads
/// (measured: up to ~235 ms stuck, the cursor with them, per read).
#[derive(Clone, Debug)]
pub enum DdcRead {
    /// Every monitor's, for the page of displays, asked for.
    All,
    /// None read: the levels last read or set, for opening the Central.
    Remembered,
}


/// `number current max` a line.
fn parse_ddc_levels(text: &str) -> HashMap<u32, (u32, u32)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace().map(str::parse::<u32>);
            let (Some(Ok(number)), Some(Ok(current)), Some(Ok(max))) = (fields.next(), fields.next(), fields.next()) else {
                return None;
            };
            (current <= max && max > 0).then_some((number, (current, max)))
        })
        .collect()
}

fn remembered_ddc_levels() -> HashMap<u32, (u32, u32)> {
    ddc_state_file("ddc-levels")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| parse_ddc_levels(&text))
        .unwrap_or_default()
}

fn remember_ddc_levels(levels: &[(u32, u32, u32)]) {
    if levels.is_empty() {
        return;
    }
    let mut known = remembered_ddc_levels();
    for &(number, current, max) in levels {
        known.insert(number, (current, max));
    }
    let text: String = known.iter().map(|(n, (c, m))| format!("{n} {c} {m}\n")).collect();
    write_ddc_state("ddc-levels", &text);
}

pub async fn displays(read: DdcRead) -> Vec<Display> {
    let Ok(output) = crate::host::command("cosmic-randr", &[])
        .args(["list", "--kdl"])
        .output()
        .await
    else {
        return Vec::new();
    };
    let mut displays = parse_displays(&String::from_utf8_lossy(&output.stdout));
    if !displays.iter().any(|d| d.enabled && !d.internal()) {
        return displays;
    }
    if matches!(read, DdcRead::Remembered) {
        let (levels, known) = (remembered_ddc_levels(), known_ddc_displays());
        for display in &mut displays {
            if let Some(&number) = known.get(&display.name) {
                // Unread: as last read or set, or kept from before.
                let (current, max) = levels.get(&number).copied().unwrap_or((u32::MAX, 100));
                display.ddc = Some((number, current, max));
            }
        }
        return displays;
    }
    let ddc = ddc_displays().await;
    let mut read_now = Vec::new();
    for display in &mut displays {
        let Some(&number) = ddc.get(&display.name) else {
            continue;
        };
        if let Some((current, max)) = ddc_brightness(number).await {
            display.ddc = Some((number, current, max));
            read_now.push((number, current, max));
        }
    }
    remember_ddc_levels(&read_now);
    displays
}

/// How tall a screen is, in logical pixels, turned or not: what COSMIC's
/// own tool says, as the panel's outputs don't tell a screen standing.
pub async fn screen_height(name: String) -> Option<i32> {
    let output = crate::host::command("cosmic-randr", &[])
        .args(["list", "--kdl"])
        .output()
        .await
        .ok()?;
    let displays = parse_displays(&String::from_utf8_lossy(&output.stdout));
    let display = displays.into_iter().find(|d| d.name == name)?;
    let tall = if display.turned { display.width } else { display.height };
    Some((tall as f32 / display.scale.max(0.1)) as i32)
}

pub async fn set_scale(display: Display, scale: f32) {
    _ = crate::host::command("cosmic-randr", &[])
        .args([
            "mode".to_owned(),
            display.name,
            display.width.to_string(),
            display.height.to_string(),
            "--refresh".to_owned(),
            format!("{:.3}", display.refresh as f32 / 1000.0),
            "--scale".to_owned(),
            scale.to_string(),
        ])
        .status()
        .await;
}

pub async fn set_ddc_brightness(number: u32, value: u32, max: u32) {
    let set = ddcutil(["--display", &number.to_string(), "setvcp", "10", &value.to_string()])
        .status()
        .await;
    if set.is_ok_and(|status| status.success()) {
        remember_ddc_levels(&[(number, value, max)]);
    }
}

/// cosmic-comp's settings, kept as cosmic-comp-config does without needing
/// it, and libinput with it.
const COMP_CONFIG_VERSION: u64 = 1;

/// Whether tiling follows each workspace, as cosmic-comp-config says it.
#[derive(serde::Serialize)]
enum TileBehavior {
    PerWorkspace,
}

/// cosmic-comp's tiling settings.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TilingConfig {
    /// New workspaces tile their windows.
    pub new_workspaces_tile: bool,
    /// The active window is outlined.
    pub active_hint: bool,
}

fn comp_config() -> Result<Config, cosmic::cosmic_config::Error> {
    Config::new(
        "com.system76.CosmicComp",
        COMP_CONFIG_VERSION,
    )
}

pub fn tiling_config() -> TilingConfig {
    let Ok(config) = comp_config() else {
        return TilingConfig::default();
    };
    TilingConfig {
        new_workspaces_tile: config.get("autotile").unwrap_or(false),
        active_hint: config.get("active_hint").unwrap_or(true),
    }
}

pub fn set_tiling_config(key: &str, value: bool) {
    let result = comp_config().and_then(|config| {
        // Tiling one workspace takes cosmic-comp tiling per workspace, as the
        // tiling applet does.
        config.set(
            "autotile_behavior",
            TileBehavior::PerWorkspace,
        )?;
        config.set(key, value)
    });
    if let Err(err) = result {
        tracing::warn!("cannot set {key}: {err}");
    }
}

/// Switch high contrast on or off in both themes, as COSMIC's accessibility
/// applet does.
pub fn set_high_contrast(enabled: bool) {
    use cosmic::{
        cosmic_config::CosmicConfigEntry,
        cosmic_theme::{CosmicPalette, Theme as CosmicTheme, ThemeBuilder},
    };
    std::thread::spawn(move || {
        for dark in [true, false] {
            let result = (|| {
                let builder_config = if dark {
                    ThemeBuilder::dark_config()?
                } else {
                    ThemeBuilder::light_config()?
                };
                let mut builder = ThemeBuilder::get_entry(&builder_config).unwrap_or_else(|(_, b)| b);
                let palette = builder.palette.inner();
                builder.palette = match (dark, enabled) {
                    (true, true) => CosmicPalette::HighContrastDark(palette),
                    (true, false) => CosmicPalette::Dark(palette),
                    (false, true) => CosmicPalette::HighContrastLight(palette),
                    (false, false) => CosmicPalette::Light(palette),
                };
                builder.write_entry(&builder_config)?;
                let theme_config = if dark {
                    CosmicTheme::dark_config()?
                } else {
                    CosmicTheme::light_config()?
                };
                builder.build().write_entry(&theme_config)?;
                Ok::<(), cosmic::cosmic_config::Error>(())
            })();
            if let Err(err) = result {
                tracing::warn!("cannot set high contrast: {err:?}");
            }
        }
    });
}

pub use cosmic_settings_a11y_manager_subscription::{
    AccessibilityEvent, AccessibilityRequest, ColorFilter,
};

#[derive(Clone, Debug)]
pub enum A11y {
    Ready(cosmic::cctk::sctk::reexports::calloop::channel::Sender<AccessibilityRequest>),
    Event(AccessibilityEvent),
}

/// The magnifier and screen filters, through COSMIC's accessibility protocol.
pub fn a11y_subscription() -> Subscription<A11y> {
    Subscription::run_with("a11y", |_| {
        stream::channel(8, |mut output: mpsc::Sender<A11y>| async move {
            match cosmic_settings_a11y_manager_subscription::spawn_wayland_connection(1) {
                Ok((tx, mut rx)) => {
                    _ = output.send(A11y::Ready(tx)).await;
                    while let Some(event) = rx.recv().await {
                        if output.send(A11y::Event(event)).await.is_err() {
                            break;
                        }
                    }
                }
                Err(err) => tracing::warn!("no accessibility protocol: {err}"),
            }
            futures::future::pending().await
        })
    })
}

/// The session bus connection inhibitions are tied to: they end with it.
async fn session() -> zbus::Result<zbus::Connection> {
    static BUS: OnceCell<zbus::Connection> = OnceCell::const_new();
    BUS.get_or_try_init(zbus::Connection::session).await.cloned()
}

#[zbus::proxy(
    interface = "org.freedesktop.ScreenSaver",
    default_service = "org.freedesktop.ScreenSaver",
    default_path = "/org/freedesktop/ScreenSaver"
)]
trait ScreenSaver {
    fn inhibit(&self, application: &str, reason: &str) -> zbus::Result<u32>;
    fn un_inhibit(&self, cookie: u32) -> zbus::Result<()>;
}

/// Keep the screen on and the computer awake, until `allow_idle`.
pub async fn inhibit_idle(reason: &str) -> zbus::Result<u32> {
    ScreenSaverProxy::new(&session().await?)
        .await?
        .inhibit(crate::APP_ID, reason)
        .await
}

pub async fn allow_idle(cookie: u32) -> zbus::Result<()> {
    ScreenSaverProxy::new(&session().await?)
        .await?
        .un_inhibit(cookie)
        .await
}

/// Which tiles show, in order, by id.
pub fn tiles() -> Option<Vec<String>> {
    Config::new(crate::APP_ID, 1)
        .and_then(|c| c.get::<Vec<String>>("tiles"))
        .ok()
}

pub fn set_tiles(tiles: &[String]) {
    if let Err(err) = Config::new(crate::APP_ID, 1).and_then(|c| c.set("tiles", tiles)) {
        tracing::warn!("cannot save tiles: {err}");
    }
}

/// Whether the bar shows the computer's battery percentage.
pub const SHOW_PERCENTAGE: Setting = Setting {
    config: crate::APP_ID,
    version: 1,
    key: "show_battery_percentage",
};

/// Fires when COSMIC settings the tiles show change: do not disturb, the
/// theme mode, tiling.
pub fn settings_subscription() -> Subscription<()> {
    Subscription::run_with("settings", |_| {
        stream::channel(4, |output: mpsc::Sender<()>| async move {
            let watchers: Vec<_> = [
                (DO_NOT_DISTURB.config, DO_NOT_DISTURB.version),
                (DARK_MODE.config, DARK_MODE.version),
                (HOT_EDGE, 1),
                // Its own: what one panel's applet saves, the others on the
                // other screens show at once.
                (crate::APP_ID, 1),
                (
                    "com.system76.CosmicComp",
                    COMP_CONFIG_VERSION,
                ),
            ]
            .into_iter()
            .filter_map(|(name, version)| {
                let output = std::sync::Mutex::new(output.clone());
                let config = Config::new(name, version).ok()?;
                config
                    .watch(move |_, _| {
                        if let Ok(mut output) = output.lock() {
                            _ = output.try_send(());
                        }
                    })
                    .inspect_err(|err| tracing::warn!("cannot watch {name}: {err}"))
                    .ok()
            })
            .collect();
            futures::future::pending::<()>().await;
            drop(watchers);
        })
    })
}

/// The touchpad, as COSMIC Settings sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Touchpad {
    Enabled,
    Disabled,
    /// Off while a mouse is plugged in.
    DisabledOnExternalMouse,
}

/// cosmic-comp's touchpad settings: the user's, or the system's defaults.
/// Read and written as text, changing only their `state`, so that the rest
/// (tapping, scrolling…) stays as it is.
fn touchpad_files() -> (Option<std::path::PathBuf>, Vec<std::path::PathBuf>) {
    use std::path::PathBuf;
    const KEY: &str = "com.system76.CosmicComp/v1/input_touchpad";
    let config = std::env::var_os("HOST_XDG_CONFIG_HOME")
        .filter(|_| crate::host::in_flatpak())
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").filter(|_| !crate::host::in_flatpak()))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    let user = config.map(|dir| dir.join("cosmic").join(KEY));
    let system = ["/usr/share/cosmic", "/run/host/usr/share/cosmic"]
        .iter()
        .map(|dir| PathBuf::from(dir).join(KEY))
        .collect();
    (user, system)
}

fn touchpad_text() -> Option<(String, std::path::PathBuf)> {
    let (user, system) = touchpad_files();
    let user = user?;
    let text = std::fs::read_to_string(&user)
        .ok()
        .or_else(|| system.iter().find_map(|path| std::fs::read_to_string(path).ok()))?;
    Some((text, user))
}

/// Where `state:`'s value is in the settings' text.
fn state_value(text: &str) -> Option<std::ops::Range<usize>> {
    let key = text.find("state")?;
    let colon = key + text[key..].find(':')?;
    let start = colon + 1 + text[colon + 1..].len() - text[colon + 1..].trim_start().len();
    let end = start
        + text[start..]
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(text.len() - start);
    Some(start..end)
}

pub fn touchpad() -> Option<Touchpad> {
    let (text, _) = touchpad_text()?;
    Some(match &text[state_value(&text)?] {
        "Disabled" => Touchpad::Disabled,
        "DisabledOnExternalMouse" => Touchpad::DisabledOnExternalMouse,
        _ => Touchpad::Enabled,
    })
}

pub fn set_touchpad(enabled: bool) {
    let result = (|| {
        let (mut text, path) = touchpad_text().ok_or("no touchpad settings")?;
        let range = state_value(&text).ok_or("no state in the touchpad settings")?;
        text.replace_range(range, if enabled { "Enabled" } else { "Disabled" });
        std::fs::create_dir_all(path.parent().ok_or("no directory")?).map_err(|e| e.to_string())?;
        // Whole, for cosmic-comp, which watches the file.
        let new = path.with_extension("new");
        std::fs::write(&new, text).map_err(|e| e.to_string())?;
        std::fs::rename(&new, &path).map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    })();
    if let Err(err) = result {
        tracing::warn!("cannot set the touchpad: {err}");
    }
}

/// When do not disturb ends by itself, in seconds since the epoch; 0 never.
pub fn dnd_until() -> Option<i64> {
    Config::new(crate::APP_ID, 1)
        .and_then(|c| c.get::<i64>("do_not_disturb_until"))
        .ok()
        .filter(|t| *t > 0)
}

pub fn set_dnd_until(until: Option<i64>) {
    let result = Config::new(crate::APP_ID, 1)
        .and_then(|c| c.set("do_not_disturb_until", until.unwrap_or(0)));
    if let Err(err) = result {
        tracing::warn!("cannot save when do not disturb ends: {err}");
    }
}

/// The Hot Edge extension (cosmic-ext-hot-edge): its daemon, which makes the
/// edge, and its settings.
const HOT_EDGE: &str = "io.github.DrakRosmann.CosmicHotEdge";
const HOT_EDGE_DAEMON: &str = "cosmic-ext-hot-edge-daemon";

/// Whether the extension is installed: its daemon on the PATH, or where
/// its installers put it.
pub fn hot_edge_installed() -> bool {
    static INSTALLED: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        let home = std::env::var("HOME").unwrap_or_default();
        std::env::var("PATH")
            .unwrap_or_default()
            .split(':')
            .map(str::to_owned)
            .chain([format!("{home}/.local/bin"), "/usr/bin".to_owned(), "/run/host/usr/bin".to_owned()])
            .any(|dir| std::path::Path::new(&dir).join(HOT_EDGE_DAEMON).exists())
    });
    *INSTALLED
}

/// Whether the edge is on: the extension's default, until set.
pub fn hot_edge() -> bool {
    Config::new(HOT_EDGE, 1)
        .and_then(|c| c.get::<bool>("enabled"))
        .unwrap_or(true)
}

/// The daemon stays running while off and follows the setting; on, it is
/// started too, in case it isn't (it runs once per session).
pub fn set_hot_edge(enabled: bool) {
    if let Err(err) = Config::new(HOT_EDGE, 1).and_then(|c| c.set("enabled", enabled)) {
        tracing::warn!("cannot set the hot edge: {err}");
    }
    if enabled {
        crate::host::spawn(HOT_EDGE_DAEMON, &[]);
    }
}

/// The phone as a webcam, through Iriun: a user unit of its own
/// (iriun-adb) opens Iriun on the computer and on the phone, keeps them up,
/// and tells how they are in a file.
const CAMERA_UNIT: &str = "iriun-camera.service";

/// A directory of the user's on the host, as `$XDG_<name>_HOME` or under
/// the home: in a sandbox, the host's, which Flatpak hands as
/// `$HOST_XDG_<name>_HOME`.
fn user_dir(name: &str, under_home: &str) -> Option<std::path::PathBuf> {
    let host = if crate::host::in_flatpak() { "HOST_" } else { "" };
    std::env::var_os(format!("{host}XDG_{name}_HOME"))
        .filter(|dir| !dir.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(under_home)))
}

/// Whether the unit is installed, where systemd looks for the user's own.
/// Looked at again now and then, not each time the popup draws, so that it
/// shows soon after it's installed. The applet runs on the host, where the
/// file is; in a sandbox the path is the same, and with no access to it the
/// tile only doesn't show.
pub fn camera_installed() -> bool {
    use std::{sync::Mutex, time::{Duration, Instant}};
    static CHECKED: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let Ok(mut checked) = CHECKED.lock() else {
        return false;
    };
    match *checked {
        Some((at, installed)) if at.elapsed() < Duration::from_secs(10) => installed,
        _ => {
            let installed = user_dir("CONFIG", ".config")
                .is_some_and(|dir| dir.join("systemd/user").join(CAMERA_UNIT).exists());
            *checked = Some((Instant::now(), installed));
            installed
        }
    }
}

/// How the service is, as it last wrote it down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraState {
    Starting,
    WaitingPhone,
    Active,
    Error,
    Stopped,
}

/// Each part the service looks after, and what went wrong, ready to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CameraStatus {
    /// None when the file says none the applet knows.
    pub state: Option<CameraState>,
    pub pc_iriun: bool,
    pub phone_connected: bool,
    pub phone_iriun: bool,
    pub screen_off: bool,
    pub error: Option<String>,
    /// Iriun's window on the computer: none when it has none, or the file
    /// says something else.
    pub pc_window: Option<PcWindow>,
}

/// Iriun's window on the computer, which the service keeps out of sight
/// (withdrawn, off the taskbar and dock too) and brings back on a signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcWindow {
    Hidden,
    Shown,
}

/// The camera now: whether the unit runs, and, if so, its status.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Camera {
    pub on: bool,
    pub status: Option<CameraStatus>,
}

/// What `systemctl is-active` prints: on while starting too.
fn camera_active(output: &str) -> bool {
    matches!(output.trim(), "active" | "activating")
}

/// The service's `status.json`; what's missing is off. Anything but an
/// object of that shape is no status at all.
fn parse_camera_status(text: &[u8]) -> Option<CameraStatus> {
    #[derive(Default, serde::Deserialize)]
    #[serde(default)]
    struct File {
        state: Option<String>,
        pc_iriun: bool,
        phone_connected: bool,
        phone_iriun: bool,
        screen_off: bool,
        error: Option<String>,
        /// Any value: an odd one is no window, not a broken file.
        pc_window: serde_json::Value,
    }
    let file: File = serde_json::from_slice(text).ok()?;
    let state = match file.state.as_deref() {
        Some("starting") => Some(CameraState::Starting),
        Some("waiting_phone") => Some(CameraState::WaitingPhone),
        Some("active") => Some(CameraState::Active),
        Some("error") => Some(CameraState::Error),
        Some("stopped") => Some(CameraState::Stopped),
        _ => None,
    };
    Some(CameraStatus {
        state,
        pc_iriun: file.pc_iriun,
        phone_connected: file.phone_connected,
        phone_iriun: file.phone_iriun,
        screen_off: file.screen_off,
        error: file.error.filter(|error| !error.trim().is_empty()),
        pc_window: match file.pc_window.as_str() {
            Some("hidden") => Some(PcWindow::Hidden),
            Some("shown") => Some(PcWindow::Shown),
            _ => None,
        },
    })
}

/// Whether the unit runs and how it is. Off, the file is left alone: it may
/// be from an earlier run.
pub async fn camera() -> Camera {
    let output = crate::host::command("systemctl", &[])
        .args(["--user", "is-active", CAMERA_UNIT])
        .output()
        .await;
    // Not active, it exits with an error: what it prints says which.
    let on = match output {
        Ok(output) => camera_active(&String::from_utf8_lossy(&output.stdout)),
        Err(err) => {
            tracing::warn!("cannot ask systemd about the camera: {err}");
            false
        }
    };
    if !on {
        return Camera { on, status: None };
    }
    let path = user_dir("STATE", ".local/state").map(|dir| dir.join("iriun-adb/status.json"));
    let status = match path {
        Some(path) => crate::host::read(&path).await.as_deref().and_then(parse_camera_status),
        None => None,
    };
    Camera { on, status: live_camera_status(status) }
}

/// With the unit running, "stopped" is the last run's file, before the
/// service writes its first: loading, not off.
fn live_camera_status(status: Option<CameraStatus>) -> Option<CameraStatus> {
    status.filter(|status| status.state != Some(CameraState::Stopped))
}

/// The unit started, stopped or restarted (`start`, `stop`, `restart`);
/// then how it is. Stopping waits for the service to put the phone back.
pub async fn camera_command(verb: &'static str) -> Camera {
    let status = crate::host::command("systemctl", &[])
        .args(["--user", verb, CAMERA_UNIT])
        .status()
        .await;
    match status {
        Ok(status) if !status.success() => tracing::warn!("cannot {verb} the camera: {status}"),
        Err(err) => tracing::warn!("cannot {verb} the camera: {err}"),
        Ok(_) => {}
    }
    camera().await
}

/// Iriun's window on the computer shown (true) or hidden, by a signal to
/// the service's main process (SIGUSR1 shows, SIGUSR2 hides); then how the
/// camera is.
pub async fn camera_window(show: bool) -> Camera {
    let signal = if show { "SIGUSR1" } else { "SIGUSR2" };
    let status = crate::host::command("systemctl", &[])
        .args(["--user", "kill", "--kill-whom=main", "-s", signal, CAMERA_UNIT])
        .status()
        .await;
    match status {
        Ok(status) if !status.success() => tracing::warn!("cannot signal the camera's window: {status}"),
        Err(err) => tracing::warn!("cannot signal the camera's window: {err}"),
        Ok(_) => {}
    }
    // The service writes it down once done: a little while for that, or
    // the button would flip back until the page reads again.
    let wanted = if show { PcWindow::Shown } else { PcWindow::Hidden };
    let mut now = camera().await;
    for _ in 0..CAMERA_WINDOW_TRIES {
        if camera_window_settled(&now, wanted) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        now = camera().await;
    }
    now
}

/// Done waiting for the window: the camera off, or its file no longer
/// telling the window as it was (as wanted, or no window at all).
fn camera_window_settled(camera: &Camera, wanted: PcWindow) -> bool {
    !camera.on
        || camera
            .status
            .as_ref()
            .is_some_and(|status| status.pc_window.is_none_or(|window| window == wanted))
}

/// How many times, a tenth of a second apart, the window's change is
/// looked for in the service's file.
const CAMERA_WINDOW_TRIES: usize = 10;

/// Whether the bar leaves out the icons of the tiles that are on.
pub const HIDE_ACTIVE: Setting = Setting {
    config: crate::APP_ID,
    version: 1,
    key: "hide_active_tiles",
};

/// A COSMIC setting, read and written like cosmic-settings does.
pub struct Setting {
    pub config: &'static str,
    pub version: u64,
    pub key: &'static str,
}

pub const DO_NOT_DISTURB: Setting = Setting {
    config: "com.system76.CosmicNotifications",
    version: 1,
    key: "do_not_disturb",
};

pub const DARK_MODE: Setting = Setting {
    config: "com.system76.CosmicTheme.Mode",
    version: 1,
    key: "is_dark",
};

impl Setting {
    pub fn get(&self) -> bool {
        Config::new(self.config, self.version)
            .and_then(|c| c.get::<bool>(self.key))
            .unwrap_or(false)
    }

    pub fn set(&self, value: bool) {
        if let Err(err) =
            Config::new(self.config, self.version).and_then(|c| c.set(self.key, value))
        {
            tracing::warn!("cannot set {}: {err}", self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ddc_displays_are_read_back_and_nonsense_is_left_out() {
        let known = super::parse_ddc_displays("DP-1 1\nHDMI-A-1 1\n  DP-3 2 \nbroken\nDP-2 x\n");
        assert_eq!(known.len(), 3);
        assert_eq!(known["DP-3"], 2);
        assert_eq!(known["HDMI-A-1"], 1);
    }

    #[test]
    fn ddc_levels_are_read_back_and_nonsense_is_left_out() {
        let levels = super::parse_ddc_levels("1 50 100\n2 82 100\nx 1 2\n3 120 100\n4 1 0\n5 7\n");
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[&1], (50, 100));
        assert_eq!(levels[&2], (82, 100));
    }

    use super::*;

    #[test]
    fn displays() {
        let kdl = r#"output "eDP-1" enabled=#true {
  description make="AU Optronics" model="0xE3AC"
  scale 1.25
  modes {
    mode 1920 1200 60001 current=#true preferred=#true
    mode 1920 1200 40001
  }
}
output "HDMI-A-1" enabled=#false {
  description make="Dell Inc." model="U2419H"
  scale 1
  modes {
    mode 1920 1080 60000 current=#true
  }
}"#;
        let displays = parse_displays(kdl);
        assert_eq!(displays.len(), 2);
        assert_eq!(displays[0].name, "eDP-1");
        assert!(displays[0].enabled && displays[0].internal());
        assert_eq!((displays[0].width, displays[0].height, displays[0].refresh), (1920, 1200, 60001));
        assert_eq!(displays[0].scale, 1.25);
        assert_eq!(displays[1].model, "U2419H");
        assert!(!displays[1].enabled && !displays[1].internal());
        assert_eq!(parse_vcp("VCP 10 C 50 100\n"), Some((50, 100)));
    }

    #[test]
    fn volumes() {
        assert_eq!(
            parse_volume("Volume: 0.45\n"),
            Some(Volume {
                level: 0.45,
                muted: false
            })
        );
        assert_eq!(
            parse_volume("Volume: 1.00 [MUTED]\n"),
            Some(Volume {
                level: 1.0,
                muted: true
            })
        );
        assert_eq!(parse_volume(""), None);
    }

    #[test]
    fn camera_is_on_while_active_or_activating() {
        assert!(camera_active("active\n"));
        assert!(camera_active("activating\n"));
        assert!(!camera_active("inactive\n"));
        assert!(!camera_active("deactivating\n"));
        assert!(!camera_active("failed\n"));
        assert!(!camera_active(""));
    }

    #[test]
    fn camera_status() {
        let json = br#"{"state": "waiting_phone", "pc_iriun": true, "phone_connected": false,
            "phone_iriun": false, "screen_off": true, "error": null, "updated": 1761080000}"#;
        assert_eq!(
            parse_camera_status(json),
            Some(CameraStatus {
                state: Some(CameraState::WaitingPhone),
                pc_iriun: true,
                phone_connected: false,
                phone_iriun: false,
                screen_off: true,
                error: None,
                pc_window: None,
            })
        );
        let failed = br#"{"state": "error", "error": "PIN errado"}"#;
        assert_eq!(
            parse_camera_status(failed).map(|s| (s.state, s.error)),
            Some((Some(CameraState::Error), Some("PIN errado".to_owned())))
        );
        for (state, expected) in [
            ("starting", CameraState::Starting),
            ("active", CameraState::Active),
            ("stopped", CameraState::Stopped),
        ] {
            let json = format!(r#"{{"state": "{state}"}}"#);
            assert_eq!(parse_camera_status(json.as_bytes()).and_then(|s| s.state), Some(expected));
        }
    }

    #[test]
    fn camera_status_missing_fields_are_off() {
        assert_eq!(parse_camera_status(b"{}"), Some(CameraStatus::default()));
        let unknown = br#"{"state": "dancing", "pc_iriun": true, "error": ""}"#;
        let status = parse_camera_status(unknown).unwrap();
        assert_eq!(status.state, None);
        assert!(status.pc_iriun && !status.phone_connected);
        // An empty error is no error.
        assert_eq!(status.error, None);
    }

    #[test]
    fn camera_status_pc_window() {
        let window = |json: &str| parse_camera_status(json.as_bytes()).map(|s| s.pc_window);
        assert_eq!(window(r#"{"pc_window": "hidden"}"#), Some(Some(PcWindow::Hidden)));
        assert_eq!(window(r#"{"pc_window": "shown"}"#), Some(Some(PcWindow::Shown)));
        // No window, or one the applet doesn't know: no button.
        assert_eq!(window(r#"{"pc_window": null}"#), Some(None));
        assert_eq!(window(r#"{}"#), Some(None));
        assert_eq!(window(r#"{"pc_window": "minimized"}"#), Some(None));
        // Nor does an odd value spoil the rest of the file.
        assert_eq!(
            parse_camera_status(br#"{"state": "active", "pc_window": 1}"#).map(|s| (s.state, s.pc_window)),
            Some((Some(CameraState::Active), None))
        );
    }

    #[test]
    fn camera_window_settles_once_the_file_tells_it() {
        let with = |window| Camera {
            on: true,
            status: Some(CameraStatus { pc_window: window, ..Default::default() }),
        };
        assert!(camera_window_settled(&with(Some(PcWindow::Shown)), PcWindow::Shown));
        assert!(!camera_window_settled(&with(Some(PcWindow::Hidden)), PcWindow::Shown));
        assert!(camera_window_settled(&with(Some(PcWindow::Hidden)), PcWindow::Hidden));
        // No window any more: nothing to wait for.
        assert!(camera_window_settled(&with(None), PcWindow::Shown));
        // A file not read yet: wait.
        assert!(!camera_window_settled(&Camera { on: true, status: None }, PcWindow::Shown));
        assert!(camera_window_settled(&Camera::default(), PcWindow::Hidden));
    }

    #[test]
    fn stopped_status_of_a_running_unit_is_stale() {
        let stopped = parse_camera_status(br#"{"state": "stopped", "error": "PIN errado?"}"#);
        assert_eq!(live_camera_status(stopped), None);
        let active = parse_camera_status(br#"{"state": "active"}"#);
        assert_eq!(live_camera_status(active.clone()), active);
        assert_eq!(live_camera_status(None), None);
    }

    #[test]
    fn broken_camera_status_is_none() {
        assert_eq!(parse_camera_status(b""), None);
        assert_eq!(parse_camera_status(b"{\"state\": \"active\""), None);
        assert_eq!(parse_camera_status(b"[1, 2]"), None);
        assert_eq!(parse_camera_status(br#"{"pc_iriun": "yes"}"#), None);
        assert_eq!(parse_camera_status(&[0xff, 0xfe]), None);
    }
}
