//! `[[nic_driver]]` entries (dasio#128): the measured best driver package
//! for a family of Dante NICs, named by the NIC's hardware (its PnP
//! hardware IDs), not by the driver bound to it now. The check reports a
//! Dante NIC of that family running another driver (its version or
//! provider), a WARN and the tray's count, never must-have; `install.ps1
//! -NicDriver <folder>` installs the pinned package only after
//! [`command`] found the folder to be exactly that package (every file's
//! SHA256, the catalog's Authenticode signer).
//!
//! The portable part (validation, matching, the check, the package
//! decision) is here; reading the NIC's hardware IDs is
//! [`windows`](super::windows), hashing and the signature are read by
//! install.ps1 and handed to `dasio pc-profile nic-driver`.

use serde::Deserialize;

use super::{Problem, Reader, Value};

/// A NIC family's pinned driver package.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NicDriverEntry {
    /// The family's name (a site entry with the same name wins).
    pub family: String,
    /// Prefixes of the Dante NIC's hardware IDs, e.g.
    /// `PCI\VEN_10EC&DEV_8125` (case ignored, matched up to an `&` or `\`
    /// or the end).
    pub hardware_ids: Vec<String>,
    /// The driver version Windows shows (the class key's `DriverVersion`).
    pub version: String,
    /// The driver's provider (the class key's `ProviderName`).
    pub provider: String,
    /// The package's INF, installed with `pnputil /add-driver`.
    pub inf: String,
    /// The CN of the Authenticode signer of the package's catalog.
    pub signer: String,
    /// Every file of the package, each with its SHA256.
    pub files: Vec<PackageFile>,
    /// Where the package comes from.
    pub source: String,
    #[serde(default)]
    pub why: String,
}

/// One file of a pinned package.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PackageFile {
    /// Its name in the package's folder (no folder part).
    pub name: String,
    /// Its SHA256 in hex (case ignored).
    pub sha256: String,
}

/// What an entry must hold; the error names the entry and the field.
pub fn validate(e: &NicDriverEntry) -> Result<(), String> {
    let fail = |what: &str| Err(format!("nic_driver {:?}: {what}", e.family));
    if e.family.trim().is_empty() {
        return fail("family must not be empty");
    }
    if e.hardware_ids.is_empty() {
        return fail("give at least one hardware_ids prefix");
    }
    for id in &e.hardware_ids {
        let up = id.to_ascii_uppercase();
        if !(up.starts_with(r"PCI\VEN_") || up.starts_with(r"USB\VID_")) {
            return fail(&format!(
                r"hardware id {id:?} is not a PCI\VEN_... or USB\VID_... prefix"
            ));
        }
    }
    for (field, v) in [
        ("version", &e.version),
        ("provider", &e.provider),
        ("signer", &e.signer),
        ("source", &e.source),
    ] {
        if v.trim().is_empty() {
            return fail(&format!("{field} must not be empty"));
        }
    }
    for (i, f) in e.files.iter().enumerate() {
        if f.name.is_empty() || f.name.contains(['\\', '/', ':']) || f.name == ".." {
            return fail(&format!(
                "file {:?}: give the name alone, no folder",
                f.name
            ));
        }
        if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return fail(&format!("file {}: sha256 is not 64 hex digits", f.name));
        }
        if e.files[..i]
            .iter()
            .any(|g| g.name.eq_ignore_ascii_case(&f.name))
        {
            return fail(&format!("file {} is named twice", f.name));
        }
    }
    if !e.files.iter().any(|f| f.name.eq_ignore_ascii_case(&e.inf)) {
        return fail(&format!("inf {} is not one of its files", e.inf));
    }
    if !e.inf.to_ascii_lowercase().ends_with(".inf") {
        return fail(&format!("inf {} is not an .inf file", e.inf));
    }
    let cats = e.files.iter().filter(|f| is_catalog(&f.name)).count();
    if cats != 1 {
        return fail(&format!("give exactly one .cat file, not {cats}"));
    }
    Ok(())
}

/// Whether `name` is a catalog (`.cat`), the file whose signature covers
/// the package.
pub fn is_catalog(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".cat")
}

/// Whether hardware ID `id` falls under `prefix`: the same text up to its
/// length (case ignored), then the end or an `&` / `\`, so
/// `PCI\VEN_10EC&DEV_8125` takes `...&DEV_8125&SUBSYS_...` but not
/// `...&DEV_81250`.
pub fn id_matches(prefix: &str, id: &str) -> bool {
    let Some(head) = id.get(..prefix.len()) else {
        return false;
    };
    head.eq_ignore_ascii_case(prefix)
        && matches!(id[prefix.len()..].chars().next(), None | Some('&' | '\\'))
}

/// The entry for a NIC with hardware IDs `ids`: the first whose prefix
/// takes one of them.
pub fn pick<'a>(entries: &'a [NicDriverEntry], ids: &[String]) -> Option<&'a NicDriverEntry> {
    entries.iter().find(|e| {
        e.hardware_ids
            .iter()
            .any(|p| ids.iter().any(|id| id_matches(p, id)))
    })
}

/// A string value as the check shows it.
fn read_text(r: &dyn Reader, name: &str) -> String {
    match r.reg("{nic_class}", name) {
        Ok(Some(Value::String(s))) => s,
        Ok(Some(other)) => format!("{other:?}"),
        Ok(None) => "absent".into(),
        Err(e) => format!("unreadable ({e})"),
    }
}

