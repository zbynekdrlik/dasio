use super::*;

/// A small device: 2 RX, 3 TX at 96 kHz, block 64 (no site value).
const DEVICE: &str = r#"
name = "test-bridge"
manufacturer = "iemmixer"
model = "iem-dante"
sample_rate = 96000
encoding = 24
latency_ns = 1000000
block = 64
state_file = "/var/lib/iem-dante/state.toml"
rx = ["RX 1", "RX 2"]
tx = ["TX 1", "TX 2", "TX 3"]
"#;

fn device() -> DeviceConfig {
    DeviceConfig::from_toml(DEVICE).expect("the test device is valid")
}

fn with_bridge(table: &str) -> String {
    format!("{DEVICE}\n[bridge]\n{table}\n")
}

const FULL: &str = r#"
driver = "Test ASIO"
to_asio = [3, 4]
from_asio = [1, 2, 5]
app_buffer_ns = 5000000
"#;

fn bridge_of(table: &str) -> BridgeConfig {
    match kind(&with_bridge(table)).expect("the bridge table reads") {
        Kind::Bridge(b) => b,
        Kind::Daw => panic!("a [bridge] table read as the DAW kind"),
    }
}

fn refusal(table: &str) -> String {
    kind(&with_bridge(table)).expect_err("the bridge table is refused")
}

#[test]
fn a_config_without_a_bridge_table_is_the_daw_kind() {
    assert_eq!(kind(DEVICE), Ok(Kind::Daw));
}

#[test]
fn the_bridge_table_reads_as_written() {
    assert_eq!(
        bridge_of(FULL),
        BridgeConfig {
            driver: "Test ASIO".into(),
            to_asio: vec![3, 4],
            from_asio: vec![1, 2, 5],
            app_buffer_ns: Some(5_000_000),
            to_asio_app_buffer_ns: None,
            from_asio_app_buffer_ns: None,
            converter_taps: None,
        }
    );
}

#[test]
fn app_buffer_ns_is_optional_and_a_missing_list_is_empty() {
    let b = bridge_of("driver = \"Test ASIO\"\nto_asio = [1]");
    assert_eq!(b.app_buffer_ns, None);
    assert_eq!(b.from_asio, Vec::<u32>::new());
}

#[test]
fn the_spec_is_the_devices_rate_block_and_counts() {
    assert_eq!(
        bridge_of(FULL).spec(&device()),
        Ok(Spec {
            dante_rate: 96_000,
            block_frames: 64,
            to_asio: 2,
            from_asio: 3,
            app_buffer_ns: Some(5_000_000),
            to_asio_app_buffer_ns: None,
            from_asio_app_buffer_ns: None,
            converter_taps: None,
        })
    );
}

/// dsg's shape (dasio#120): to-ASIO at its own target, from-ASIO at the
/// shared default.
const PER_DIRECTION: &str = r#"
driver = "Test ASIO"
to_asio = [3, 4]
from_asio = [1, 2, 5]
to_asio_app_buffer_ns = 1333333
"#;

#[test]
fn each_direction_takes_its_own_target_into_the_spec() {
    let b = bridge_of(PER_DIRECTION);
    assert_eq!(b.app_buffer_ns, None);
    assert_eq!(b.to_asio_app_buffer_ns, Some(1_333_333));
    assert_eq!(b.from_asio_app_buffer_ns, None);
    let spec = b.spec(&device()).unwrap();
    assert_eq!(spec.app_buffer_ns, None);
    assert_eq!(spec.to_asio_app_buffer_ns, Some(1_333_333));
    assert_eq!(spec.from_asio_app_buffer_ns, None);
    let b = bridge_of(&format!("{FULL}from_asio_app_buffer_ns = 2000000\n"));
    let spec = b.spec(&device()).unwrap();
    assert_eq!(spec.app_buffer_ns, Some(5_000_000));
    assert_eq!(spec.to_asio_app_buffer_ns, None);
    assert_eq!(spec.from_asio_app_buffer_ns, Some(2_000_000));
    // Below dlink's 1 ms (dstack issue 422): a direction's own target
    // reaches dstack's spec as written, never raised to 1 ms here.
    let b = bridge_of(&format!(
        "{FULL}to_asio_app_buffer_ns = 500000\nfrom_asio_app_buffer_ns = 750000\n"
    ));
    let spec = b.spec(&device()).unwrap();
    assert_eq!(spec.to_asio_app_buffer_ns, Some(500_000));
    assert_eq!(spec.from_asio_app_buffer_ns, Some(750_000));
}

