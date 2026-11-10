use super::*;

fn p(pid: u32, name: &str, ms: u64) -> Proc {
    Proc {
        pid,
        name: name.into(),
        cpu_100ns: ms * 10_000,
    }
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
/// two samples is this process's and its own top thread, by its name; the
/// second sample reads the handles the first opened (no refresh within
/// 10 s).
#[cfg(windows)]
#[test]
fn a_spinning_thread_shows_in_this_process_and_by_its_name() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let (spun, wait) = mpsc::channel();
    let (done, hold) = mpsc::channel::<()>();
    let spinner = std::thread::Builder::new()
        .name("cpu-snap-test".into())
        .spawn(move || {
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(300) {
                std::hint::spin_loop();
            }
            spun.send(()).unwrap();
            // Alive for the second sample.
            let _ = hold.recv();
        })
        .unwrap();
    let mut g = windows::Gatherer::new();
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
    let threads = top_threads(&before, &now, 1);
    assert_eq!(
        threads.first().map(|s| s.name.as_str()),
        Some("cpu-snap-test"),
        "{threads:?}"
    );
    assert!(threads[0].us >= 100_000, "{threads:?}");
}

/// The `cpu-snap` thread on the real Windows: it answers a burst request
/// and ends once its sender is dropped.
#[cfg(windows)]
#[test]
fn the_sampler_thread_answers_a_burst_and_ends_with_its_sender() {
    use std::time::{Duration, Instant};
    let (tx, thread) = windows::spawn(Instant::now()).expect("the sampler");
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
