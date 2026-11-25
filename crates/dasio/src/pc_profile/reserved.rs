//! A written `ReservedCpuSets` is not yet a reserved CPU (dasio#237).
//!
//! Windows reads `Session Manager\kernel\ReservedCpuSets` at boot only, so
//! right after `apply` the check reads the value as wanted while no CPU is
//! reserved yet: dasio's CPUs still take other processes' threads (alex-nb
//! 25.11.2026: the TX thread overslept 5-10 ms until the reboot). The check
//! compares the value with the CPUs Windows reserves now (the `Allocated`
//! CPU sets, [`crate::cpu_sets::reserved_mask`]).
//!
//! This part is portable and tested everywhere; reading the reserved CPUs
//! is [`windows`](super::windows).

use super::{Problem, Profile, Reader, Value, applies, want_reg};

/// The key of the value, as the baseline names it.
pub const KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\kernel";
/// The value's name.
pub const NAME: &str = "ReservedCpuSets";

/// The CPU mask of a `ReservedCpuSets` value: its first 8 bytes, little
/// endian (CPU 0 is bit 0); a shorter value is read as if zero-filled.
pub fn mask(bytes: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    for (to, from) in b.iter_mut().zip(bytes) {
        *to = *from;
    }
    u64::from_le_bytes(b)
}

/// The one problem a written, not yet booted `ReservedCpuSets` gives: only
/// when the profile's entry applies and reads as the profile wants (a
/// mismatch is its own problem already) and Windows reserves fewer CPUs
/// than the value names. `Ok(None)` from the reader (nothing to read on
/// this OS) says nothing.
pub fn check(p: &Profile, r: &dyn Reader) -> Option<Problem> {
    let want = p.registry.iter().find_map(|e| {
        if !(e.key.eq_ignore_ascii_case(KEY) && e.name.eq_ignore_ascii_case(NAME) && applies(e, r))
        {
            return None;
        }
        let Some(Value::Binary(b)) = want_reg(e) else {
            return None;
        };
        match r.reg(&e.key, &e.name) {
            Ok(Some(Value::Binary(have))) if have == b => Some(mask(&b)),
            _ => None,
        }
    })?;
    let found = match r.reserved_cpus() {
        Ok(None) => return None,
        Ok(Some(have)) if have & want == want => return None,
        Ok(Some(have)) => format!(
            "written ({want:#x}) but Windows reserves {have:#x} now: it reads the value at boot, so a reboot is pending"
        ),
        Err(e) => format!(
            "unreadable ({e}): cannot tell whether the written ReservedCpuSets is in effect"
        ),
    };
    Some(Problem {
        what: "dasio's reserved CPUs (ReservedCpuSets)".into(),
        found,
        want: format!("CPUs {want:#x} reserved: a reboot after the write (dasio#237)"),
        why: "Windows reads ReservedCpuSets at boot only; until then other processes' threads run on dasio's CPUs".into(),
        must: false,
    })
}
