use super::*;

fn sample(cpu: f64, disk: Option<f64>) -> Sample {
    Sample {
        cpu_percent: Some(cpu),
        disk_queue: disk,
    }
}

const QUIET: Sample = Sample {
    cpu_percent: Some(3.0),
    disk_queue: Some(0.01),
};

const BUSY: Sample = Sample {
    cpu_percent: Some(85.0),
    disk_queue: Some(2.5),
};

#[test]
fn a_start_within_ten_minutes_of_the_boot_is_a_boot_start() {
    assert_eq!(BOOT_WINDOW, Duration::from_secs(600));
    assert!(is_boot(Duration::ZERO));
    assert!(is_boot(Duration::from_secs(45)));
    assert!(is_boot(BOOT_WINDOW - Duration::from_millis(1)));
    assert!(!is_boot(BOOT_WINDOW));
    assert!(!is_boot(Duration::from_secs(86_400)));
}

#[test]
fn only_a_boot_start_off_the_test_clock_waits() {
    let boot = Duration::from_secs(40);
    let later = Duration::from_secs(3600);
    assert_eq!(at_once(boot, false), None);
    assert_eq!(at_once(later, false), Some(Why::NotABoot));
    assert_eq!(at_once(boot, true), Some(Why::TestClock));
    assert_eq!(at_once(later, true), Some(Why::TestClock));
}

#[test]
fn the_cpu_busy_is_the_non_idle_share_of_the_time_that_passed() {
    let before = CpuTimes {
        idle: 1_000,
        kernel: 3_000,
        user: 500,
    };
    // 100 units passed: kernel 80 (idle 60 of it), user 20: 40 % busy.
    let after = CpuTimes {
        idle: 1_060,
        kernel: 3_080,
        user: 520,
    };
    assert_eq!(busy_percent(before, after), Some(40.0));
    // All idle, then all busy.
    let idle = CpuTimes {
        idle: 1_100,
        kernel: 3_100,
        user: 500,
    };
    assert_eq!(busy_percent(before, idle), Some(0.0));
    let busy = CpuTimes {
        idle: 1_000,
        kernel: 3_030,
        user: 570,
    };
    assert_eq!(busy_percent(before, busy), Some(100.0));
    // A quarter, at an offset, so neither the share nor the scale is a
    // coincidence: 400 passed, 300 idle.
    let quarter = CpuTimes {
        idle: 1_300,
        kernel: 3_350,
        user: 550,
    };
    assert_eq!(busy_percent(before, quarter), Some(25.0));
}

#[test]
fn no_time_or_a_counter_that_went_back_gives_no_busy() {
    let t = CpuTimes {
        idle: 10,
        kernel: 30,
        user: 5,
    };
    assert_eq!(busy_percent(t, t), None);
    for back in [
        CpuTimes { idle: 9, ..t },
        CpuTimes { kernel: 29, ..t },
        CpuTimes { user: 4, ..t },
    ] {
        assert_eq!(busy_percent(t, back), None, "{back:?}");
    }
}

#[test]
fn a_calm_second_has_the_cpu_and_the_disk_at_or_under_their_bounds() {
    assert_eq!(CPU_CALM_PERCENT, 20.0);
    assert_eq!(DISK_CALM_QUEUE, 0.5);
    assert!(is_calm(sample(20.0, Some(0.5))));
    assert!(is_calm(sample(0.0, Some(0.0))));
    assert!(!is_calm(sample(20.1, Some(0.0))));
    assert!(!is_calm(sample(0.0, Some(0.51))));
    assert!(!is_calm(sample(20.1, Some(0.51))));
    // A disk queue PDH cannot give: the CPU alone decides.
    assert!(is_calm(sample(19.0, None)));
    assert!(!is_calm(sample(21.0, None)));
    // An unread CPU is never calm.
    assert!(!is_calm(Sample {
        cpu_percent: None,
        disk_queue: Some(0.0),
    }));
    assert!(!is_calm(Sample::default()));
}

#[test]
fn the_progress_figures_are_rounded_or_unread() {
    assert_eq!(shown(Some(37.46), 1), "37.5");
    assert_eq!(shown(Some(1.234), 2), "1.23");
    assert_eq!(shown(None, 1), "unread");
}

#[test]
fn twenty_calm_seconds_in_a_row_end_the_wait() {
    assert_eq!(CALM_FOR_S, 20);
    let mut c = Calm::default();
    for i in 1..CALM_FOR_S {
        assert!(matches!(c.step(QUIET), Step::Wait { .. }), "sample {i}");
        assert_eq!(c.calm_s(), i);
    }
    assert_eq!(c.step(QUIET), Step::Calm);
    assert_eq!((c.waited_s(), c.calm_s()), (20, 20));
}

#[test]
fn a_busy_second_starts_the_calm_count_again() {
    let mut c = Calm::default();
    for _ in 1..CALM_FOR_S {
        c.step(QUIET);
    }
    assert_eq!(c.step(BUSY), Step::Wait { log: true }, "sample 20");
    assert_eq!(c.calm_s(), 0);
    for _ in 1..CALM_FOR_S {
        assert!(matches!(c.step(QUIET), Step::Wait { .. }));
    }
    assert_eq!(c.step(QUIET), Step::Calm);
    assert_eq!(c.waited_s(), 40);
}

