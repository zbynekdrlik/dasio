//! Who held the CPU when the DAW missed blocks (dasio#22): a burst line
//! says when (to 0.1 s), this says who. Once a second the device takes a
//! sample of every process's CPU time and of its own threads' (the Windows
//! gatherer, [`windows`]); at a burst start it takes one more at once and
//! logs what each spent since the sample before, which is at most a second
//! back and so covers the stall. No kernel tracing: a standing ETW session
//! lengthened band Live's callbacks on FOH (+50–150 % over-period callbacks,
//! dasio#22, 10.11.2026). The same sample carries each process's bytes read
//! and written and each physical disk's, so a burst also says how much the
//! disks moved in that span and which processes moved data (the owner's
//! "bursty disk work", dasio#22, 24.11.2026). A process's read/write bytes
//! are its file and device IO (cached reads included); its sockets (band
//! Live streams to AudioGridder over TCP) land in "other", shown apart.
//! The disks' count is only the disks'. The diffs, the rankings and the
//! line are pure, so tested everywhere.

/// One process's CPU time and IO so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    /// Kernel plus user time, in 100 ns units (Windows' FILETIME).
    pub cpu_100ns: u64,
    /// Bytes read and written so far, `None` when Windows refused the
    /// counters for this sample.
    pub io: Option<Io>,
}

/// A process's IO so far, `IO_COUNTERS`' transfer counts: `read` and
/// `write` (file and device reads and writes) and `other` (control
/// calls, where Windows counts socket sends and receives).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Io {
    pub read: u64,
    pub write: u64,
    pub other: u64,
}

/// One physical disk's counters so far (`IOCTL_DISK_PERFORMANCE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disk {
    /// Its number, `\\.\PhysicalDrive<n>`.
    pub n: u32,
    pub read: u64,
    pub write: u64,
    /// Reads completed so far (Windows' 32-bit count, it wraps).
    pub reads: u32,
    /// Writes completed so far (32-bit, wraps).
    pub writes: u32,
    /// The requests in flight when read.
    pub queue: u32,
}

/// One of this process's threads and its CPU time so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub tid: u32,
    /// Its description (`SetThreadDescription`), empty without one.
    pub name: String,
    pub cpu_100ns: u64,
}

/// Everything taken at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sample {
    /// When it was taken, ms of any steady clock.
    pub at_ms: u64,
    pub procs: Vec<Proc>,
    pub threads: Vec<Thread>,
    /// The threads of the DAW that has the instance's buffers (empty
    /// without one, or with a driver before 0.1.52, which does not say).
    pub daw: Vec<Thread>,
    /// The physical disks (empty where they cannot be read).
    pub disks: Vec<Disk>,
}

/// What one process or thread spent between two samples, µs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spent {
    pub id: u32,
    pub name: String,
    pub us: u64,
}

/// The processes listed in a burst's line.
pub const TOP_PROCS: usize = 8;
/// The device's own threads listed in a burst's line.
pub const TOP_THREADS: usize = 4;
/// The DAW's threads listed in a burst's line.
pub const TOP_DAW_THREADS: usize = 6;

