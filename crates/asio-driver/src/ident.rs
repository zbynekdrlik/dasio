//! The instances' identities: CLSIDs from one base whose last byte is the
//! instance (1 to [`MAX_INSTANCES`]), the driver's name.

use crate::abi::Guid;

/// The most instances one PC registers.
pub const MAX_INSTANCES: u32 = 16;

/// dasio's CLSID base (`{5D4A7C21-9B3E-4F6A-8C11-2E475A90D100}`); the
/// last byte is the instance.
pub const CLSID_BASE: Guid = Guid {
    data1: 0x5d4a_7c21,
    data2: 0x9b3e,
    data3: 0x4f6a,
    data4: [0x8c, 0x11, 0x2e, 0x47, 0x5a, 0x90, 0xd1, 0x00],
};

/// Instance `n`'s CLSID; `None` outside 1 to [`MAX_INSTANCES`].
pub fn clsid(n: u32) -> Option<Guid> {
    (1..=MAX_INSTANCES).contains(&n).then(|| {
        let mut g = CLSID_BASE;
        g.data4[7] = n as u8;
        g
    })
}

/// The instance a CLSID names; `None` for any other CLSID.
pub fn instance_of(g: &Guid) -> Option<u32> {
    let n = u32::from(g.data4[7]);
    (clsid(n).as_ref() == Some(g)).then_some(n)
}

/// The registry form, `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`.
pub fn braced(g: &Guid) -> String {
    let d = g.data4;
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1, g.data2, g.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}

/// The driver's name for instance `n` (at most 31 bytes: ASIO's 32 with its NUL).
pub fn driver_name(n: u32) -> String {
    format!("dasio {n}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_driver_name_carries_the_instance_and_fits_asio_s_32_bytes() {
        assert_eq!(driver_name(3), "dasio 3");
        assert!(driver_name(MAX_INSTANCES).len() <= 31);
    }

    #[test]
    fn each_instance_has_its_own_clsid_and_is_found_from_it() {
        assert_eq!(clsid(0), None);
        assert_eq!(clsid(MAX_INSTANCES + 1), None);
        let one = clsid(1).unwrap();
        let two = clsid(2).unwrap();
        assert_ne!(one, two);
        assert_eq!(instance_of(&one), Some(1));
        assert_eq!(instance_of(&two), Some(2));
        assert_eq!(
            instance_of(&CLSID_BASE),
            None,
            "instance 0 is not registered"
        );
        let mut other = one;
        other.data1 ^= 1;
        assert_eq!(instance_of(&other), None);
    }

    #[test]
    fn the_registry_form_is_braced_uppercase() {
        assert_eq!(
            braced(&clsid(2).unwrap()),
            "{5D4A7C21-9B3E-4F6A-8C11-2E475A90D102}"
        );
    }
}
