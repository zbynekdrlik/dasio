//! dasio's CPUs as Windows CPU sets (dasio#120): the baseline reserves
//! the layout's dasio CPUs (`ReservedCpuSets`, read at boot), so no thread
//! runs there unless its process asks for them; at start the service asks
//! for its instance's CPUs with `SetProcessDefaultCpuSets`.
//!
//! Why: Process Lasso's affinities cannot hold a protected process
//! (Defender's MsMpEng) or a short-lived one; on dsg MsMpEng ran on the
//! pump's CPU for 0.4-1.4 ms whenever the pump yielded, and the pump woke
//! ~2.5 ms late. A reserved CPU keeps every such thread off.
//!
//! From Microsoft's CPU Sets documentation: reserved sets carry the
//! `Allocated` flag in `SYSTEM_CPU_SET_INFORMATION`; a process's default
//! sets apply to every thread without its own; "if a thread or process has
//! a restrictive affinity mask set, the affinity mask is respected above
//! any conflicting CPU Set assignment", so Lasso's `{dasio:N}` affinity
//! stays in force.
//!
//! The decision is pure ([`decide`] over a [`parse`]d
//! `GetSystemCpuSetInformation` buffer); the Windows calls are in
//! [`windows`](self::windows).

use crate::pc_profile::Layout;

/// `SYSTEM_CPU_SET_INFORMATION`'s `Type` for a CPU set
/// (`CpuSetInformation`), the only type Windows defines.
const CPU_SET_INFORMATION: u32 = 0;
/// The bytes of an entry up to and including `AllFlags` (offset 19).
const ENTRY_MIN: usize = 20;
/// `AllFlags`' `Parked` bit.
const PARKED: u8 = 1;
/// `AllFlags`' `Allocated` bit: the set is reserved.
const ALLOCATED: u8 = 1 << 1;

/// One CPU set, as Windows describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuSet {
    /// The id `SetProcessDefaultCpuSets` takes.
    pub id: u32,
    /// The processor group.
    pub group: u16,
    /// The group-relative logical processor (its home CPU).
    pub cpu: u8,
    /// Reserved (`Allocated`): only a process that asks runs there.
    pub allocated: bool,
    /// Parked for power or thermal reasons.
    pub parked: bool,
}

/// The CPU sets in a `GetSystemCpuSetInformation` buffer: variable-sized
/// entries walked by their `Size`, other types skipped (Microsoft: "use the
/// size field to determine the offset to the next structure"). Layout
/// (winnt.h): `Size` u32 @0, `Type` u32 @4, `Id` u32 @8, `Group` u16 @12,
/// `LogicalProcessorIndex` u8 @14, `AllFlags` u8 @19.
pub fn parse(buf: &[u8]) -> Result<Vec<CpuSet>, String> {
    let u32_at = |at: usize| u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]);
    let mut out = Vec::new();
    let mut at = 0usize;
    // Each entry takes 8 bytes or more: at most len / 8 of them, so the
    // walk ends even on a buffer it misreads.
    for _ in 0..buf.len().div_ceil(8) {
        // The walk never passes the end: an entry is never larger than
        // what is left (checked below).
        if at == buf.len() {
            break;
        }
        if buf.len() - at < 8 {
            return Err(format!("a CPU set entry at byte {at} is cut short"));
        }
        let size = u32_at(at) as usize;
        if size < 8 || size > buf.len() - at {
            return Err(format!(
                "a CPU set entry at byte {at} has size {size} ({} bytes left)",
                buf.len() - at
            ));
        }
        if u32_at(at + 4) == CPU_SET_INFORMATION {
            if size < ENTRY_MIN {
                return Err(format!("a CPU set entry at byte {at} has size {size}"));
            }
            let flags = buf[at + 19];
            out.push(CpuSet {
                id: u32_at(at + 8),
                group: u16::from_le_bytes([buf[at + 12], buf[at + 13]]),
                cpu: buf[at + 14],
                allocated: flags & ALLOCATED != 0,
                parked: flags & PARKED != 0,
            });
        }
        at += size;
    }
    Ok(out)
}

/// The group-0 CPUs (0 to 63) whose sets are reserved.
pub fn reserved_mask(sets: &[CpuSet]) -> u64 {
    sets.iter()
        .filter(|s| s.group == 0 && s.allocated && s.cpu < 64)
        .fold(0, |m, s| m | 1 << s.cpu)
}

/// The ids of the group-0 sets whose CPU is in `mask`
/// ([`crate::cpu_set_ids`]).
pub fn ids_for(mask: u64, sets: &[CpuSet]) -> Vec<u32> {
    let tuples: Vec<(u16, u8, u32)> = sets.iter().map(|s| (s.group, s.cpu, s.id)).collect();
    crate::cpu_set_ids(mask as usize, &tuples)
}

/// The CPUs the start gave the process as its default CPU sets (0: none),
/// for the helper placer, which narrows its own to them. Process-wide, as
/// the default CPU sets are.
static ASSIGNED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The CPUs the start gave the process (`windows::assign_at_start`); 0
/// when none.
pub fn assigned() -> u64 {
    ASSIGNED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Records the CPUs the start gave the process.
pub fn set_assigned(cpus: u64) {
    ASSIGNED.store(cpus, std::sync::atomic::Ordering::Relaxed);
}

/// The helper threads' CPUs within the process's reserved ones
/// (`assigned`): the helper placer otherwise gives them every CPU the
/// media threads leave, which without a Lasso affinity includes another
/// instance's reserved CPUs (review of dasio#120). When the assigned CPUs
/// are all media CPUs (or none were assigned), `helpers` as before.
pub fn helpers_within(helpers: u64, assigned: u64) -> u64 {
    match helpers & assigned {
        0 => helpers,
        both => both,
    }
}

/// What the service does with its CPU sets at start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Every CPU of the instance is reserved: the process's default sets
    /// are `ids` (CPUs `cpus`).
    Assign { ids: Vec<u32>, cpus: u64 },
    /// Some of the instance's CPUs (`missing` of `own`) are not reserved:
    /// nothing is set, the process keeps its affinity (Lasso's) alone.
    NotReserved { own: u64, missing: u64 },
    /// The layout has no CPUs for this instance (the reason).
    NoLayout(String),
}

/// The CPU sets instance `instance` (1-based) asks for: its layout CPUs
/// (`{dasio:N}`) when all of them are reserved. A bridge (`bridge`) is its
/// own ASIO host, no DAW runs on its PC: it also takes the DAW CPUs the PC
/// reserves (dsg's Lasso gives it 1-3). A CPU that is not reserved is
/// never asked for: other processes' threads run there anyway.
pub fn decide(sets: &[CpuSet], layout: &Layout, instance: usize, bridge: bool) -> Decision {
    let own = match layout.dasio_mask(instance) {
        Ok(m) => m,
        Err(e) => return Decision::NoLayout(e),
    };
    let reserved = reserved_mask(sets);
    let missing = own & !reserved;
    if missing != 0 {
        return Decision::NotReserved { own, missing };
    }
    let extra = if bridge {
        layout.daw_mask().unwrap_or(0) & reserved
    } else {
        0
    };
    let cpus = own | extra;
    Decision::Assign {
        ids: ids_for(cpus, sets),
        cpus,
    }
}

#[cfg(windows)]
pub mod windows;

#[cfg(test)]
mod tests;
