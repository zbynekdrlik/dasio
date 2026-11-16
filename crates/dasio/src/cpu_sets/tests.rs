//! dasio's CPUs as Windows CPU sets (dasio#120).

use super::*;

/// One `SYSTEM_CPU_SET_INFORMATION` entry as Windows writes it (32
/// bytes, winnt.h): CPU set `id` on `cpu` of `group` with `flags`.
fn entry(id: u32, group: u16, cpu: u8, flags: u8) -> Vec<u8> {
    let mut e = vec![0u8; 32];
    e[0..4].copy_from_slice(&32u32.to_le_bytes());
    e[4..8].copy_from_slice(&0u32.to_le_bytes());
    e[8..12].copy_from_slice(&id.to_le_bytes());
    e[12..14].copy_from_slice(&group.to_le_bytes());
    e[14] = cpu;
    e[15] = cpu; // CoreIndex
    e[19] = flags;
    e[24..32].copy_from_slice(&0xDEAD_BEEF_u64.to_le_bytes()); // AllocationTag
    e
}

/// A PC of `cpus` CPUs whose CPUs in `reserved` are reserved; ids start
/// at 0x100, as on FOH's Ryzen.
fn pc(cpus: u8, reserved: u64) -> Vec<u8> {
    (0..cpus)
        .flat_map(|c| {
            let flags = if reserved & 1 << c != 0 { ALLOCATED } else { 0 };
            entry(0x100 + u32::from(c), 0, c, flags)
        })
        .collect()
}

#[test]
fn parse_reads_each_entry_s_id_cpu_and_flags() {
    let mut buf = entry(0x100, 0, 0, 0);
    buf.extend(entry(0x101, 0, 1, ALLOCATED));
    buf.extend(entry(0x102, 0, 2, PARKED | ALLOCATED | 0b0100));
    buf.extend(entry(0x200, 1, 0, ALLOCATED));
    let sets = parse(&buf).unwrap();
    assert_eq!(
        sets,
        [
            CpuSet {
                id: 0x100,
                group: 0,
                cpu: 0,
                allocated: false,
                parked: false
            },
            CpuSet {
                id: 0x101,
                group: 0,
                cpu: 1,
                allocated: true,
                parked: false
            },
            CpuSet {
                id: 0x102,
                group: 0,
                cpu: 2,
                allocated: true,
                parked: true
            },
            CpuSet {
                id: 0x200,
                group: 1,
                cpu: 0,
                allocated: true,
                parked: false
            },
        ]
    );
    assert_eq!(parse(&[]).unwrap(), Vec::<CpuSet>::new());
}

#[test]
fn parse_walks_by_size_and_skips_other_types() {
    // A larger (future) entry: the next one starts after its Size.
    let mut big = entry(0x100, 0, 0, ALLOCATED);
    big[0..4].copy_from_slice(&40u32.to_le_bytes());
    big.extend([0xAA; 8]);
    // An entry of an unknown type: skipped, not misread.
    let mut other = entry(0x999, 0, 5, ALLOCATED);
    other[4..8].copy_from_slice(&7u32.to_le_bytes());
    let mut buf = big;
    buf.extend(other);
    buf.extend(entry(0x101, 0, 1, 0));
    let ids: Vec<u32> = parse(&buf).unwrap().iter().map(|s| s.id).collect();
    assert_eq!(ids, [0x100, 0x101]);
}

#[test]
fn parse_refuses_a_cut_or_broken_buffer() {
    let one = entry(0x100, 0, 0, 0);
    assert!(
        parse(&one[..31])
            .unwrap_err()
            .contains("size 32 (31 bytes left)")
    );
    assert!(parse(&one[..5]).unwrap_err().contains("cut short"));
    let mut zero = one.clone();
    zero[0..4].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(
        parse(&zero).unwrap_err(),
        "a CPU set entry at byte 0 has size 0 (32 bytes left)",
        "never loops forever"
    );
    let mut small = one;
    small[0..4].copy_from_slice(&12u32.to_le_bytes());
    assert!(parse(&small[..12]).unwrap_err().contains("size 12"));
}

