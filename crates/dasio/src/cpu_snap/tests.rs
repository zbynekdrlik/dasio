use super::*;

fn p(pid: u32, name: &str, ms: u64) -> Proc {
    Proc {
        pid,
        name: name.into(),
        cpu_100ns: ms * 10_000,
        io: Some(Io { read: 0, write: 0 }),
    }
}

/// A process with IO bytes so far (its CPU time does not matter here).
fn pio(pid: u32, name: &str, read: u64, write: u64) -> Proc {
    Proc {
        pid,
        name: name.into(),
        cpu_100ns: 0,
        io: Some(Io { read, write }),
    }
}

/// A process whose IO counters Windows refused in this sample.
fn refused(pid: u32, name: &str) -> Proc {
    Proc {
        pid,
        name: name.into(),
        cpu_100ns: 0,
        io: None,
    }
}

fn mv(id: u32, name: &str, read: u64, write: u64) -> Moved {
    Moved {
        id,
        name: name.into(),
        read,
        write,
    }
}

#[test]
fn the_processes_are_ranked_by_the_bytes_they_moved_between_the_samples() {
    let before = sample(
        0,
        vec![
            pio(10, "Ableton Live 12 Suite.exe", 1_000, 5_000),
            pio(20, "Ableton Index.exe", 300, 0),
            pio(30, "idle.exe", 7, 7),
            pio(40, "System", 0, 100),
            pio(50, "tiny.exe", 0, 0),
            pio(60, "kib.exe", 0, 0),
        ],
        vec![],
    );
    let now = sample(
        1000,
        vec![
            pio(10, "Ableton Live 12 Suite.exe", 1_500, 9_000),
            pio(20, "Ableton Index.exe", 6_300, 0),
            pio(30, "idle.exe", 7, 7),
            pio(40, "System", 0, 3_100),
            pio(50, "tiny.exe", 1_000, 23),
            pio(60, "kib.exe", 1_000, 24),
        ],
        vec![],
    );
    assert_eq!(
        top_io(&before, &now, 6),
        vec![
            mv(20, "Ableton Index.exe", 6_000, 0),
            mv(10, "Ableton Live 12 Suite.exe", 500, 4_000),
            mv(40, "System", 0, 3_000),
            mv(60, "kib.exe", 1_000, 24),
        ],
        "read plus written, the most first; under 1 KiB in all is left out"
    );
}

#[test]
fn io_counts_only_a_process_read_in_both_samples() {
    // Two AudioGridderServer sandboxes share a name: each is matched by its
    // own id. Id 10 now belongs to another exe, 23 is new, and 30/31 had
    // their counters refused in one of the samples: none of those counts,
    // or a lifetime total would show as one span's bytes.
    let before = sample(
        0,
        vec![
            pio(10, "old.exe", 100, 100),
            pio(21, "AudioGridderServer.exe", 10_000, 0),
            pio(22, "AudioGridderServer.exe", 500_000, 0),
            refused(30, "refused-before.exe"),
            pio(31, "refused-now.exe", 0, 0),
        ],
        vec![],
    );
    let now = sample(
        1000,
        vec![
            pio(10, "new.exe", 900_000, 0),
            pio(21, "AudioGridderServer.exe", 40_000, 0),
            pio(22, "AudioGridderServer.exe", 520_000, 0),
            pio(23, "started.exe", 0, 50_000),
            pio(30, "refused-before.exe", 40_000_000, 0),
            refused(31, "refused-now.exe"),
        ],
        vec![],
    );
    assert_eq!(
        top_io(&before, &now, 6),
        vec![
            mv(21, "AudioGridderServer.exe", 30_000, 0),
            mv(22, "AudioGridderServer.exe", 20_000, 0),
        ]
    );
}

#[test]
fn io_counters_that_went_back_move_nothing_and_only_the_top_n_are_kept() {
    let before = sample(0, vec![pio(10, "x.exe", 50_000, 50_000)], vec![]);
    let now = sample(1000, vec![pio(10, "x.exe", 40_000, 40_000)], vec![]);
    assert!(top_io(&before, &now, 6).is_empty());

    let zero = sample(
        0,
        vec![
            pio(3, "c", 0, 0),
            pio(1, "a", 0, 0),
            pio(2, "b", 0, 0),
            pio(4, "d", 0, 0),
        ],
        vec![],
    );
    let now = sample(
        1000,
        vec![
            pio(3, "c", 10_240, 0),
            pio(1, "a", 0, 10_240),
            pio(2, "b", 15_360, 5_120),
            pio(4, "d", 5_120, 0),
        ],
        vec![],
    );
    let ids: Vec<u32> = top_io(&zero, &now, 3).iter().map(|m| m.id).collect();
    assert_eq!(ids, vec![2, 1, 3], "ties go by id");
}

