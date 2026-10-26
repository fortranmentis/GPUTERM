//! Which Linux distribution a host runs, from os-release(5). Telemetry reads
//! the same /proc and coreutils output on every distribution; this names the
//! host (Fedora, openSUSE, Arch, Gentoo, Ubuntu, …) for the Monitor.

use serde::Serialize;

/// systemd and every current distribution ship /etc/os-release; a few minimal
/// images only have the vendor copy in /usr/lib.
pub(crate) const OS_RELEASE_COMMAND: &str =
    "cat /etc/os-release 2>/dev/null || cat /usr/lib/os-release 2>/dev/null || true";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OsRelease {
    /// Lowercase machine id: `fedora`, `opensuse-tumbleweed`, `arch`, `gentoo`.
    pub(crate) id: String,
    /// Parent distributions, e.g. `["suse", "opensuse"]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) id_like: Vec<String>,
    /// For display: `Fedora Linux 41 (Server Edition)`, `Arch Linux`.
    pub(crate) pretty_name: String,
    /// Absent on rolling releases (Arch, Tumbleweed, Gentoo).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) version_id: Option<String>,
}

/// Parses `KEY=value` lines, unquoting shell-style values. Returns None when
/// the file is missing or names nothing.
pub fn parse_os_release(output: &str) -> Option<OsRelease> {
    let mut id = None;
    let mut id_like = Vec::new();
    let mut name = None;
    let mut pretty_name = None;
    let mut version_id = None;
    for line in output.lines().map(str::trim) {
        if line.starts_with('#') {
            continue;
        }
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        let value = unquote(raw);
        if value.is_empty() {
            continue;
        }
        match key {
            "ID" => id = Some(value.to_ascii_lowercase()),
            "ID_LIKE" => {
                id_like = value
                    .split_whitespace()
                    .map(str::to_ascii_lowercase)
                    .collect()
            }
            "NAME" => name = Some(value),
            "PRETTY_NAME" => pretty_name = Some(value),
            "VERSION_ID" => version_id = Some(value),
            _ => {}
        }
    }
    // os-release(5): a missing ID defaults to "linux".
    let pretty_name = pretty_name.or_else(|| name.clone())?;
    Some(OsRelease {
        id: id.unwrap_or_else(|| "linux".to_string()),
        id_like,
        pretty_name,
        version_id,
    })
}

/// Values may be bare, or wrapped in single or double quotes with
/// backslash escapes inside double quotes.
fn unquote(raw: &str) -> String {
    let raw = raw.trim();
    let bytes = raw.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        let inner = &raw[1..raw.len() - 1];
        if bytes[0] == b'\'' {
            return inner.to_string();
        }
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else {
                out.push(c);
            }
        }
        return out;
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEDORA: &str = r#"NAME="Fedora Linux"
VERSION="41 (Server Edition)"
ID=fedora
VERSION_ID=41
PRETTY_NAME="Fedora Linux 41 (Server Edition)"
ANSI_COLOR="0;38;2;60;110;180"
"#;

    const TUMBLEWEED: &str = r#"NAME="openSUSE Tumbleweed"
# VERSION="20241020"
ID="opensuse-tumbleweed"
ID_LIKE="opensuse suse"
VERSION_ID="20241020"
PRETTY_NAME="openSUSE Tumbleweed"
"#;

    const ARCH: &str =
        "NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nID=arch\nBUILD_ID=rolling\n";

    const GENTOO: &str =
        "NAME=Gentoo\nID=gentoo\nPRETTY_NAME=\"Gentoo Linux\"\nVERSION_ID=\"2.17\"\n";

    const UBUNTU: &str = "PRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\nID_LIKE=debian\n";

    #[test]
    fn reads_fedora() {
        let release = parse_os_release(FEDORA).unwrap();
        assert_eq!(release.id, "fedora");
        assert_eq!(release.pretty_name, "Fedora Linux 41 (Server Edition)");
        assert_eq!(release.version_id.as_deref(), Some("41"));
        assert!(release.id_like.is_empty());
    }

    #[test]
    fn reads_tumbleweed_with_its_parents_and_skips_comments() {
        let release = parse_os_release(TUMBLEWEED).unwrap();
        assert_eq!(release.id, "opensuse-tumbleweed");
        assert_eq!(release.id_like, vec!["opensuse", "suse"]);
        assert_eq!(release.version_id.as_deref(), Some("20241020"));
    }

    #[test]
    fn reads_rolling_releases_without_a_version() {
        let arch = parse_os_release(ARCH).unwrap();
        assert_eq!(
            (arch.id.as_str(), arch.pretty_name.as_str()),
            ("arch", "Arch Linux")
        );
        assert_eq!(arch.version_id, None);
        let gentoo = parse_os_release(GENTOO).unwrap();
        assert_eq!(gentoo.id, "gentoo");
        assert_eq!(gentoo.pretty_name, "Gentoo Linux");
    }

    #[test]
    fn keeps_the_ubuntu_baseline() {
        let release = parse_os_release(UBUNTU).unwrap();
        assert_eq!(release.id, "ubuntu");
        assert_eq!(release.id_like, vec!["debian"]);
        assert_eq!(release.pretty_name, "Ubuntu 24.04.1 LTS");
    }

    #[test]
    fn falls_back_to_name_and_the_default_id() {
        let release = parse_os_release("NAME='Custom OS'\n").unwrap();
        assert_eq!(release.pretty_name, "Custom OS");
        assert_eq!(release.id, "linux");
    }

    #[test]
    fn unescapes_double_quoted_values() {
        let release = parse_os_release("ID=x\nPRETTY_NAME=\"Say \\\"hi\\\"\"\n").unwrap();
        assert_eq!(release.pretty_name, "Say \"hi\"");
    }

    #[test]
    fn a_missing_file_names_nothing() {
        assert_eq!(parse_os_release(""), None);
        assert_eq!(parse_os_release("ID=fedora\n"), None);
    }
}
