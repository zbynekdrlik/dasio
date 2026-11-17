use super::*;

fn seed(to: f64) -> Seed {
    Seed {
        dante_rate: 96_000,
        driver_rate: 96_000,
        to_asio_ppm: Some(to),
        from_asio_ppm: Some(-7.5),
    }
}

#[test]
fn the_file_is_in_the_instance_folder_named_by_the_driver() {
    let dir = Path::new("instance-1");
    assert_eq!(path(dir, "MiniFuse"), dir.join("asrc-seed-MiniFuse.toml"));
    // Spaces, dots, slashes and other characters never leave the folder.
    assert_eq!(
        path(dir, " Waves SoundGrid/ASIO 1.0 "),
        dir.join("asrc-seed-Waves_SoundGrid_ASIO_1_0.toml")
    );
    assert_eq!(path(dir, "..\\x"), dir.join("asrc-seed-___x.toml"));
    assert_eq!(path(dir, "a-b_c"), dir.join("asrc-seed-a-b_c.toml"));
    assert_eq!(path(dir, "  "), dir.join("asrc-seed-driver.toml"));
    // Another driver, another file.
    assert_ne!(path(dir, "MiniFuse"), path(dir, "SoundGrid"));
}

#[test]
fn the_start_seeds_only_from_a_readable_seed() {
    let s = seed(7.512);
    let st = at_start(&Ok(Some(s)));
    assert_eq!(st.seed, Some(s));
    assert!(!st.warn);
    assert!(
        st.line.contains("to ASIO 7.512 ppm, from ASIO -7.500 ppm"),
        "{}",
        st.line
    );
    let st = at_start(&Ok(None));
    assert_eq!(st.seed, None);
    assert!(!st.warn);
    assert!(st.line.starts_with("no ASRC seed yet"), "{}", st.line);
    let st = at_start(&Err("x: unknown key `y`".into()));
    assert_eq!(st.seed, None);
    assert!(st.warn);
    assert!(st.line.contains("x: unknown key `y`"), "{}", st.line);
}

#[test]
fn a_seed_moves_past_the_tolerance_or_on_a_changed_shape() {
    let a = seed(7.5);
    // The fit's small moves while locked are not written.
    assert!(!moved(&a, &a));
    assert!(!moved(&a, &seed(7.5 + 0.04)));
    assert!(!moved(&a, &seed(7.5 - 0.04)));
    assert!(moved(&a, &seed(7.5 + 0.06)));
    assert!(moved(&a, &seed(7.5 - 0.06)));
    // Exactly the tolerance is not past it.
    assert!(!moved(&seed(0.0), &seed(SEED_TOLERANCE_PPM)));
    let from = |p: Option<f64>| Seed {
        from_asio_ppm: p,
        ..a
    };
    assert!(moved(&a, &from(Some(-7.56))));
    assert!(!moved(&a, &from(Some(-7.46))));
    assert!(moved(&a, &from(None)));
    assert!(moved(&from(None), &a));
    assert!(!moved(&from(None), &from(None)));
    let to_none = Seed {
        to_asio_ppm: None,
        ..a
    };
    assert!(moved(&a, &to_none));
    assert!(moved(&to_none, &a));
    assert!(moved(
        &a,
        &Seed {
            dante_rate: 48_000,
            ..a
        }
    ));
    assert!(moved(
        &a,
        &Seed {
            driver_rate: 48_000,
            ..a
        }
    ));
}

#[test]
fn a_seed_with_one_side_reads_none_for_the_other() {
    let s = Seed {
        from_asio_ppm: None,
        ..seed(1.0)
    };
    assert_eq!(
        describe(&s),
        "Dante 96000 Hz, driver 96000 Hz, to ASIO 1.000 ppm, from ASIO none"
    );
}

