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
/// ini file and key, process, drive, bcd element, task); the rest is added.
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
    let mut process: Vec<ProcEntry> = base
        .process
        .iter()
        .filter(|b| {
            !site
                .process
                .iter()
                .any(|s| s.exe.eq_ignore_ascii_case(&b.exe))
        })
        .cloned()
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
    };
    merge(base, &empty)
}

/// What the PC at `path`'s profile is held to: the baseline merged with
/// the site profile at `path`, or the baseline alone when there is no
/// file there (dasio#128), its CPU masks computed for a PC of `cpus`
/// logical CPUs ([`resolve`]).
pub fn load(path: &std::path::Path, cpus: usize) -> Result<Profile, String> {
    let base = baseline()?;
    let mut p = match std::fs::read_to_string(path) {
        Ok(text) => merge(
            &base,
            &parse(&text).map_err(|e| format!("{}: {e}", path.display()))?,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => baseline_only(&base),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    resolve(&mut p, cpus)?;
    Ok(p)
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
