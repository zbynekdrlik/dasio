use super::*;

#[test]
fn dates_and_minutes_are_utc_at_known_epochs() {
    for (s, want) in [
        (0, "1970-01-01T00:00Z"),
        (951_782_400, "2000-02-29T00:00Z"),
        (946_684_740, "1999-12-31T23:59Z"),
        (1_709_251_140, "2024-02-29T23:59Z"),
        (1_794_305_220, "2026-11-10T10:07Z"),
        (4_107_542_400, "2100-03-01T00:00Z"),
    ] {
        assert_eq!(minute_utc(s), want, "{s}");
        // Seconds inside the minute do not show.
        assert_eq!(minute_utc(s + 59), want, "{s} + 59");
    }
    assert_eq!(date(1_794_305_220), "2026-11-10");
}

fn totals(late: u64, driver_missed: u64, missed: u64) -> Totals {
    Totals {
        late,
        driver_missed,
        missed,
        ..Totals::default()
    }
}

/// 10:07:00Z on 10.11.2026.
const T: u64 = 1_794_305_220;

#[test]
fn ticks_fold_into_their_minute_and_a_new_minute_hands_it_over() {
    let mut agg = MinuteAgg::default();
    let w = |max_us, over| Window {
        blocks: 3_750,
        over,
        max_us,
        wake_max_us: max_us / 10,
        process_max_us: max_us / 2,
    };
    // The first tick is the base: nothing grew yet.
    assert_eq!(agg.tick(T, totals(5, 2, 0), w(900, 0), true), None);
    assert_eq!(agg.tick(T + 10, totals(7, 2, 0), w(4_000, 2), true), None);
    assert_eq!(agg.tick(T + 50, totals(8, 3, 1), w(1_200, 1), false), None);
    // 10:08:00: the 10:07 row comes out.
    let row = agg
        .tick(T + 60, totals(8, 3, 1), w(500, 0), true)
        .expect("the 10:07 row");
    assert_eq!(row.minute, T);
    assert_eq!(row.grown, totals(3, 1, 1), "growth since the base");
    assert_eq!(row.window.blocks, 3 * 3_750);
    assert_eq!(row.window.over, 3);
    assert_eq!(row.window.max_us, 4_000, "the longest");
    assert_eq!(row.window.wake_max_us, 400);
    assert_eq!(row.window.process_max_us, 2_000);
    assert!(!row.attached, "as at the minute's last tick");
    assert_eq!(
        line(&row),
        "2026-11-10T10:07Z,3,1,0,0,0,1,0,0,11250,3,4000,400,2000,0"
    );
    // The next minute starts from the last tick's totals.
    let row = agg
        .tick(T + 120, totals(10, 3, 1), w(0, 0), true)
        .expect("the 10:08 row");
    assert_eq!(row.minute, T + 60);
    assert_eq!(row.grown, totals(0, 0, 0));
    assert!(row.attached);
}

#[test]
fn a_counter_that_goes_back_adds_nothing() {
    let mut agg = MinuteAgg::default();
    agg.tick(T, totals(9, 9, 9), Window::default(), true);
    // Back to 1 (a restarted counter): no growth, no wrap.
    agg.tick(T + 10, totals(1, 1, 1), Window::default(), true);
    // Then it grows from there.
    agg.tick(T + 20, totals(2, 1, 1), Window::default(), true);
    let row = agg
        .tick(T + 60, totals(2, 1, 1), Window::default(), true)
        .unwrap();
    assert_eq!(row.grown, totals(1, 0, 0));
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dasio-minutes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn rows_go_to_their_day_s_file_with_one_header() {
    let d = dir("days");
    let mut log = MinuteLog::new(&d, KEEP_DAYS);
    let row = |minute| Row {
        minute,
        ..Row::default()
    };
    log.write(&row(T)).unwrap();
    log.write(&row(T + 60)).unwrap();
    // 2026-11-11T00:00Z: a new file.
    let midnight = T - T % 86_400 + 86_400;
    log.write(&row(midnight)).unwrap();
    let today = std::fs::read_to_string(d.join("counters-2026-11-10.csv")).unwrap();
    let lines: Vec<&str> = today.lines().collect();
    assert_eq!(lines.len(), 3, "{today}");
    assert_eq!(lines[0], HEADER);
    assert!(lines[1].starts_with("2026-11-10T10:07Z,"));
    assert!(lines[2].starts_with("2026-11-10T10:08Z,"));
    let next = std::fs::read_to_string(d.join("counters-2026-11-11.csv")).unwrap();
    assert_eq!(
        next.lines().collect::<Vec<_>>(),
        [HEADER, &line(&row(midnight))[..]]
    );
    // A log opened again (a restarted device) appends without a new header.
    let mut again = MinuteLog::new(&d, KEEP_DAYS);
    again.write(&row(T + 120)).unwrap();
    let today = std::fs::read_to_string(d.join("counters-2026-11-10.csv")).unwrap();
    assert_eq!(today.lines().filter(|l| *l == HEADER).count(), 1);
    assert_eq!(today.lines().count(), 4);
}

#[test]
fn a_new_day_removes_only_counter_files_older_than_kept() {
    let d = dir("prune");
    for name in [
        "counters-2026-10-10.csv", // 31 days before: goes
        "counters-2026-10-11.csv", // 30 days before: kept
        "counters-2026-11-09.csv",
        "counters-notes.csv", // not a date: kept
        "dasio.log",
    ] {
        std::fs::write(d.join(name), "x").unwrap();
    }
    let mut log = MinuteLog::new(&d, 30);
    log.write(&Row {
        minute: T,
        ..Row::default()
    })
    .unwrap();
    let mut left: Vec<String> = std::fs::read_dir(&d)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            "counters-2026-10-11.csv",
            "counters-2026-11-09.csv",
            "counters-2026-11-10.csv",
            "counters-notes.csv",
            "dasio.log"
        ]
    );
}