#[test]
fn only_a_changed_seed_is_written() {
    let (a, b) = (seed(7.5), seed(7.6));
    // The file's own seed is not written again, nor one within the
    // tolerance; nothing known, nothing.
    let k = Keeper::new(Some(a));
    assert_eq!(k.due(Some(a)), None);
    assert_eq!(k.due(Some(seed(7.51))), None);
    assert_eq!(k.due(None), None);
    assert_eq!(k.due(Some(b)), Some(b));
    // No file: the first seed is written.
    let mut k = Keeper::new(None);
    assert_eq!(k.due(Some(a)), Some(a));
    assert_eq!(k.wrote(a, Ok(())), None);
    assert_eq!(k.due(Some(a)), None);
    assert_eq!(k.due(Some(b)), Some(b));
}

#[test]
fn a_failed_write_is_retried_and_logged_once_per_error() {
    let (a, b) = (seed(7.5), seed(7.6));
    let mut k = Keeper::new(Some(a));
    assert_eq!(k.wrote(b, Err("denied".into())), Some("denied".into()));
    // Still due (the file holds a), the same error not logged again.
    assert_eq!(k.due(Some(b)), Some(b));
    assert_eq!(k.wrote(b, Err("denied".into())), None);
    assert_eq!(k.wrote(b, Err("full".into())), Some("full".into()));
    assert_eq!(k.wrote(b, Ok(())), None);
    assert_eq!(k.due(Some(b)), None);
    // After a success the same error is news again.
    assert_eq!(k.wrote(a, Err("full".into())), Some("full".into()));
}

#[test]
fn the_seed_round_trips_through_dstacks_file() {
    let dir = std::env::temp_dir().join(format!("dasio-asrc-seed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = path(&dir, "Test ASIO");
    let _ = std::fs::remove_file(&p);
    assert_eq!(Seed::read(&p), Ok(None));
    let s = seed(7.512);
    let mut k = Keeper::new(None);
    let due = k.due(Some(s)).unwrap();
    assert_eq!(k.wrote(due, due.write(&p)), None);
    assert_eq!(Seed::read(&p), Ok(Some(s)));
    assert_eq!(at_start(&Seed::read(&p)).seed, Some(s));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The bridge's glue (Windows only, so read as text): the seed read and
/// given before the driver's first open, the save on its own timer in the
/// run loop and once more at the end, never in the driver's side.
#[test]
fn the_bridge_instance_reads_and_keeps_the_seed() {
    // A Windows checkout has CRLF lines.
    let src = include_str!("../run_bridge.rs").replace("\r\n", "\n");
    let once = |needle: &str| {
        let found: Vec<usize> = src.match_indices(needle).map(|(i, _)| i).collect();
        assert_eq!(found.len(), 1, "{needle}: once in run_bridge.rs");
        found[0]
    };
    let path = once("asrc_seed::path(at.dir, &bridge.driver)");
    let helper_start = once("\nfn seed_bridge(");
    let read = once("asrc_seed::at_start(&Seed::read(path))");
    let given = once("b.seed_from(s);");
    assert!(
        helper_start < read && read < given,
        "read, then seed_from, in the helper"
    );
    let call = once("seed_bridge(&mut b, &seed_path)");
    let runner = src.find("spawn_runner(").expect("the runner's start");
    assert!(path < call, "the path, then the helper");
    assert!(call < runner, "seeded before the driver's first open");
    let timer = once("Every::new(asrc_seed::SAVE_EVERY, start, false)");
    let due = once("if save.due(now) {\n            save_seed(&shared, &mut keeper, &seed_path);");
    let end = once(
        "stop_device(&stop, pump, device);\n    // The last lock for the next start.\n    save_seed(&shared, &mut keeper, &seed_path);",
    );
    let loop_start = once("let why = loop {");
    assert!(timer < loop_start && loop_start < due && due < end);
    assert_eq!(src.matches("save_seed(&shared").count(), 2);
    let helper = once("\nfn save_seed(");
    let write = once("s.write(path)");
    assert!(helper < write, "the write is in the helper");
    let callback = src
        .find("let make: MakeProcess")
        .expect("the driver's side");
    assert!(
        !src[callback..].contains("seed"),
        "no seed work in the driver's side"
    );
}