#[test]
fn a_directions_target_is_held_to_the_bridges_range_under_its_own_name() {
    // dstack issue 422 (v0.6.185): a direction's own target goes down to
    // 0.5 ms (dlink's 1 ms floor is a dlink app's; dasio#190).
    for key in ["to_asio_app_buffer_ns", "from_asio_app_buffer_ns"] {
        for ok in [500_000_u64, 750_000, 999_999, 1_000_000, 150_000_000] {
            let b = bridge_of(&format!("driver = \"Test ASIO\"\n{key} = {ok}"));
            let got = match key {
                "to_asio_app_buffer_ns" => b.to_asio_app_buffer_ns,
                _ => b.from_asio_app_buffer_ns,
            };
            assert_eq!(got, Some(ok), "{key}");
        }
        for bad in ["499999", "150000001", "0"] {
            let e = refusal(&format!("driver = \"Test ASIO\"\n{key} = {bad}"));
            assert!(e.starts_with(&format!("[bridge] {key}: ")), "{bad}: {e}");
            assert!(e.ends_with("; 0.5 to 150 ms"), "{bad}: {e}");
        }
        assert_eq!(
            refusal(&format!("driver = \"Test ASIO\"\n{key} = 499999")),
            format!("[bridge] {key}: 499999 ns (0.499999 ms); 0.5 to 150 ms")
        );
        let e = refusal(&format!("driver = \"Test ASIO\"\n{key} = -5"));
        assert_eq!(e, format!("[bridge] `{key}` = -5: a negative time"));
        let e = refusal(&format!("driver = \"Test ASIO\"\n{key} = 1.5"));
        assert_eq!(e, format!("[bridge] `{key}` must be a whole number of ns"));
    }
}

#[test]
fn the_description_names_a_directions_own_target() {
    assert_eq!(
        bridge_of(PER_DIRECTION).describe(),
        "bridge to \"Test ASIO\": RX -> outputs [3,4], inputs [1,2,5] -> TX, app buffer default, \
         to ASIO 1333333 ns"
    );
    assert_eq!(
        bridge_of(&format!(
            "{FULL}to_asio_app_buffer_ns = 1333333\nfrom_asio_app_buffer_ns = 2000000\n"
        ))
        .describe(),
        "bridge to \"Test ASIO\": RX -> outputs [3,4], inputs [1,2,5] -> TX, app buffer 5000000 ns, \
         to ASIO 1333333 ns, from ASIO 2000000 ns"
    );
}

#[test]
fn the_channels_are_zero_based_inputs_exact() {
    assert_eq!(
        bridge_of(FULL).channels(),
        Channels {
            inputs: Inputs::Exact(vec![0, 1, 4]),
            outputs: vec![2, 3],
        }
    );
}

#[test]
fn every_rx_channel_needs_its_driver_output() {
    let b = bridge_of("driver = \"Test ASIO\"\nto_asio = [1]\nfrom_asio = [1, 2, 3]");
    let e = b.spec(&device()).unwrap_err();
    assert!(
        e.contains("`to_asio` names 1 driver channels for the device's 2 RX channels"),
        "{e}"
    );
}

#[test]
fn every_tx_channel_needs_its_driver_input() {
    let b = bridge_of("driver = \"Test ASIO\"\nto_asio = [1, 2]\nfrom_asio = [1, 2, 3, 4]");
    let e = b.spec(&device()).unwrap_err();
    assert!(
        e.contains("`from_asio` names 4 driver channels for the device's 3 TX channels"),
        "{e}"
    );
}

