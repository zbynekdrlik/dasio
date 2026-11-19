//! The CPU layout a dasio PC runs (dasio#128), computed from its logical
//! CPU count, so a new PC never works it out again:
//! - the system CPUs first: Windows, the Dante NIC's interrupts, the
//!   filter's deferred receive (on the last of them) and every other
//!   process; one per 8 CPUs, at least one;
//! - then each dasio instance's media threads: 4 CPUs on a PC of 16 or
//!   more (FOH's spinning pump, rx, io and helpers), else 2 (mbc's
//!   timer-waiting device);
//! - the DAW the rest.
//!
//! It is what the PCs proved: FOH (32 CPUs, 2 instances) 0-3 / 4-7 / 8-11
//! / 12-31, mbc (12, 1) 0-1 / 2-3 / 4-11, dsg (4, 1) 0 / 1-2 / 3.
//!
//! A profile asks for it with `computed = "system_cpus"` (a binary
//! affinity mask, e.g. the NIC's `AssignmentSetOverride`),
//! `computed = "dasio_cpus"` (a binary mask of every instance's CPUs, the
//! kernel's `ReservedCpuSets`, dasio#120) or `computed = "rxd_cpu"` (a
//! dword, the filter's `RxdCpu`) on a registry entry, and with `{system}`, `{dasio:N}` or `{daw}` as a process's
//! affinity; [`resolve`] puts the values in. The Dante NIC's receive-side
//! scaling range ([`Layout::rss_span`]) comes as `computed =
//! "rss_base_cpu"`, `"rss_max_cpu"` and `"rss_cpus"` (REG_SZ, like every
//! NIC advanced property: `*RssBaseProcNumber`, `*RssMaxProcNumber`,
//! `*MaxRssProcessors`).

use super::{Profile, parse_mask};

/// The names `computed` takes.
pub const COMPUTED: [&str; 6] = [
    "system_cpus",
    "dasio_cpus",
    "rxd_cpu",
    "rss_base_cpu",
    "rss_max_cpu",
    "rss_cpus",
];

/// The CPUs an affinity mask (a u64) can name: one processor group.
const MAX_CPUS: usize = 64;

/// One PC's CPU layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Logical CPUs (at most [`MAX_CPUS`] are laid out).
    pub cpus: usize,
    /// The system CPUs, 0 to `system - 1`.
    pub system: usize,
    /// CPUs per dasio instance.
    pub per_instance: usize,
    /// dasio instances on the PC.
    pub instances: usize,
}

/// The mask of CPUs `from` to `to - 1` (a span narrower than 64: the
/// system CPUs come first).
fn range(from: usize, to: usize) -> u64 {
    ((1u64 << (to - from)) - 1) << from
}

/// `from-to` (or one CPU) as the layout's text shows it.
fn span(from: usize, to: usize) -> String {
    if to - from == 1 {
        format!("{from}")
    } else {
        format!("{from}-{}", to - 1)
    }
}

impl Layout {
    /// The layout of a PC with `cpus` logical CPUs and `instances` dasio
    /// instances.
    pub fn of(cpus: usize, instances: usize) -> Self {
        let cpus = cpus.clamp(1, MAX_CPUS);
        Self {
            cpus,
            system: cpus.div_ceil(8),
            per_instance: if cpus >= 16 { 4 } else { 2 },
            instances,
        }
    }

    /// The system CPUs' mask (the NIC's interrupts go there).
    pub fn system_mask(&self) -> u64 {
        range(0, self.system)
    }

    /// The deferred receive's CPU: the last system CPU.
    pub fn rxd_cpu(&self) -> u32 {
        (self.system - 1) as u32
    }

