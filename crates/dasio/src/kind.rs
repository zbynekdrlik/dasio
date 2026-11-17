//! An instance's kind (dasio#106, the owner's decision of 13.11.2026 on
//! dstack#243): the DAW kind (dasio's own ASIO driver in a DAW, through the
//! shared memory) or a bridge to a local ASIO driver (e.g. Waves SoundGrid's
//! virtual soundcard), whose clock is not Dante's, on dstack-asio.
//!
//! The kind is the device config's `[bridge]` table, dasio's own like
//! `iface` ([`crate::place::device_config`] takes it out before dstack's
//! `DeviceConfig` reads the file); without it the instance is the DAW kind:
//!
//! ```toml
//! [bridge]
//! # The registered ASIO driver's name (HKLM\SOFTWARE\ASIO).
//! driver = "<driver name>"
//! # The driver output each RX channel goes to, in RX order (1-based).
//! to_asio = [1, 2]
//! # The driver input each TX channel comes from, in TX order (1-based).
//! from_asio = [1, 2]
//! # The FIFO target each way, ns (optional; default: the ASIO buffer
//! # plus one pump block, dstack#247).
//! app_buffer_ns = 5000000
//! # One direction's own FIFO target, ns (optional; default:
//! # app_buffer_ns; dstack issue 382, dasio#120).
//! to_asio_app_buffer_ns = 1333333
//! from_asio_app_buffer_ns = 1666666
//! # Both directions' converter length, taps (optional; 32 to 256 in
//! # steps of 8, default 256; dstack issue 382, dasio#120).
//! converter_taps = 48
//! ```
//!
//! Every RX channel has its driver output and every TX channel its driver
//! input: the pump's side of the bridge writes only the TX channels the
//! bridge carries, so a TX channel without an input would carry nothing
//! defined. Pure, tested on every OS; run.rs and run_bridge.rs call it.

use dlink::bridge::AppBuffer;
use dstack::device::DeviceConfig;
use dstack_asio::bridge::{MAX_CONVERTER_TAPS, MIN_CONVERTER_TAPS, Spec};
use dstack_asio::{Channels, Inputs};

/// The device config's table that makes an instance a bridge.
pub const CONFIG_BRIDGE: &str = "bridge";

/// The keys of the `[bridge]` table.
const KEYS: [&str; 7] = [
    "driver",
    "to_asio",
    "from_asio",
    "app_buffer_ns",
    "to_asio_app_buffer_ns",
    "from_asio_app_buffer_ns",
    "converter_taps",
];

/// An instance's kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// dasio's own ASIO driver in a DAW (no `[bridge]` table).
    Daw,
    /// A bridge to a local ASIO driver.
    Bridge(BridgeConfig),
}

/// The `[bridge]` table as written (channels 1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeConfig {
    /// The registered ASIO driver's name.
    pub driver: String,
    /// The driver output of each RX channel, in RX order.
    pub to_asio: Vec<u32>,
    /// The driver input of each TX channel, in TX order.
    pub from_asio: Vec<u32>,
    /// The FIFO target, ns (`None`: dstack's default).
    pub app_buffer_ns: Option<u64>,
    /// The to-ASIO FIFO's own target, ns (`None`: `app_buffer_ns`).
    pub to_asio_app_buffer_ns: Option<u64>,
    /// The from-ASIO FIFO's own target, ns (`None`: `app_buffer_ns`).
    pub from_asio_app_buffer_ns: Option<u64>,
    /// Both converters' sinc length, taps (`None`: dstack's 256).
    pub converter_taps: Option<u32>,
}

/// The kind the device config in `text` names.
pub fn kind(text: &str) -> Result<Kind, String> {
    let table: toml::Table = text
        .parse()
        .map_err(|e| format!("the config as a TOML table: {e}"))?;
    match table.get(CONFIG_BRIDGE) {
        None => Ok(Kind::Daw),
        Some(toml::Value::Table(t)) => bridge(t).map(Kind::Bridge),
        Some(_) => Err(format!(
            "`{CONFIG_BRIDGE}` in the device config must be a table"
        )),
    }
}

fn bridge(t: &toml::Table) -> Result<BridgeConfig, String> {
    if let Some(k) = t.keys().find(|k| !KEYS.contains(&k.as_str())) {
        return Err(format!(
            "[{CONFIG_BRIDGE}] has an unknown key `{k}` (known: {})",
            KEYS.join(", ")
        ));
    }
    let driver = match t.get("driver") {
        Some(toml::Value::String(d)) if !d.trim().is_empty() => d.clone(),
        _ => {
            return Err(format!(
                "[{CONFIG_BRIDGE}] needs `driver`, the registered ASIO driver's name"
            ));
        }
    };
    Ok(BridgeConfig {
        driver,
        to_asio: channel_list(t, "to_asio", "output")?,
        from_asio: channel_list(t, "from_asio", "input")?,
        app_buffer_ns: buffer_ns(t, "app_buffer_ns")?,
        to_asio_app_buffer_ns: buffer_ns(t, "to_asio_app_buffer_ns")?,
        from_asio_app_buffer_ns: buffer_ns(t, "from_asio_app_buffer_ns")?,
        converter_taps: converter_taps(t)?,
    })
}