#[test]
fn the_io_list_reads_name_id_and_kibibytes_rounded_down() {
    let moved = vec![
        mv(780, "Ableton Live 12 Suite.exe", 3_145_728, 1_536),
        mv(4, "", 0, 1_023),
        mv(9, "x", 2_047, 0),
    ];
    assert_eq!(
        list_io(&moved),
        "Ableton Live 12 Suite.exe(780)=r3072KiB/w1KiB, ?(4)=r0KiB/w0KiB, x(9)=r1KiB/w0KiB"
    );
    assert_eq!(list_io(&[]), "");
}

fn disk(n: u32, read: u64, write: u64, queue: u32) -> Disk {
    Disk {
        n,
        read,
        write,
        queue,
    }
}

#[test]
fn each_disk_in_both_samples_shows_its_bytes_between_and_its_queue_now() {
    let mut before = sample(0, vec![], vec![]);
    before.disks = vec![disk(0, 100, 5_000, 7), disk(1, 9_000, 9_000, 0)];
    let mut now = sample(1000, vec![], vec![]);
    now.disks = vec![
        disk(0, 3_145_828, 6_536, 3),
        disk(1, 8_000, 9_000, 0),
        disk(2, 1 << 30, 0, 1),
    ];
    let moved = disk_moved(&before, &now);
    assert_eq!(
        moved,
        vec![disk(0, 3_145_728, 1_536, 3), disk(1, 8_000, 0, 0)],
        "disk 2 is new and left out; disk 1's read went back (a reset): 8000 since"
    );
    assert_eq!(list_disks(&moved), "0:r3072KiB/w1KiB/q3, 1:r7KiB/w0KiB/q0");
    assert_eq!(list_disks(&[]), "");
    assert!(disk_moved(&sample(0, vec![], vec![]), &now).is_empty());
}

/// The gatherer on the real Windows: bytes this process writes to a file
/// (and reads back) between two samples show as its IO, a delta of the
/// counters read in both.
#[cfg(windows)]
#[test]
fn the_bytes_this_process_writes_show_as_its_io() {
    use std::io::{Read, Write};
    let mut g = windows::Gatherer::new(|| 0);
    let before = g.sample(0);
    let name = format!("dasio-cpu-snap-io-{}.bin", std::process::id());
    let path = std::env::temp_dir().join(name);
    let block = vec![0x5au8; 1 << 20];
    {
        let mut f = std::fs::File::create(&path).unwrap();
        for _ in 0..4 {
            f.write_all(&block).unwrap();
        }
        f.sync_all().unwrap();
    }
    let mut back = Vec::new();
    std::fs::File::open(&path)
        .unwrap()
        .read_to_end(&mut back)
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    let now = g.sample(100);
    let me = std::process::id();
    // A real delta: this process was read in the first sample too (a
    // process missing there would not count at all).
    assert!(
        before.procs.iter().any(|p| p.pid == me && p.io.is_some()),
        "this process with its counters in the first sample"
    );
    let io = top_io(&before, &now, usize::MAX);
    let mine = io
        .iter()
        .find(|m| m.id == me)
        .unwrap_or_else(|| panic!("this process among the movers: {io:?}"));
    assert!(mine.write >= 4 << 20, "wrote {} bytes", mine.write);
    assert!(mine.read >= 4 << 20, "read {} bytes", mine.read);
    // The flushed 4 MiB reached a physical disk, which counts it too.
    assert!(!now.disks.is_empty(), "no physical disk could be read");
    let written: u64 = disk_moved(&before, &now).iter().map(|d| d.write).sum();
    assert!(written >= 4 << 20, "the disks wrote {written} bytes");
}

fn t(tid: u32, name: &str, ms: u64) -> Thread {
    Thread {
        tid,
        name: name.into(),
        cpu_100ns: ms * 10_000,
    }
}