#[test]
fn one_direction_alone_is_a_bridge() {
    let mut cfg = device();
    cfg.rx.clear();
    let b = bridge_of("driver = \"Test ASIO\"\nfrom_asio = [1, 2, 3]");
    let spec = b.spec(&cfg).expect("ASIO -> Dante alone is a bridge");
    assert_eq!((spec.to_asio, spec.from_asio), (0, 3));
    let mut cfg = device();
    cfg.tx.clear();
    let b = bridge_of("driver = \"Test ASIO\"\nto_asio = [1, 2]");
    let spec = b.spec(&cfg).expect("Dante -> ASIO alone is a bridge");
    assert_eq!((spec.to_asio, spec.from_asio), (2, 0));
}

#[test]
fn a_bridge_with_no_channel_is_refused() {
    let mut cfg = device();
    cfg.rx.clear();
    cfg.tx.clear();
    let e = bridge_of("driver = \"Test ASIO\"").spec(&cfg).unwrap_err();
    assert_eq!(e, "[bridge] carries no channel");
}

#[test]
fn a_block_beyond_u32_is_refused() {
    let mut cfg = device();
    cfg.block = u64::from(u32::MAX) + 1;
    let e = bridge_of(FULL).spec(&cfg).unwrap_err();
    assert!(e.contains("too large"), "{e}");
    cfg.block = u64::from(u32::MAX);
    assert_eq!(bridge_of(FULL).spec(&cfg).unwrap().block_frames, u32::MAX);
}

#[test]
fn the_driver_is_required_and_not_blank() {
    for table in ["to_asio = [1, 2]", "driver = \"  \"", "driver = 3"] {
        assert!(refusal(table).contains("needs `driver`"), "{table}");
    }
}

#[test]
fn an_unknown_key_is_refused_by_name() {
    let e = refusal("driver = \"Test ASIO\"\nto_asoi = [1, 2]");
    assert!(e.contains("unknown key `to_asoi`"), "{e}");
}

#[test]
fn a_bridge_that_is_not_a_table_is_refused() {
    let e = kind(&format!("bridge = \"Test ASIO\"\n{DEVICE}")).unwrap_err();
    assert_eq!(e, "`bridge` in the device config must be a table");
}

#[test]
fn a_config_that_is_not_toml_is_refused() {
    assert!(
        kind("name = ")
            .unwrap_err()
            .starts_with("the config as a TOML table")
    );
}

#[test]
fn a_channel_list_must_be_a_list() {
    let e = refusal("driver = \"Test ASIO\"\nto_asio = 3");
    assert!(
        e.contains("`to_asio` must be a list of driver output numbers"),
        "{e}"
    );
    let e = refusal("driver = \"Test ASIO\"\nfrom_asio = \"1\"");
    assert!(
        e.contains("`from_asio` must be a list of driver input numbers"),
        "{e}"
    );
}

#[test]
fn a_channel_is_one_based_and_whole() {
    for bad in ["0", "-1", "\"1\"", "1.5", "4294967296"] {
        let e = refusal(&format!("driver = \"Test ASIO\"\nto_asio = [1, {bad}]"));
        assert!(
            e.contains("is not a driver output number (1-based)"),
            "{bad}: {e}"
        );
    }
    let b = bridge_of("driver = \"Test ASIO\"\nto_asio = [4294967295]");
    assert_eq!(b.to_asio, vec![u32::MAX]);
}

#[test]
fn a_channel_given_twice_is_refused() {
    let e = refusal("driver = \"Test ASIO\"\nfrom_asio = [2, 1, 2]");
    assert!(e.contains("driver input 2 is given twice"), "{e}");
}