#[test]
fn the_wait_ends_at_the_cap_without_calm() {
    assert_eq!(CAP_S, 300);
    let mut c = Calm::default();
    for i in 1..CAP_S {
        assert!(matches!(c.step(BUSY), Step::Wait { .. }), "sample {i}");
    }
    assert_eq!(c.step(BUSY), Step::Cap);
    assert_eq!(c.waited_s(), CAP_S);
}

#[test]
fn calm_at_the_cap_s_last_second_is_calm() {
    let mut c = Calm::default();
    for _ in 0..CAP_S - CALM_FOR_S {
        c.step(BUSY);
    }
    for _ in 1..CALM_FOR_S {
        c.step(QUIET);
    }
    assert_eq!(c.step(QUIET), Step::Calm);
    assert_eq!(c.waited_s(), CAP_S);
}

#[test]
fn a_progress_line_is_due_every_ten_seconds() {
    assert_eq!(LOG_EVERY_S, 10);
    let mut c = Calm::default();
    let logs: Vec<u32> = (1..=35)
        .filter(|_| c.step(BUSY) == Step::Wait { log: true })
        .collect();
    assert_eq!(logs, [10, 20, 30]);
}

#[test]
fn the_wait_returns_once_the_pc_is_calm() {
    let stop = AtomicBool::new(false);
    let mut reads = 0;
    let mut paused = Duration::ZERO;
    let out = wait(
        || {
            reads += 1;
            if reads <= 5 { BUSY } else { QUIET }
        },
        &stop,
        |d| paused += d,
    );
    assert_eq!(
        out,
        Outcome {
            why: Why::Calm,
            waited_s: 25
        }
    );
    assert_eq!(reads, 25);
    assert_eq!(paused, 25 * SAMPLE);
}

#[test]
fn the_wait_starts_anyway_at_the_cap() {
    let stop = AtomicBool::new(false);
    let mut reads = 0;
    let out = wait(
        || {
            reads += 1;
            BUSY
        },
        &stop,
        |_| {},
    );
    assert_eq!(
        out,
        Outcome {
            why: Why::Cap,
            waited_s: CAP_S
        }
    );
    assert_eq!(reads, CAP_S);
}

#[test]
fn a_stop_ends_the_wait_without_a_start() {
    let stop = AtomicBool::new(false);
    let mut reads = 0;
    let out = wait(
        || {
            reads += 1;
            if reads == 3 {
                stop.store(true, Ordering::Release);
            }
            BUSY
        },
        &stop,
        |_| {},
    );
    assert_eq!(
        out,
        Outcome {
            why: Why::Stopped,
            waited_s: 3
        }
    );
    assert_eq!(reads, 3);
}

#[test]
fn a_start_that_does_not_wait_never_opens_the_reader() {
    let stop = AtomicBool::new(false);
    for (uptime, test_clock, why) in [
        (Duration::from_secs(3600), false, Why::NotABoot),
        (Duration::from_secs(30), true, Why::TestClock),
    ] {
        let out = before_start(
            uptime,
            test_clock,
            || -> fn() -> Sample { panic!("opened the reader for {why:?}") },
            &stop,
            |_| panic!("paused for {why:?}"),
        );
        assert_eq!(out, Outcome { why, waited_s: 0 });
    }
}

#[test]
fn a_boot_start_waits_with_the_reader_it_opens() {
    let stop = AtomicBool::new(false);
    let mut opened = 0;
    let out = before_start(
        Duration::from_secs(50),
        false,
        || {
            opened += 1;
            || QUIET
        },
        &stop,
        |_| {},
    );
    assert_eq!(opened, 1);
    assert_eq!(
        out,
        Outcome {
            why: Why::Calm,
            waited_s: CALM_FOR_S
        }
    );
}

#[test]
fn the_reasons_and_end_lines_name_each_outcome() {
    assert_eq!(Why::Calm.name(), "calm");
    assert_eq!(Why::Cap.name(), "cap");
    assert_eq!(Why::NotABoot.name(), "not-a-boot");
    assert_eq!(Why::TestClock.name(), "test-clock");
    assert_eq!(Why::Stopped.name(), "stopped");
    let starts = (
        false,
        "the wait for a calm Windows is over: the device starts its media",
    );
    for why in [Why::Calm, Why::NotABoot, Why::TestClock] {
        assert_eq!(end_line(why), starts, "{why:?}");
    }
    assert_eq!(
        end_line(Why::Cap),
        (
            true,
            "Windows did not calm within the cap: the device starts its media anyway"
        )
    );
    assert_eq!(
        end_line(Why::Stopped),
        (
            false,
            "a stop was asked while waiting for a calm Windows: the device does not start"
        )
    );
}

#[cfg(windows)]
#[test]
fn the_windows_reader_gives_the_uptime_the_cpu_busy_and_the_disk_queue() {
    let up = windows::uptime();
    assert!(up > Duration::ZERO, "{up:?}");
    let mut r = windows::Reader::open();
    std::thread::sleep(Duration::from_millis(1100));
    let s = r.read();
    let cpu = s.cpu_percent.expect("GetSystemTimes moved on");
    assert!((0.0..=100.0).contains(&cpu), "{cpu}");
    let disk = s.disk_queue.expect("PDH reads the runner's disk queue");
    assert!(disk >= 0.0, "{disk}");
    assert!(windows::uptime() > up);
}
