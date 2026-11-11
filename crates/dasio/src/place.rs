//! Where an instance's Dante device serves: its address, given as
//! `--ip <address>` or `--iface <name>` (dasio#29: the interface's one
//! IPv4 address, the rule dstack's devices use), and its control ports and
//! process id, which a config that names none takes from the first free
//! slot, as dsend and dlisten do (dasio#89: beside Audinate's ConMon, which
//! holds 0.0.0.0:8700 and 8800 on a PC with Dante Controller, dstack#200).
//! The decisions are here, portable and tested on every OS; run.rs and the
//! service call them.

use std::net::Ipv4Addr;

use dstack::daemon::{IfAddr, Place, pick_interface};
use dstack::device::DeviceConfig;

/// The refusal when the command line gives neither or both of the flags.
pub const ONE_PLACE: &str = "give exactly one of --ip <address> and --iface <interface name>";

/// The device's place from `--ip <address>` or `--iface <name>` in `args`:
/// exactly one of the two, once.
pub fn parse(args: &[String]) -> Result<Place, String> {
    let mut found = None;
    for (i, flag) in args.iter().enumerate() {
        if flag != "--ip" && flag != "--iface" {
            continue;
        }
        let value = args
            .get(i + 1)
            .filter(|v| !v.is_empty() && !v.starts_with("--"))
            .ok_or_else(|| format!("{flag} needs a value"))?;
        let place = if flag == "--ip" {
            Place::Ip(
                value
                    .parse()
                    .map_err(|_| format!("--ip {value}: not an IPv4 address"))?,
            )
        } else {
            Place::Iface(value.clone())
        };
        if found.replace(place).is_some() {
            return Err(ONE_PLACE.into());
        }
    }
    found.ok_or_else(|| ONE_PLACE.into())
}

/// The device's address on `place` among `addrs`, dstack's rule
/// ([`pick_interface`], as its own start applies it): the named
/// interface's one IPv4 address, or the given address when an interface
/// holds it; none, several, or one the Dante network cannot reach
/// (loopback, unspecified) is refused.
pub fn address_in(place: &Place, addrs: &[IfAddr]) -> Result<Ipv4Addr, String> {
    pick_interface(place, addrs)
        .map(|a| a.ip)
        .map_err(|e| match place {
            Place::Iface(name) => format!("the interface {name:?}: {e}"),
            Place::Ip(ip) => format!("{ip}: {e}"),
        })
}

/// [`address_in`] on this box's interfaces now (an interface at boot may
/// have no address yet: the service asks again).
pub fn address(place: &Place) -> Result<Ipv4Addr, String> {
    let addrs = dstack::host::interfaces()
        .map_err(|e| format!("the box's interfaces could not be read: {e}"))?;
    address_in(place, &addrs)
}

/// The device config's keys that set its control ports and process id.
pub const SLOT_KEYS: [&str; 4] = ["arc_port", "cmc_port", "flows_port", "process_id"];

/// A device's control ports (ARC, CMC, flow control) and process id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub arc: u16,
    pub cmc: u16,
    pub flows: u16,
    pub process_id: u16,
}

/// Where a device's control ports and process id came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotFrom {
    /// The config names at least one of [`SLOT_KEYS`]: it is kept as
    /// written (the stack's default for any key it leaves out).
    Config,
    /// The config names none: the first free slot, written into it.
    Picked,
    /// The config names none and no slot was free: the stack's defaults
    /// (the start then names the port it cannot bind).
    Defaults,
}