    /// The CPUs the Dante NIC's receive-side scaling may use, first and
    /// last (dasio#128): the system CPUs, never dasio's or the DAW's, so
    /// the per-packet receive DPCs stay off dasio's pump (dsg 16.11:
    /// to_asio underruns ~10/10 min -> 0, dasio#120). Without CPU 0 when
    /// there are two or more system CPUs: every timer DPC lands there
    /// (mbc 17.11, dasio#24). FOH 32 = 1-3, mbc 12 = 1, dsg 4 = 0. It
    /// holds the deferred receive's CPU (the last system CPU) on purpose:
    /// both are the NIC's receive work, kept together off dasio's CPUs.
    pub fn rss_span(&self) -> (u32, u32) {
        let last = (self.system - 1) as u32;
        (u32::from(self.system >= 2), last)
    }

    /// How many CPUs [`Layout::rss_span`] holds (`*MaxRssProcessors`).
    pub fn rss_cpus(&self) -> u32 {
        let (first, last) = self.rss_span();
        last - first + 1
    }

    /// Instance `n`'s first CPU and the CPU after its last.
    fn dasio_span(&self, n: usize) -> Result<(usize, usize), String> {
        if n == 0 || n > self.instances {
            return Err(format!(
                "dasio:{n}: the profile has instances = {} (1 to {0})",
                self.instances
            ));
        }
        let from = self.system + (n - 1) * self.per_instance;
        let to = from + self.per_instance;
        if to > self.cpus {
            return Err(format!(
                "dasio:{n} needs CPUs {}, the PC has {}",
                span(from, to),
                self.cpus
            ));
        }
        Ok((from, to))
    }

    /// The CPUs of dasio instance `n` (1-based, in install order).
    pub fn dasio_mask(&self, n: usize) -> Result<u64, String> {
        let (from, to) = self.dasio_span(n)?;
        Ok(range(from, to))
    }

    /// Every instance's CPUs together (the CPUs Windows reserves for
    /// dasio, dasio#120). An instance that does not fit adds none: the
    /// reservation never makes a whole profile refuse to load (a profile
    /// that does not load stops the device), and the affinity placeholder
    /// of such an instance names the problem; 0 (nothing reserved) when
    /// none fits.
    pub fn dasio_cpus_mask(&self) -> u64 {
        let fit = (1..=self.instances)
            .take_while(|&n| self.dasio_mask(n).is_ok())
            .count();
        range(self.system, self.system + fit * self.per_instance)
    }

    /// The DAW's CPUs: those after the last instance's.
    pub fn daw_mask(&self) -> Result<u64, String> {
        let from = self.system + self.instances * self.per_instance;
        if from >= self.cpus {
            return Err(format!(
                "no CPU is left for the DAW: {} system and {} x {} dasio CPUs of {}",
                self.system, self.instances, self.per_instance, self.cpus
            ));
        }
        Ok(range(from, self.cpus))
    }

    /// One line, e.g. `system 0-3 (NIC interrupts, rxd 3), dasio:1 4-7,
    /// DAW 8-31`.
    pub fn describe(&self) -> String {
        let mut out = format!(
            "system {} (NIC interrupts, rxd {})",
            span(0, self.system),
            self.rxd_cpu()
        );
        for n in 1..=self.instances {
            match self.dasio_span(n) {
                Ok((from, to)) => out.push_str(&format!(", dasio:{n} {}", span(from, to))),
                Err(e) => out.push_str(&format!(", {e}")),
            }
        }
        let from = self.system + self.instances * self.per_instance;
        if from < self.cpus {
            out.push_str(&format!(", DAW {}", span(from, self.cpus)));
        } else {
            out.push_str(", no CPU left for the DAW");
        }
        out
    }

    /// The mask a process affinity placeholder stands for; `None` when
    /// `text` is not one.
    pub fn placeholder(&self, text: &str) -> Option<Result<u64, String>> {
        let inner = text.trim().strip_prefix('{')?.strip_suffix('}')?;
        match inner {
            "system" => Some(Ok(self.system_mask())),
            "daw" => Some(self.daw_mask()),
            _ => {
                let n = inner.strip_prefix("dasio:")?.parse().ok()?;
                Some(self.dasio_mask(n))
            }
        }
    }
}