#[test]
fn the_reserved_mask_is_group_0_s_allocated_cpus() {
    let sets = parse(&pc(4, 0x6)).unwrap();
    assert_eq!(reserved_mask(&sets), 0x6);
    let mut buf = pc(4, 0);
    buf.extend(entry(0x200, 1, 2, ALLOCATED));
    assert_eq!(
        reserved_mask(&parse(&buf).unwrap()),
        0,
        "group 1 is not in the mask"
    );
}

#[test]
fn the_ids_are_matched_by_logical_processor_not_by_position() {
    // Windows lists sets in its own order with its own ids.
    let mut buf = entry(0x10C, 0, 3, 0);
    buf.extend(entry(0x10A, 0, 1, 0));
    buf.extend(entry(0x10B, 0, 2, 0));
    buf.extend(entry(0x209, 1, 2, 0));
    let sets = parse(&buf).unwrap();
    assert_eq!(ids_for(0x6, &sets), [0x10A, 0x10B]);
    assert_eq!(ids_for(0x8, &sets), [0x10C]);
    assert_eq!(ids_for(0, &sets), Vec::<u32>::new());
}

#[test]
fn an_instance_whose_cpus_are_reserved_takes_them() {
    // FOH: 32 CPUs, 2 instances, the baseline's 0xFF0 reserved.
    let foh = Layout::of(32, 2);
    let sets = parse(&pc(32, foh.dasio_cpus_mask())).unwrap();
    assert_eq!(
        decide(&sets, &foh, 1, false),
        Decision::Assign {
            ids: vec![0x104, 0x105, 0x106, 0x107],
            cpus: 0xF0
        }
    );
    assert_eq!(
        decide(&sets, &foh, 2, false),
        Decision::Assign {
            ids: vec![0x108, 0x109, 0x10A, 0x10B],
            cpus: 0xF00
        }
    );
    // FOH's own 4-31: still only the instance's CPUs, never the DAW's.
    let sets = parse(&pc(32, 0xFFFF_FFF0)).unwrap();
    assert_eq!(
        decide(&sets, &foh, 2, false),
        Decision::Assign {
            ids: vec![0x108, 0x109, 0x10A, 0x10B],
            cpus: 0xF00
        }
    );
}

#[test]
fn a_bridge_also_takes_the_daw_cpus_the_pc_reserves() {
    let dsg = Layout::of(4, 1);
    // The baseline's 1-2: the bridge takes 1-2.
    let base = parse(&pc(4, 0x6)).unwrap();
    assert_eq!(
        decide(&base, &dsg, 1, true),
        Decision::Assign {
            ids: vec![0x101, 0x102],
            cpus: 0x6
        }
    );
    // dsg's own 1-3 (Lasso's 0xE): the bridge takes 1-3; a DAW kind
    // does not.
    let site = parse(&pc(4, 0xE)).unwrap();
    assert_eq!(
        decide(&site, &dsg, 1, true),
        Decision::Assign {
            ids: vec![0x101, 0x102, 0x103],
            cpus: 0xE
        }
    );
    assert_eq!(
        decide(&site, &dsg, 1, false),
        Decision::Assign {
            ids: vec![0x101, 0x102],
            cpus: 0x6
        }
    );
}

#[test]
fn cpus_not_reserved_set_nothing_and_name_the_missing_ones() {
    let dsg = Layout::of(4, 1);
    // No ReservedCpuSets (or no reboot since apply): today's dsg.
    let none = parse(&pc(4, 0)).unwrap();
    assert_eq!(
        decide(&none, &dsg, 1, true),
        Decision::NotReserved {
            own: 0x6,
            missing: 0x6
        }
    );
    // Half reserved: still nothing, never a part of the instance's CPUs.
    let half = parse(&pc(4, 0x2)).unwrap();
    assert_eq!(
        decide(&half, &dsg, 1, false),
        Decision::NotReserved {
            own: 0x6,
            missing: 0x4
        }
    );
    // No CPU set information at all.
    assert_eq!(
        decide(&[], &dsg, 1, false),
        Decision::NotReserved {
            own: 0x6,
            missing: 0x6
        }
    );
}

#[test]
fn an_instance_the_layout_has_no_cpus_for_is_named() {
    let mbc = Layout::of(12, 1);
    let sets = parse(&pc(12, 0xC)).unwrap();
    assert_eq!(
        decide(&sets, &mbc, 2, false),
        Decision::NoLayout("dasio:2: the profile has instances = 1 (1 to 1)".into())
    );
    assert_eq!(
        decide(&sets, &mbc, 1, false),
        Decision::Assign {
            ids: vec![0x102, 0x103],
            cpus: 0xC
        }
    );
}