#[test]
fn app_buffer_ns_is_held_to_dlinks_range() {
    for ok in [1_000_000_u64, 150_000_000] {
        let b = bridge_of(&format!("driver = \"Test ASIO\"\napp_buffer_ns = {ok}"));
        assert_eq!(b.app_buffer_ns, Some(ok));
    }
    for bad in ["999999", "150000001", "0"] {
        let e = refusal(&format!("driver = \"Test ASIO\"\napp_buffer_ns = {bad}"));
        assert!(e.starts_with("[bridge] app_buffer_ns: "), "{bad}: {e}");
        assert!(e.contains("1 to 150 ms"), "{bad}: {e}");
    }
    let e = refusal("driver = \"Test ASIO\"\napp_buffer_ns = -5000000");
    assert_eq!(e, "[bridge] `app_buffer_ns` = -5000000: a negative time");
    let e = refusal("driver = \"Test ASIO\"\napp_buffer_ns = \"5ms\"");
    assert!(e.contains("must be a whole number of ns"), "{e}");
}

#[test]
fn the_description_names_the_driver_and_both_maps() {
    assert_eq!(
        bridge_of(FULL).describe(),
        "bridge to \"Test ASIO\": RX -> outputs [3,4], inputs [1,2,5] -> TX, app buffer 5000000 ns"
    );
    assert_eq!(
        bridge_of("driver = \"Test ASIO\"\nto_asio = [1]").describe(),
        "bridge to \"Test ASIO\": RX -> outputs [1], inputs [] -> TX, app buffer default"
    );
}

/// dsg's converter (dasio#120, the owner's decision of 17.11.2026 on
/// dstack issue 382): both directions' sinc length into the spec.
#[test]
fn the_converter_length_goes_into_the_spec_and_the_description() {
    let b = bridge_of(&format!("{PER_DIRECTION}converter_taps = 48\n"));
    assert_eq!(b.converter_taps, Some(48));
    assert_eq!(b.spec(&device()).unwrap().converter_taps, Some(48));
    assert_eq!(
        b.describe(),
        "bridge to \"Test ASIO\": RX -> outputs [3,4], inputs [1,2,5] -> TX, app buffer default, \
         to ASIO 1333333 ns, converter 48 taps"
    );
    // Unset: dstack's default (256 taps), and the line names none.
    let b = bridge_of(FULL);
    assert_eq!(b.converter_taps, None);
    assert_eq!(b.spec(&device()).unwrap().converter_taps, None);
    assert!(!b.describe().contains("converter"), "{}", b.describe());
}

/// What dstack-asio's `Bridge::new` takes: 32 to 256 taps in steps of 8,
/// each edge and step checked on its own.
#[test]
fn the_converter_length_is_held_to_32_to_256_in_steps_of_8() {
    for ok in [32_u32, 40, 48, 128, 248, 256] {
        let b = bridge_of(&format!("{FULL}converter_taps = {ok}\n"));
        assert_eq!(b.converter_taps, Some(ok), "{ok}");
        assert_eq!(b.spec(&device()).unwrap().converter_taps, Some(ok));
    }
    for bad in [
        "0",
        "8",
        "24",
        "31",
        "33",
        "36",
        "47",
        "252",
        "257",
        "264",
        "-8",
        "4294967328",
    ] {
        assert_eq!(
            refusal(&format!("driver = \"Test ASIO\"\nconverter_taps = {bad}")),
            format!("[bridge] `converter_taps` = {bad}: 32 to 256 in steps of 8"),
            "{bad}"
        );
    }
    for bad in ["48.0", "\"48\"", "[48]"] {
        assert_eq!(
            refusal(&format!("driver = \"Test ASIO\"\nconverter_taps = {bad}")),
            "[bridge] `converter_taps` must be a whole number of taps",
            "{bad}"
        );
    }
}

/// dstack-asio takes every length dasio lets through, and dasio refuses
/// every one dstack-asio refuses (its own check, `Bridge::new`).
#[test]
fn dasio_s_converter_check_agrees_with_dstack_asio_s() {
    for taps in 0..=300_u32 {
        let spec = Spec {
            converter_taps: Some(taps),
            ..bridge_of(FULL).spec(&device()).unwrap()
        };
        let dstack_takes = dstack_asio::bridge::Bridge::new(spec).is_ok();
        let dasio_takes = kind(&with_bridge(&format!(
            "driver = \"Test ASIO\"\nconverter_taps = {taps}"
        )))
        .is_ok();
        assert_eq!(dasio_takes, dstack_takes, "{taps} taps");
    }
}