/// The converter length under `converter_taps`, taps (absent: `None`),
/// held to what dstack-asio's `Bridge::new` takes (dstack issue 382):
/// [`MIN_CONVERTER_TAPS`] to [`MAX_CONVERTER_TAPS`] in steps of
/// [`dstack_resample::TAPS_STEP`].
fn converter_taps(t: &toml::Table) -> Result<Option<u32>, String> {
    const KEY: &str = "converter_taps";
    let Some(v) = t.get(KEY) else {
        return Ok(None);
    };
    let toml::Value::Integer(n) = v else {
        return Err(format!(
            "[{CONFIG_BRIDGE}] `{KEY}` must be a whole number of taps"
        ));
    };
    match u32::try_from(*n) {
        Ok(taps)
            if (MIN_CONVERTER_TAPS..=MAX_CONVERTER_TAPS).contains(&taps)
                && (taps as usize).is_multiple_of(dstack_resample::TAPS_STEP) =>
        {
            Ok(Some(taps))
        }
        _ => Err(format!(
            "[{CONFIG_BRIDGE}] `{KEY}` = {n}: {MIN_CONVERTER_TAPS} to {MAX_CONVERTER_TAPS} \
             in steps of {}",
            dstack_resample::TAPS_STEP
        )),
    }
}

/// A FIFO target under `key`, ns (absent: `None`), held to the range the
/// bridge's FIFO accepts (dlink's own check and text, dstack#247).
fn buffer_ns(t: &toml::Table, key: &str) -> Result<Option<u64>, String> {
    match t.get(key) {
        None => Ok(None),
        Some(toml::Value::Integer(n)) => {
            let Ok(n) = u64::try_from(*n) else {
                return Err(format!("[{CONFIG_BRIDGE}] `{key}` = {n}: a negative time"));
            };
            match AppBuffer::new(0, 0).set(Some(n)).problem() {
                // dlink names `app_buffer_ns`; the key read here is `key`.
                Some(problem) => Err(format!(
                    "[{CONFIG_BRIDGE}] {}",
                    problem.replacen("app_buffer_ns", key, 1)
                )),
                None => Ok(Some(n)),
            }
        }
        Some(_) => Err(format!(
            "[{CONFIG_BRIDGE}] `{key}` must be a whole number of ns"
        )),
    }
}

/// The 1-based driver channels under `key` (absent: none), each once.
fn channel_list(t: &toml::Table, key: &str, what: &str) -> Result<Vec<u32>, String> {
    let Some(v) = t.get(key) else {
        return Ok(Vec::new());
    };
    let toml::Value::Array(items) = v else {
        return Err(format!(
            "[{CONFIG_BRIDGE}] `{key}` must be a list of driver {what} numbers (1-based)"
        ));
    };
    let mut list = Vec::with_capacity(items.len());
    for item in items {
        let n = match item {
            toml::Value::Integer(n) => u32::try_from(*n).ok().filter(|&n| n > 0),
            _ => None,
        };
        let Some(n) = n else {
            return Err(format!(
                "[{CONFIG_BRIDGE}] `{key}`: {item} is not a driver {what} number (1-based)"
            ));
        };
        if list.contains(&n) {
            return Err(format!(
                "[{CONFIG_BRIDGE}] `{key}`: driver {what} {n} is given twice"
            ));
        }
        list.push(n);
    }
    Ok(list)
}

impl BridgeConfig {
    /// The bridge's [`Spec`] on the device `cfg`: refused unless every RX
    /// channel has its driver output and every TX channel its driver input,
    /// and the bridge carries at least one channel.
    pub fn spec(&self, cfg: &DeviceConfig) -> Result<Spec, String> {
        for (list, key, channels, side) in [
            (&self.to_asio, "to_asio", cfg.rx.len(), "RX"),
            (&self.from_asio, "from_asio", cfg.tx.len(), "TX"),
        ] {
            if list.len() != channels {
                return Err(format!(
                    "[{CONFIG_BRIDGE}] `{key}` names {} driver channels for the device's {channels} \
                     {side} channels: one for each",
                    list.len()
                ));
            }
        }
        if self.to_asio.is_empty() && self.from_asio.is_empty() {
            return Err(format!("[{CONFIG_BRIDGE}] carries no channel"));
        }
        let block_frames = u32::try_from(cfg.block)
            .map_err(|_| format!("the block of {} frames is too large", cfg.block))?;
        Ok(Spec {
            dante_rate: cfg.sample_rate,
            block_frames,
            to_asio: self.to_asio.len(),
            from_asio: self.from_asio.len(),
            app_buffer_ns: self.app_buffer_ns,
            to_asio_app_buffer_ns: self.to_asio_app_buffer_ns,
            from_asio_app_buffer_ns: self.from_asio_app_buffer_ns,
            converter_taps: self.converter_taps,
        })
    }

    /// The driver channels the runner opens (0-based): the inputs each TX
    /// channel comes from, exactly these, and the outputs each RX channel
    /// goes to.
    pub fn channels(&self) -> Channels {
        let zero_based = |l: &[u32]| l.iter().map(|c| c - 1).collect::<Vec<_>>();
        Channels {
            inputs: Inputs::Exact(zero_based(&self.from_asio)),
            outputs: zero_based(&self.to_asio),
        }
    }

    /// One line for the log and the tray: the driver, both maps, the FIFO
    /// targets (a direction's own only when it sets one) and the converter
    /// length when set.
    pub fn describe(&self) -> String {
        let list = |l: &[u32]| l.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let ns = |n: Option<u64>| n.map_or_else(|| "default".to_string(), |n| format!("{n} ns"));
        let mut line = format!(
            "bridge to \"{}\": RX -> outputs [{}], inputs [{}] -> TX, app buffer {}",
            self.driver,
            list(&self.to_asio),
            list(&self.from_asio),
            ns(self.app_buffer_ns),
        );
        for (side, own) in [
            ("to ASIO", self.to_asio_app_buffer_ns),
            ("from ASIO", self.from_asio_app_buffer_ns),
        ] {
            if own.is_some() {
                line.push_str(&format!(", {side} {}", ns(own)));
            }
        }
        if let Some(t) = self.converter_taps {
            line.push_str(&format!(", converter {t} taps"));
        }
        line
    }
}

#[cfg(test)]
mod tests;