fn sample(at_ms: u64, procs: Vec<Proc>, threads: Vec<Thread>) -> Sample {
    Sample {
        at_ms,
        procs,
        threads,
        daw: vec![],
        disks: vec![],
    }
}

#[test]
fn the_processes_are_ranked_by_what_they_spent_between_the_samples() {
    let before = sample(
        0,
        vec![
            p(10, "Ableton Live 12 Suite.exe", 5000),
            p(20, "MsMpEng.exe", 100),
            p(30, "idle.exe", 7),
        ],
        vec![],
    );
    let now = sample(
        1000,
        vec![
            p(10, "Ableton Live 12 Suite.exe", 5300),
            p(20, "MsMpEng.exe", 900),
            p(30, "idle.exe", 7),
        ],
        vec![],
    );
    let top = top_procs(&before, &now, 8);
    assert_eq!(
        top,
        vec![
            Spent {
                id: 20,
                name: "MsMpEng.exe".into(),
                us: 800_000
            },
            Spent {
                id: 10,
                name: "Ableton Live 12 Suite.exe".into(),
                us: 300_000
            },
        ],
        "the largest first; a process that spent nothing is left out"
    );
}

#[test]
fn a_new_process_counts_all_its_time_and_a_reused_id_is_a_new_process() {
    let before = sample(0, vec![p(10, "old.exe", 4000)], vec![]);
    let now = sample(
        1000,
        vec![p(10, "new.exe", 250), p(11, "started.exe", 40)],
        vec![],
    );
    let top = top_procs(&before, &now, 8);
    assert_eq!(
        top,
        vec![
            Spent {
                id: 10,
                name: "new.exe".into(),
                us: 250_000
            },
            Spent {
                id: 11,
                name: "started.exe".into(),
                us: 40_000
            },
        ]
    );
}

#[test]
fn a_counter_that_went_back_spends_nothing() {
    let before = sample(0, vec![p(10, "x.exe", 500)], vec![]);
    let now = sample(1000, vec![p(10, "x.exe", 400)], vec![]);
    assert!(top_procs(&before, &now, 8).is_empty());
}

#[test]
fn only_the_top_n_are_kept_and_ties_go_by_id() {
    let before = sample(0, vec![], vec![]);
    let now = sample(
        1000,
        vec![p(3, "c", 10), p(1, "a", 10), p(2, "b", 20), p(4, "d", 5)],
        vec![],
    );
    let ids: Vec<u32> = top_procs(&before, &now, 3).iter().map(|s| s.id).collect();
    assert_eq!(ids, vec![2, 1, 3]);
}

#[test]
fn the_device_threads_are_ranked_the_same_way() {
    let before = sample(0, vec![], vec![t(7, "dstack-engine", 100), t(8, "", 1)]);
    let now = sample(1000, vec![], vec![t(7, "dstack-engine", 160), t(8, "", 3)]);
    assert_eq!(
        top_threads(&before, &now, 4),
        vec![
            Spent {
                id: 7,
                name: "dstack-engine".into(),
                us: 60_000
            },
            Spent {
                id: 8,
                name: String::new(),
                us: 2_000
            },
        ]
    );
}

#[test]
fn the_list_reads_name_id_and_ms_to_a_tenth() {
    let spent = vec![
        Spent {
            id: 20,
            name: "MsMpEng.exe".into(),
            us: 812_345,
        },
        Spent {
            id: 8,
            name: String::new(),
            us: 2_050,
        },
        Spent {
            id: 9,
            name: "x".into(),
            us: 99,
        },
    ];
    assert_eq!(
        list(&spent),
        "MsMpEng.exe(20)=812.3ms, ?(8)=2.0ms, x(9)=0.0ms"
    );
    assert_eq!(list(&[]), "");
}

#[test]
fn a_filetime_joins_its_high_and_low_halves() {
    assert_eq!(filetime(0, 0), 0);
    assert_eq!(filetime(0, 7), 7);
    assert_eq!(filetime(1, 0), 1 << 32);
    assert_eq!(filetime(2, 5), (2 << 32) + 5);
    assert_eq!(filetime(u32::MAX, u32::MAX), u64::MAX);
}