/// Whether `text` is a process affinity the profile takes: a hex mask or
/// one of [`Layout::placeholder`]'s.
pub fn affinity_ok(text: &str) -> bool {
    parse_mask(text).is_some() || Layout::of(MAX_CPUS, 1).placeholder(text).is_some()
}

/// A mask as a registry binary: 8 bytes, little-endian, in hex.
fn mask_hex(mask: u64) -> String {
    mask.to_le_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `p` with its computed values put in, for a PC of `cpus` logical CPUs:
/// each `computed` registry entry gets its value, each placeholder
/// affinity (a process entry's, `others_affinity`) its hex mask. The error
/// names an entry the layout cannot fill.
pub fn resolve(p: &mut Profile, cpus: usize) -> Result<(), String> {
    let layout = Layout::of(cpus, p.instances.unwrap_or(1));
    for e in &mut p.registry {
        match e.computed.take().as_deref() {
            None => {}
            Some("system_cpus") => e.binary = Some(mask_hex(layout.system_mask())),
            Some("dasio_cpus") => e.binary = Some(mask_hex(layout.dasio_cpus_mask())),
            Some("rxd_cpu") => e.dword = Some(layout.rxd_cpu()),
            Some("rss_base_cpu") => e.string = Some(layout.rss_span().0.to_string()),
            Some("rss_max_cpu") => e.string = Some(layout.rss_span().1.to_string()),
            Some("rss_cpus") => e.string = Some(layout.rss_cpus().to_string()),
            Some(other) => {
                return Err(format!(
                    "registry {}\\{}: computed {other} is not one of {COMPUTED:?}",
                    e.key, e.name
                ));
            }
        }
    }
    for e in &mut p.process {
        let Some(a) = &e.affinity else { continue };
        if let Some(mask) = layout.placeholder(a) {
            let mask = mask.map_err(|err| format!("process {}: {err}", e.exe))?;
            e.affinity = Some(format!("0x{mask:X}"));
        }
    }
    if let Some(a) = &p.others_affinity
        && let Some(mask) = layout.placeholder(a)
    {
        let mask = mask.map_err(|err| format!("others_affinity: {err}"))?;
        p.others_affinity = Some(format!("0x{mask:X}"));
    }
    p.layout = Some(layout);
    Ok(())
}

/// The CPUs of `mask` as runs, e.g. `4-7` or `0-3, 16-31`; empty for 0.
pub fn cpus(mask: u64) -> String {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < MAX_CPUS {
        if mask & (1u64 << i) == 0 {
            i += 1;
            continue;
        }
        let from = i;
        while i < MAX_CPUS && mask & (1u64 << i) != 0 {
            i += 1;
        }
        runs.push(span(from, i));
    }
    runs.join(", ")
}

/// Whether instance `n` has CPUs of its own on the PC `p` was loaded for
/// (dasio#199), before install.ps1 touches it: the layout must hold it
/// (`instances` at least `n`, its CPUs on the PC) and no `[[process]]`
/// entry but a dasio exe's, nor `others_affinity`, may name its CPUs. The
/// line says which CPUs are its own, or what to change first.
pub fn instance_room(p: &Profile, n: usize) -> Result<String, String> {
    let l = p
        .layout
        .ok_or_else(|| "the profile's CPU layout was not computed".to_string())?;
    if n == 0 || n > l.instances {
        return Err(format!(
            "instance {n} has no CPUs of its own: the site profile has instances = {} (set instances = {n} there first; dasio pc-profile layout shows the CPUs it then gives)",
            l.instances
        ));
    }
    let own = l
        .dasio_mask(n)
        .map_err(|e| format!("instance {n} has no CPUs of its own: {e}"))?;
    let mut takers: Vec<String> = p
        .process
        .iter()
        .filter(|e| !super::keep::dasio_exe(&e.exe))
        .filter_map(|e| {
            let m = parse_mask(e.affinity.as_deref()?)?;
            (m & own != 0).then(|| format!("{} ({})", e.exe, cpus(m)))
        })
        .collect();
    if let Some(m) = p.others_affinity.as_deref().and_then(parse_mask)
        && m & own != 0
    {
        takers.push(format!("others_affinity ({})", cpus(m)));
    }
    if !takers.is_empty() {
        return Err(format!(
            "instance {n}'s CPUs {} are pinned to {} too: move them off in the site profile first (several dasio on one PC must not fight, dasio#199)",
            cpus(own),
            takers.join(", ")
        ));
    }
    Ok(format!(
        "instance {n}: CPUs {} are its own ({})",
        cpus(own),
        l.describe()
    ))
}

/// Why a dasio instance's CPUs are its own (dasio#199).
pub const SHARE_WHY: &str = "a dasio instance's CPUs are its own: a DAW or a second instance pinned there takes the pump's time and the device misses its deadlines (dasio#199, owner 19.11.2026: several dasio on one PC must not fight)";

/// The CPU shares in `p` (dasio#199), one problem each:
/// - a `[[process]]` entry that is not a dasio exe, or `others_affinity`,
///   whose affinity names a dasio instance's CPUs: the layout's instances
///   (`instances`, when [`resolve`] put the layout in) and each dasio exe
///   entry's own mask;
/// - two dasio exe entries on a common CPU.
///
/// Only hex masks count: a placeholder [`resolve`] did not fill is the
/// load's error, not a share. A dasio exe's own entry may span more than
/// its instance (a bridge is its own DAW: dsg's 1-3).
pub fn shares(p: &Profile) -> Vec<super::Problem> {
    let mask_of = |a: &Option<String>| a.as_deref().and_then(parse_mask);
    let dasio: Vec<(&str, u64)> = p
        .process
        .iter()
        .filter(|e| super::keep::dasio_exe(&e.exe))
        .filter_map(|e| Some((e.exe.as_str(), mask_of(&e.affinity)?)))
        .collect();
    let mut owners: Vec<(String, u64)> = Vec::new();
    if let Some(l) = p.layout {
        for n in 1..=l.instances {
            if let Ok(m) = l.dasio_mask(n) {
                owners.push((format!("dasio:{n}"), m));
            }
        }
    }
    owners.extend(dasio.iter().map(|(exe, m)| (exe.to_string(), *m)));
    // Without an owner nothing is shared, so the list is never empty in a
    // problem.
    let list: Vec<String> = owners
        .iter()
        .map(|(o, m)| format!("{o} {}", cpus(*m)))
        .collect();
    let want = format!("none of dasio's CPUs ({})", list.join(", "));
    let problem = |what: String, found: String| super::Problem {
        what,
        found,
        want: want.clone(),
        why: SHARE_WHY.into(),
        must: false,
    };
    let mut out = Vec::new();
    let mut judge = |what: String, mask: u64| {
        // Each shared CPU once, named by its first owner: an instance's
        // entry on the layout's own CPUs adds nothing to its line.
        let mut seen = 0u64;
        let mut hits = Vec::new();
        for (owner, m) in &owners {
            let shared = m & mask & !seen;
            if shared != 0 {
                hits.push(format!("{} with {owner}", cpus(shared)));
                seen |= shared;
            }
        }
        if !hits.is_empty() {
            out.push(problem(
                what,
                format!("CPUs {} share {}", cpus(mask), hits.join(", ")),
            ));
        }
    };
    for e in p.process.iter().filter(|e| !super::keep::dasio_exe(&e.exe)) {
        if let Some(mask) = mask_of(&e.affinity) {
            judge(format!("process {} affinity", e.exe), mask);
        }
    }
    if let Some(mask) = mask_of(&p.others_affinity) {
        judge("others_affinity".into(), mask);
    }
    for (i, (a, ma)) in dasio.iter().enumerate() {
        for (b, mb) in &dasio[i + 1..] {
            if ma & mb != 0 {
                out.push(problem(
                    format!("process {a} affinity"),
                    format!("CPUs {} share {} with {b}", cpus(*ma), cpus(ma & mb)),
                ));
            }
        }
    }
    out
}
