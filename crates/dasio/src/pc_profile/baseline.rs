//! The baseline every dasio PC gets (dasio#128), merged with the PC's own
//! (site) profile, and its must-have items.

use super::*;

/// The baseline every dasio PC gets (dasio#128): `config/baseline-pc-profile.toml`,
/// built into dasio. Its Dante NIC entries take the PC's `nic_guid`.
pub fn baseline() -> Result<Profile, String> {
    parse_with(BASELINE, false).map_err(|e| format!("the built-in baseline: {e}"))
}

/// The baseline's text, as shipped.
pub const BASELINE: &str = include_str!("../../../../config/baseline-pc-profile.toml");

/// What a PC is held to: the baseline and its own `site` profile. The
/// site's entry wins over the baseline's same entry (registry key and name,
/// ini file and key, process, drive, bcd element, task, absent target,
/// device); the rest is added.
/// Without the site's `nic_guid`, the baseline's Dante NIC entries are
/// left out (no NIC to read).
pub fn merge(base: &Profile, site: &Profile) -> Profile {
    let nic = |k: &str| k.contains("{nic_class}") || k.contains("{nic_device}");
    let mut out = site.clone();
    let mut registry: Vec<RegEntry> =
        base.registry
            .iter()
            .filter(|b| site.nic_guid.is_some() || !nic(&b.key))
            .filter(|b| {
                !site.registry.iter().any(|s| {
                    s.key.eq_ignore_ascii_case(&b.key) && s.name.eq_ignore_ascii_case(&b.name)
                })
            })
            .cloned()
            .collect();
    // A site value for a must-have item stays must-have: a PC may change
    // the value, not what dasio needs (dasio#128).
    registry.extend(site.registry.iter().map(|s| {
        let must = base.registry.iter().any(|b| {
            b.must && s.key.eq_ignore_ascii_case(&b.key) && s.name.eq_ignore_ascii_case(&b.name)
        });
        RegEntry {
            must: s.must || must,
            ..s.clone()
        }
    }));
    out.registry = registry;
    let mut ini: Vec<IniEntry> = base
        .ini
        .iter()
        .filter(|b| {
            !site
                .ini
                .iter()
                .any(|s| s.file.eq_ignore_ascii_case(&b.file) && s.key == b.key)
        })
        .cloned()
        .collect();
    ini.extend(site.ini.iter().cloned());
    out.ini = ini;
    // A site entry takes over only the fields it sets: the baseline's
    // integrity stays checked next to a site's Lasso affinity for the same
    // exe (dasio#128), each with its own reason; a baseline entry left
    // checking nothing goes.
    let mut process: Vec<ProcEntry> = base
        .process
        .iter()
        .map(|b| {
            let mut rest = b.clone();
            for s in site
                .process
                .iter()
                .filter(|s| s.exe.eq_ignore_ascii_case(&b.exe))
            {
                if s.affinity.is_some() {
                    rest.affinity = None;
                }
                if s.priority.is_some() {
                    rest.priority = None;
                }
                if s.integrity.is_some() {
                    rest.integrity = None;
                }
            }
            rest
        })
        .filter(ProcEntry::checks)
        .collect();
    process.extend(site.process.iter().cloned());
    out.process = process;
    let mut disk: Vec<DiskEntry> = base
        .disk
        .iter()
        .filter(|b| {
            !site
                .disk
                .iter()
                .any(|s| s.drive.eq_ignore_ascii_case(&b.drive))
        })
        .cloned()
        .collect();
    disk.extend(site.disk.iter().cloned());
    out.disk = disk;
    let mut bcd: Vec<BcdEntry> = base
        .bcd
        .iter()
        .filter(|b| {
            !site
                .bcd
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case(&b.name))
        })
        .cloned()
        .collect();
    bcd.extend(site.bcd.iter().map(|s| {
        let must = base
            .bcd
            .iter()
            .any(|b| b.must && s.name.eq_ignore_ascii_case(&b.name));
        BcdEntry {
            must: s.must || must,
            ..s.clone()
        }
    }));
    out.bcd = bcd;
    let mut task: Vec<TaskEntry> = base
        .task
        .iter()
        .filter(|b| {
            !site
                .task
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case(&b.name))
        })
        .cloned()
        .collect();
    task.extend(site.task.iter().cloned());
    out.task = task;
    let mut absent: Vec<AbsentEntry> = base
        .absent
        .iter()
        .filter(|b| !site.absent.iter().any(|s| s.same_target(b)))
        .cloned()
        .collect();
    absent.extend(site.absent.iter().cloned());
    out.absent = absent;
    let mut nic_driver: Vec<NicDriverEntry> = base
        .nic_driver
        .iter()
        .filter(|b| {
            !site
                .nic_driver
                .iter()
                .any(|s| s.family.eq_ignore_ascii_case(&b.family))
        })
        .cloned()
        .collect();
    nic_driver.extend(site.nic_driver.iter().cloned());
    out.nic_driver = nic_driver;
    let mut device: Vec<DeviceEntry> = base
        .device
        .iter()
        .filter(|b| {
            !site
                .device
                .iter()
                .any(|s| s.instance_id.eq_ignore_ascii_case(&b.instance_id))
        })
        .cloned()
        .collect();
    device.extend(site.device.iter().cloned());
    out.device = device;
    out
}

/// What a PC is held to when it has no site profile: the baseline alone,
/// without its Dante NIC entries.
pub fn baseline_only(base: &Profile) -> Profile {
    let empty = Profile {
        name: "baseline only".into(),
        nic_guid: None,
        instances: None,
        registry: Vec::new(),
        ini: Vec::new(),
        process: Vec::new(),
        disk: Vec::new(),
        bcd: Vec::new(),
        task: Vec::new(),
        absent: Vec::new(),
        nic_driver: Vec::new(),
        device: Vec::new(),
    };
    merge(base, &empty)
}

/// What the PC at `path`'s profile is held to: the baseline merged with
/// the site profile at `path`, or the baseline alone when there is no
/// file there (dasio#128), its CPU masks computed for a PC of `cpus`
/// logical CPUs ([`resolve`]).
pub fn load(path: &std::path::Path, cpus: usize) -> Result<Profile, String> {
    let mut p = load_merged(path)?;
    resolve(&mut p, cpus)?;
    Ok(p)
}

/// The baseline merged with the site profile at `path` (the baseline
/// alone without a file there), its CPU masks not computed yet: what
/// [`load`] starts from, and all `dasio pc-profile nic-driver` needs.
pub fn load_merged(path: &std::path::Path) -> Result<Profile, String> {
    let base = baseline()?;
    Ok(match std::fs::read_to_string(path) {
        Ok(text) => merge(
            &base,
            &parse(&text).map_err(|e| format!("{}: {e}", path.display()))?,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => baseline_only(&base),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    })
}

/// How a must-have problem's text starts (dasio#128): the service ends on
/// one, `dasio pc-profile check` exits 2, `install.ps1` refuses.
pub const MUST_HAVE: &str = "must-have: ";

/// Why the device must not run, when `problems` (as [`Problem`]'s text)
/// hold a must-have item; `None` when they hold none.
pub fn must_have_failure(problems: &[String]) -> Option<String> {
    let must: Vec<&str> = problems
        .iter()
        .filter_map(|t| t.strip_prefix(MUST_HAVE))
        .collect();
    if must.is_empty() {
        return None;
    }
    Some(format!(
        "the PC misses {} must-have item(s) of dasio's baseline (dasio#128): {}; set them (dasio pc-profile apply, then restart the Dante NIC) and start dasio again",
        must.len(),
        must.join("; ")
    ))
}