#[test]
fn the_helper_threads_stay_within_the_process_s_reserved_cpus() {
    // FOH instance 1 (4-7, media pinned on 4-5) with no Lasso affinity:
    // the placer's helpers would be every CPU but 4-5, instance 2's
    // reserved 8-11 included; within its own reserved CPUs, 6-7 only.
    let foh = Layout::of(32, 2);
    let own = 0x30u64;
    let helpers = 0xFFFF_FFFF & !own;
    let assigned = foh.dasio_mask(1).unwrap();
    assert_eq!(helpers_within(helpers, assigned), 0xC0);
    assert_eq!(
        helpers_within(helpers, assigned) & foh.dasio_mask(2).unwrap(),
        0
    );
    // Nothing assigned (CPUs not reserved): as before.
    assert_eq!(helpers_within(helpers, 0), helpers);
    // Every assigned CPU is a media CPU: as before, never no CPU.
    assert_eq!(helpers_within(0xC0, 0x30), 0xC0);
    // Nothing pinned: no helpers to place.
    assert_eq!(helpers_within(0, assigned), 0);
}

#[test]
fn the_smallest_entries_parse() {
    // An entry of another type takes 8 bytes at least, a CPU set 20 (up
    // to its flags): both are read to the buffer's last byte.
    let mut tiny_other = vec![0u8; 8];
    tiny_other[0..4].copy_from_slice(&8u32.to_le_bytes());
    tiny_other[4..8].copy_from_slice(&3u32.to_le_bytes());
    let mut short_set = entry(0x105, 0, 5, ALLOCATED)[..20].to_vec();
    short_set[0..4].copy_from_slice(&20u32.to_le_bytes());
    let mut buf = short_set.clone();
    buf.extend(&tiny_other);
    let sets = parse(&buf).unwrap();
    assert_eq!(
        sets,
        [CpuSet {
            id: 0x105,
            group: 0,
            cpu: 5,
            allocated: true,
            parked: false
        }]
    );
    let mut buf = tiny_other;
    buf.extend(&short_set);
    assert_eq!(parse(&buf).unwrap(), sets);
}

#[test]
fn a_cpu_beyond_the_mask_s_64_is_left_out() {
    let mut buf = pc(2, 0x3);
    buf.extend(entry(0x140, 0, 64, ALLOCATED));
    buf.extend(entry(0x1FF, 0, 255, ALLOCATED));
    let sets = parse(&buf).unwrap();
    assert_eq!(sets.len(), 4);
    assert_eq!(reserved_mask(&sets), 0x3);
}

#[test]
fn the_start_s_cpus_are_kept_for_the_helper_placer() {
    assert_eq!(assigned(), 0, "nothing assigned before the start");
    set_assigned(0x6);
    assert_eq!(assigned(), 0x6);
    set_assigned(0);
    assert_eq!(assigned(), 0);
}

#[test]
fn an_entry_past_the_end_is_refused_wherever_it_starts() {
    let mut buf = entry(0x100, 0, 0, 0);
    let mut long = entry(0x101, 0, 1, 0);
    long[0..4].copy_from_slice(&40u32.to_le_bytes());
    buf.extend(long);
    assert_eq!(
        parse(&buf).unwrap_err(),
        "a CPU set entry at byte 32 has size 40 (32 bytes left)"
    );
}

#[test]
fn two_sets_on_one_cpu_keep_it_reserved() {
    // Microsoft: "on most systems" a CPU set maps to one home processor,
    // not on all; a CPU with two reserved sets stays in the mask.
    let mut buf = pc(4, 0x6);
    buf.extend(entry(0x110, 0, 2, ALLOCATED));
    let sets = parse(&buf).unwrap();
    assert_eq!(reserved_mask(&sets), 0x6);
    assert_eq!(ids_for(0x4, &sets), [0x102, 0x110]);
}

#[test]
fn a_cut_entry_after_a_whole_one_is_named() {
    let mut buf = entry(0x100, 0, 0, 0);
    buf.extend([32, 0, 0, 0, 0]);
    assert_eq!(
        parse(&buf).unwrap_err(),
        "a CPU set entry at byte 32 is cut short"
    );
}