/// The Dante NIC's driver against its family's entry: one problem (WARN,
/// never must-have) when its version or provider is not the pinned one;
/// nothing for a NIC of no listed family or one not found (the NIC's own
/// problem says that).
pub fn check(entries: &[NicDriverEntry], r: &dyn Reader) -> Vec<Problem> {
    let ids = r.nic_hardware_ids();
    let Some(e) = pick(entries, &ids) else {
        return Vec::new();
    };
    let version = read_text(r, "DriverVersion");
    let provider = read_text(r, "ProviderName");
    if version == e.version && provider == e.provider {
        return Vec::new();
    }
    vec![Problem {
        what: format!("the Dante NIC's driver ({})", e.family),
        found: format!("{version} from {provider}"),
        want: format!("{} from {} ({})", e.version, e.provider, e.inf),
        why: format!(
            "{}; install it with install.ps1 -NicDriver <folder> (someone on site): {}",
            e.why, e.source
        ),
        must: false,
    }]
}

/// A driver package as install.ps1 read it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Package {
    /// Every file of the folder: name and SHA256.
    pub files: Vec<(String, String)>,
    /// The catalog's Authenticode status (`Valid`, `NotSigned`, ...).
    pub signature: String,
    /// The catalog's signer subject (`CN=..., O=...`).
    pub signer: String,
}

/// The CN of a certificate subject (`CN=Microsoft ..., O=...`); `None`
/// without one.
pub fn common_name(subject: &str) -> Option<&str> {
    subject
        .split(',')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("CN="))
}

/// Why `pkg` is not `e`'s package: every reason, empty when it is.
pub fn verify(e: &NicDriverEntry, pkg: &Package) -> Vec<String> {
    let mut out = Vec::new();
    for f in &e.files {
        match pkg
            .files
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(&f.name))
        {
            None => out.push(format!("{}: missing", f.name)),
            Some((_, got)) if !got.eq_ignore_ascii_case(&f.sha256) => out.push(format!(
                "{}: SHA256 {got}, the baseline pins {}",
                f.name, f.sha256
            )),
            Some(_) => {}
        }
    }
    for (n, _) in &pkg.files {
        if !e.files.iter().any(|f| f.name.eq_ignore_ascii_case(n)) {
            out.push(format!("{n}: not a file of the pinned package"));
        }
    }
    if !pkg.signature.eq_ignore_ascii_case("Valid") {
        out.push(format!(
            "the catalog's signature is {:?}, not Valid",
            pkg.signature
        ));
    }
    if !common_name(&pkg.signer).is_some_and(|cn| cn.eq_ignore_ascii_case(&e.signer)) {
        out.push(format!(
            "the catalog is signed by {:?}, the baseline pins CN={}",
            pkg.signer, e.signer
        ));
    }
    out
}

/// `dasio pc-profile nic-driver --hardware-id <id>... --file
/// <name>=<sha256>... --signature <status> --signer <subject>`
/// (install.ps1's `-NicDriver` step): whether the package install.ps1 read
/// is the pinned one for the Dante NIC with those hardware IDs. The lines
/// to print and the exit code: 0 with `INF <name>` and `VERSION <v>` on a
/// match, 2 with a `REFUSED` line naming every reason otherwise, 64 on
/// arguments it cannot read.
pub fn command(entries: &[NicDriverEntry], args: &[String]) -> (Vec<String>, u8) {
    let mut ids = Vec::new();
    let mut pkg = Package::default();
    let (mut signature, mut signer) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let Some(v) = it.next() else {
            return (vec![format!("{a}: needs a value")], 64);
        };
        match a.as_str() {
            "--hardware-id" => ids.push(v.clone()),
            "--file" => match v.split_once('=') {
                Some((n, h)) if !n.is_empty() => pkg.files.push((n.to_string(), h.to_string())),
                _ => return (vec![format!("--file {v}: give <name>=<sha256>")], 64),
            },
            "--signature" => signature = Some(v.clone()),
            "--signer" => signer = Some(v.clone()),
            _ => return (vec![format!("{a}: not an argument of nic-driver")], 64),
        }
    }
    let (Some(signature), Some(signer)) = (signature, signer) else {
        return (vec!["give --signature and --signer".into()], 64);
    };
    if ids.is_empty() || pkg.files.is_empty() {
        return (vec!["give --hardware-id and --file".into()], 64);
    }
    pkg.signature = signature;
    pkg.signer = signer;
    let Some(e) = pick(entries, &ids) else {
        return (
            vec![format!(
                "REFUSED the Dante NIC ({}) is of no family the baseline pins a driver for",
                ids.join(", ")
            )],
            2,
        );
    };
    let why = verify(e, &pkg);
    if !why.is_empty() {
        return (
            vec![format!(
                "REFUSED this is not {}'s pinned package ({} {} from {}): {}",
                e.family,
                e.inf,
                e.version,
                e.source,
                why.join("; ")
            )],
            2,
        );
    }
    (
        vec![
            format!(
                "the package is {}'s pinned driver ({} {}, signed by {})",
                e.family, e.inf, e.version, e.signer
            ),
            format!("INF {}", e.inf),
            format!("VERSION {}", e.version),
        ],
        0,
    )
}