/// The device config in `text`, with dstack's checks: as written when it
/// names any of [`SLOT_KEYS`] (explicit ports win); else with the slot
/// `pick` gives (`slots::pick` on the device's address), or dstack's
/// defaults when it gives none. `pick` runs only for a config that names
/// none and passes the checks as written; a picked slot the checks refuse
/// is named in the error.
pub fn device_config(
    text: &str,
    pick: impl FnOnce() -> Option<Slot>,
) -> Result<(DeviceConfig, SlotFrom), String> {
    // As written first: its errors name the file's own lines.
    let written = DeviceConfig::from_toml(text).map_err(|e| e.to_string())?;
    let mut table: toml::Table = text
        .parse()
        .map_err(|e| format!("the config as a TOML table: {e}"))?;
    if SLOT_KEYS.iter().any(|k| table.contains_key(*k)) {
        return Ok((written, SlotFrom::Config));
    }
    let Some(slot) = pick() else {
        return Ok((written, SlotFrom::Defaults));
    };
    for (key, value) in SLOT_KEYS
        .into_iter()
        .zip([slot.arc, slot.cmc, slot.flows, slot.process_id])
    {
        table.insert(key.into(), toml::Value::Integer(i64::from(value)));
    }
    let named = format!(
        "the free slot picked for it (arc_port {}, cmc_port {}, flows_port {}, process_id {})",
        slot.arc, slot.cmc, slot.flows, slot.process_id
    );
    let picked = toml::to_string(&table).map_err(|e| format!("{named}: {e}"))?;
    DeviceConfig::from_toml(&picked)
        .map(|c| (c, SlotFrom::Picked))
        .map_err(|e| format!("{named}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = include_str!("../../../config/test-dante-device.toml");

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_address_comes_from_ip_or_iface() {
        assert_eq!(
            parse(&args(&["run", "--instance", "1", "--ip", "10.77.7.232"])),
            Ok(Place::Ip(Ipv4Addr::new(10, 77, 7, 232)))
        );
        assert_eq!(
            parse(&args(&["service", "--iface", "Ethernet 2", "--log", "x"])),
            Ok(Place::Iface("Ethernet 2".into()))
        );
    }

    #[test]
    fn neither_both_or_twice_is_refused() {
        assert_eq!(
            parse(&args(&["run", "--instance", "1"])),
            Err(ONE_PLACE.into())
        );
        assert_eq!(
            parse(&args(&["run", "--ip", "10.0.0.1", "--iface", "eth0"])),
            Err(ONE_PLACE.into())
        );
        assert_eq!(
            parse(&args(&["run", "--iface", "eth0", "--iface", "eth1"])),
            Err(ONE_PLACE.into())
        );
        assert_eq!(
            parse(&args(&["run", "--ip", "10.0.0.1", "--ip", "10.0.0.1"])),
            Err(ONE_PLACE.into())
        );
    }

    #[test]
    fn a_missing_or_bad_value_is_refused() {
        assert_eq!(
            parse(&args(&["run", "--ip", "10.0.0"])),
            Err("--ip 10.0.0: not an IPv4 address".into())
        );
        assert_eq!(
            parse(&args(&["run", "--iface"])),
            Err("--iface needs a value".into())
        );
        assert_eq!(
            parse(&args(&["run", "--iface", "--log", "x"])),
            Err("--iface needs a value".into())
        );
        assert_eq!(
            parse(&args(&["run", "--ip", ""])),
            Err("--ip needs a value".into())
        );
    }

    fn addrs() -> Vec<IfAddr> {
        vec![
            IfAddr {
                name: "Ethernet 2".into(),
                ip: Ipv4Addr::new(10, 77, 7, 232),
                netmask: Ipv4Addr::new(255, 255, 255, 0),
            },
            IfAddr {
                name: "Wi-Fi".into(),
                ip: Ipv4Addr::new(192, 168, 1, 5),
                netmask: Ipv4Addr::new(255, 255, 255, 0),
            },
        ]
    }

    #[test]
    fn an_interface_name_gives_its_address() {
        assert_eq!(
            address_in(&Place::Iface("Ethernet 2".into()), &addrs()),
            Ok(Ipv4Addr::new(10, 77, 7, 232))
        );
        assert_eq!(
            address_in(&Place::Iface("Wi-Fi".into()), &addrs()),
            Ok(Ipv4Addr::new(192, 168, 1, 5))
        );
    }

    #[test]
    fn an_unknown_interface_is_refused_and_named() {
        let e = address_in(&Place::Iface("Ethernet 9".into()), &addrs()).unwrap_err();
        assert!(e.starts_with("the interface \"Ethernet 9\": "), "{e}");
    }

    #[test]
    fn an_address_must_be_one_an_interface_holds() {
        let ip = Ipv4Addr::new(10, 77, 7, 232);
        assert_eq!(address_in(&Place::Ip(ip), &addrs()), Ok(ip));
        let e = address_in(&Place::Ip(Ipv4Addr::new(10, 77, 7, 9)), &addrs()).unwrap_err();
        assert!(e.starts_with("10.77.7.9: "), "{e}");
        // No interface of this box holds a documentation address.
        assert!(address(&Place::Ip(Ipv4Addr::new(192, 0, 2, 1))).is_err());
    }

    #[test]
    fn a_loopback_or_unspecified_address_is_refused() {
        let lo = vec![IfAddr {
            name: "lo".into(),
            ip: Ipv4Addr::LOCALHOST,
            netmask: Ipv4Addr::new(255, 0, 0, 0),
        }];
        assert!(address_in(&Place::Ip(Ipv4Addr::LOCALHOST), &lo).is_err());
        assert!(address_in(&Place::Iface("lo".into()), &lo).is_err());
        assert!(address_in(&Place::Ip(Ipv4Addr::UNSPECIFIED), &addrs()).is_err());
    }

    const SLOT_1: Slot = Slot {
        arc: 4441,
        cmc: 8801,
        flows: 4456,
        process_id: 1,
    };

    fn slot_of(c: &DeviceConfig) -> Slot {
        Slot {
            arc: c.arc_port,
            cmc: c.cmc_port,
            flows: c.flows_port,
            process_id: c.process_id,
        }
    }

    #[test]
    fn a_config_without_ports_takes_the_picked_slot() {
        let (cfg, from) = device_config(DEVICE, || Some(SLOT_1)).unwrap();
        assert_eq!(from, SlotFrom::Picked);
        assert_eq!(slot_of(&cfg), SLOT_1);
        // The rest of the config is as written.
        let written = DeviceConfig::from_toml(DEVICE).unwrap();
        assert_eq!(cfg.name, written.name);
        assert_eq!(cfg.rx, written.rx);
        assert_eq!(cfg.tx, written.tx);
        assert_eq!(cfg.latency_ns, written.latency_ns);
    }

    #[test]
    fn explicit_ports_are_kept_and_nothing_is_picked() {
        for key in SLOT_KEYS {
            let text = format!("{key} = 7\n{DEVICE}");
            let (cfg, from) = device_config(&text, || panic!("picked for {key}")).unwrap();
            assert_eq!(from, SlotFrom::Config);
            let written = DeviceConfig::from_toml(&text).unwrap();
            assert_eq!(slot_of(&cfg), slot_of(&written), "{key}");
        }
    }

    #[test]
    fn no_free_slot_keeps_the_stack_s_defaults() {
        let (cfg, from) = device_config(DEVICE, || None).unwrap();
        assert_eq!(from, SlotFrom::Defaults);
        assert_eq!(
            slot_of(&cfg),
            Slot {
                arc: 4440,
                cmc: 8800,
                flows: 4455,
                process_id: 0
            }
        );
    }

    #[test]
    fn the_picked_config_passes_dstack_s_checks() {
        // RX ports from 4456: clear of the defaults (4455 the last), not of
        // slot 1's flow control (4456). The picked slot is refused, as it
        // would be written by hand.
        let text = format!("rx_port_base = 4456\n{DEVICE}");
        assert!(DeviceConfig::from_toml(&text).is_ok());
        let e = device_config(&text, || Some(SLOT_1)).unwrap_err();
        assert!(e.contains("rx_port_base"), "{e}");
        let named = "the free slot picked for it \
                     (arc_port 4441, cmc_port 8801, flows_port 4456, process_id 1): ";
        assert!(e.starts_with(named), "{e}");
    }

    #[test]
    fn a_broken_config_is_refused_without_a_pick() {
        let e = device_config("name = ", || panic!("picked")).unwrap_err();
        assert!(e.starts_with("device config: "), "{e}");
        // TOML, but not a device config as written: refused before a pick.
        let text = format!("timing = \"fast\"\n{DEVICE}");
        let e = device_config(&text, || panic!("picked")).unwrap_err();
        assert!(e.starts_with("device config: "), "{e}");
    }
}