#[test]
fn a_utf16_name_ends_at_its_first_nul() {
    let mut buf = [0u16; 8];
    for (d, c) in buf.iter_mut().zip("abc.exe".encode_utf16()) {
        *d = c;
    }
    assert_eq!(utf16_name(&buf), "abc.exe");
    let full: Vec<u16> = "xyz".encode_utf16().collect();
    assert_eq!(utf16_name(&full), "xyz", "no NUL: all of it");
    assert_eq!(utf16_name(&[0, 65]), "");
}

/// The gatherer on the real Windows: a thread that spins 300 ms between
/// two samples is this process's and among its top threads, by its name; the
/// second sample reads the handles the first opened (no refresh within
/// 10 s).
#[cfg(windows)]
#[test]
fn a_spinning_thread_shows_in_this_process_and_by_its_name() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let (spun, wait) = mpsc::channel();
    let (done, hold) = mpsc::channel::<()>();
    let (began, begun) = mpsc::channel::<()>();
    let spinner = std::thread::Builder::new()
        .name("cpu-snap-test".into())
        .spawn(move || {
            // std names the thread inside it before this runs: once this
            // is sent, the first sample reads the name (it reads a name
            // once, CI 43820329361 read it empty before it was set).
            began.send(()).unwrap();
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(300) {
                std::hint::spin_loop();
            }
            spun.send(()).unwrap();
            // Alive for the second sample.
            let _ = hold.recv();
        })
        .unwrap();
    // This test process stands in for the DAW: its threads are read as
    // the DAW's too.
    let mut g = windows::Gatherer::new(std::process::id);
    begun.recv().unwrap();
    let before = g.sample(0);
    wait.recv().unwrap();
    let now = g.sample(300);
    done.send(()).unwrap();
    spinner.join().unwrap();

    let me = std::process::id();
    let procs = top_procs(&before, &now, usize::MAX);
    let mine = procs
        .iter()
        .find(|s| s.id == me)
        .expect("this process in the sample");
    assert!(mine.us >= 100_000, "this process spent {} us", mine.us);
    // Among the top few, by its name: the tests beside it in this binary
    // run in parallel, and a busy one can outspend the spinner (CI
    // 43795232968: a converter test at 281 ms, dasio#107).
    let threads = top_threads(&before, &now, 4);
    let spinner = threads
        .iter()
        .find(|s| s.name == "cpu-snap-test")
        .unwrap_or_else(|| panic!("the spinner among the top threads: {threads:?}"));
    assert!(spinner.us >= 100_000, "{threads:?}");
    let daw = top_daw_threads(&before, &now, 4);
    assert!(
        daw.iter().any(|s| s.name == "cpu-snap-test"),
        "the DAW's threads by its pid: {daw:?}"
    );
}

/// The `cpu-snap` thread on the real Windows: it answers a burst request
/// and ends once its sender is dropped.
#[cfg(windows)]
#[test]
fn the_sampler_thread_answers_a_burst_and_ends_with_its_sender() {
    use std::time::{Duration, Instant};
    let (tx, thread) = windows::spawn(Instant::now(), || 0).expect("the sampler");
    std::thread::sleep(Duration::from_millis(1200));
    tx.send(()).expect("the sampler is running");
    drop(tx);
    let t = Instant::now();
    while !thread.is_finished() {
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "the sampler did not end after its sender was dropped"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    thread.join().expect("the sampler did not panic");
}

#[test]
fn the_daw_threads_are_ranked_apart_from_the_device_s() {
    let mut before = sample(0, vec![], vec![t(7, "dstack-engine", 100)]);
    before.daw = vec![t(40, "Ableton audio", 1000), t(41, "GUI", 50)];
    let mut now = sample(1000, vec![], vec![t(7, "dstack-engine", 101)]);
    now.daw = vec![t(40, "Ableton audio", 1900), t(41, "GUI", 60)];
    assert_eq!(
        top_daw_threads(&before, &now, 6),
        vec![
            Spent {
                id: 40,
                name: "Ableton audio".into(),
                us: 900_000
            },
            Spent {
                id: 41,
                name: "GUI".into(),
                us: 10_000
            },
        ]
    );
    assert_eq!(
        top_threads(&before, &now, 4).len(),
        1,
        "the device's own stay apart"
    );
    assert!(top_daw_threads(&sample(0, vec![], vec![]), &sample(1, vec![], vec![]), 6).is_empty());
}