/// What each of `now`'s entries spent since `before`, the largest first,
/// at most `n`, none that spent nothing. An entry is the same one only with
/// the same id and name (an id Windows reused for another process is a new
/// one); a new one counts all its time, since it started within the span.
fn ranked<'a>(
    before: impl Iterator<Item = (u32, &'a str, u64)> + Clone,
    now: impl Iterator<Item = (u32, &'a str, u64)>,
    n: usize,
) -> Vec<Spent> {
    let mut out: Vec<Spent> = now
        .map(|(id, name, cpu)| {
            let prev = before
                .clone()
                .find(|&(i, nm, _)| i == id && nm == name)
                .map_or(0, |(_, _, c)| c);
            Spent {
                id,
                name: name.to_string(),
                us: cpu.saturating_sub(prev) / 10,
            }
        })
        .filter(|s| s.us > 0)
        .collect();
    out.sort_by(|a, b| b.us.cmp(&a.us).then(a.id.cmp(&b.id)));
    out.truncate(n);
    out
}

/// The processes that spent the most CPU from `before` to `now`.
pub fn top_procs(before: &Sample, now: &Sample, n: usize) -> Vec<Spent> {
    ranked(
        before
            .procs
            .iter()
            .map(|p| (p.pid, p.name.as_str(), p.cpu_100ns)),
        now.procs
            .iter()
            .map(|p| (p.pid, p.name.as_str(), p.cpu_100ns)),
        n,
    )
}

/// The threads in `before` and `now` that spent the most CPU between.
fn top_of(before: &[Thread], now: &[Thread], n: usize) -> Vec<Spent> {
    ranked(
        before.iter().map(|t| (t.tid, t.name.as_str(), t.cpu_100ns)),
        now.iter().map(|t| (t.tid, t.name.as_str(), t.cpu_100ns)),
        n,
    )
}

/// The device's threads that spent the most CPU from `before` to `now`.
pub fn top_threads(before: &Sample, now: &Sample, n: usize) -> Vec<Spent> {
    top_of(&before.threads, &now.threads, n)
}

/// The DAW's threads that spent the most CPU from `before` to `now`.
pub fn top_daw_threads(before: &Sample, now: &Sample, n: usize) -> Vec<Spent> {
    top_of(&before.daw, &now.daw, n)
}

/// `spent` as `name(id)=ms`, comma separated, ms to 0.1; an unnamed thread
/// is `?`.
pub fn list(spent: &[Spent]) -> String {
    spent
        .iter()
        .map(|s| {
            let name = if s.name.is_empty() { "?" } else { &s.name };
            format!("{name}({})={}.{}ms", s.id, s.us / 1000, s.us % 1000 / 100)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What one process read and wrote between two samples, bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    pub id: u32,
    pub name: String,
    pub read: u64,
    pub write: u64,
    /// Bytes of other IO (sockets): shown, not ranked.
    pub other: u64,
}

impl Moved {
    /// The bytes a process is ranked by: read plus written.
    fn total(&self) -> u64 {
        self.read.saturating_add(self.write)
    }
}

/// The processes listed by their IO in a burst's line.
pub const TOP_IO: usize = 8;

/// The fewest bytes a process must read or write to be listed: the line
/// prints KiB, so less would read `r0KiB/w0KiB`.
pub const MOVED_AT_LEAST: u64 = 1024;

/// The processes that read plus wrote the most bytes from `before` to
/// `now`, the most first (ties by id), at most `n`, none that read and
/// wrote less than [`MOVED_AT_LEAST`] each (a socket-only process is not a
/// disk suspect). The same process is the same id and name, as in [`ranked`].
/// Unlike CPU, only a process with readable counters in both samples
/// counts: a new one (the gatherer opens new processes only every 10 s)
/// or one whose counters Windows refused once would show its lifetime
/// bytes, gigabytes for a DAW that loaded a set, as one span's.
pub fn top_io(before: &Sample, now: &Sample, n: usize) -> Vec<Moved> {
    let mut out: Vec<Moved> = now
        .procs
        .iter()
        .filter_map(|p| {
            let was = before
                .procs
                .iter()
                .find(|b| b.pid == p.pid && b.name == p.name)?
                .io?;
            let is = p.io?;
            Some(Moved {
                id: p.pid,
                name: p.name.clone(),
                read: is.read.saturating_sub(was.read),
                write: is.write.saturating_sub(was.write),
                other: is.other.saturating_sub(was.other),
            })
        })
        .filter(|m| m.read >= MOVED_AT_LEAST || m.write >= MOVED_AT_LEAST)
        .collect();
    out.sort_by(|a, b| b.total().cmp(&a.total()).then(a.id.cmp(&b.id)));
    out.truncate(n);
    out
}

/// What one disk did between two samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskMoved {
    pub n: u32,
    pub read: u64,
    pub write: u64,
    /// Reads plus writes that completed in the span.
    pub requests: u64,
    /// The requests in flight at `now`, the burst's sample, so after the
    /// stall.
    pub queue: u32,
}

/// What each disk in both samples did between them, by disk number. A
/// disk only in `now` is left out, and so is one whose byte counters went
/// back (reset within the span: diskperf, a driver reset, another disk
/// under the same number), since no delta of it is true. The 32-bit
/// request counts are taken modulo 2^32, so their wrap is no reset.
pub fn disk_moved(before: &Sample, now: &Sample) -> Vec<DiskMoved> {
    now.disks
        .iter()
        .filter_map(|d| {
            let was = before.disks.iter().find(|b| b.n == d.n)?;
            (d.read >= was.read && d.write >= was.write).then(|| DiskMoved {
                n: d.n,
                read: d.read - was.read,
                write: d.write - was.write,
                requests: u64::from(d.reads.wrapping_sub(was.reads))
                    + u64::from(d.writes.wrapping_sub(was.writes)),
                queue: d.queue,
            })
        })
        .collect()
}

/// `disks` as `<n>:r<KiB>/w<KiB>/n<requests>/q<depth>`, comma separated,
/// KiB rounded down.
pub fn list_disks(disks: &[DiskMoved]) -> String {
    disks
        .iter()
        .map(|d| {
            format!(
                "{}:r{}KiB/w{}KiB/n{}/q{}",
                d.n,
                d.read / 1024,
                d.write / 1024,
                d.requests,
                d.queue
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `moved` as `name(id)=r<KiB>/w<KiB>/o<KiB>`, comma separated, KiB
/// rounded down; an unnamed process is `?`.
pub fn list_io(moved: &[Moved]) -> String {
    moved
        .iter()
        .map(|m| {
            let name = if m.name.is_empty() { "?" } else { &m.name };
            format!(
                "{name}({})=r{}KiB/w{}KiB/o{}KiB",
                m.id,
                m.read / 1024,
                m.write / 1024,
                m.other / 1024
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A FILETIME's two halves as one count of 100 ns units.
pub fn filetime(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

/// A NUL-terminated UTF-16 buffer (a Toolhelp exe name) as a string: up to
/// its first NUL, or all of it without one.
pub fn utf16_name(s: &[u16]) -> String {
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..n])
}

#[cfg(windows)]
pub mod windows;

#[cfg(test)]
mod tests;
