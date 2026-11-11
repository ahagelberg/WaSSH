//! Windows Subsystem for Linux environments.
//!
//! WSL has no API of its own: `wsl.exe` is the interface, and its redirected output is
//! UTF-16LE, hence the decoder below.

use std::process::Stdio;

/// The WSL launcher.
const WSL: &str = "wsl.exe";
/// Lists installed distributions, names only.
const LIST_INSTALLED: [&str; 2] = ["--list", "--quiet"];
/// Lists installed distributions with their state, version and default marker.
const LIST_VERBOSE: [&str; 2] = ["--list", "--verbose"];
/// Lists distributions that can be installed.
const LIST_ONLINE: [&str; 2] = ["--list", "--online"];
/// Selects a distribution to run.
const DISTRIBUTION: &str = "--distribution";
/// Byte-order mark WSL puts on UTF-16LE output.
const UTF16LE_BOM: &[u8] = &[0xFF, 0xFE];

/// One installed distribution, as `wsl --list --verbose` reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DistroInfo {
    pub name: String,
    /// `Running`, `Stopped`, … — WSL's own wording, shown as-is.
    pub state: String,
    /// WSL version, 1 or 2.
    pub version: u8,
    /// Whether WSL starts this one when asked for its default distribution.
    pub default: bool,
}

/// How to launch `distro`, or WSL's default when there is none.
pub fn launch(distro: Option<&str>) -> (String, Vec<String>) {
    match distro {
        Some(name) => (
            WSL.to_owned(),
            vec![DISTRIBUTION.to_owned(), name.to_owned()],
        ),
        None => (WSL.to_owned(), Vec::new()),
    }
}

/// Installed distributions, in WSL's order.
pub fn distros() -> Vec<String> {
    parse_quiet_names(&run_list(&LIST_INSTALLED))
}

/// Distributions that `wsl --install` can fetch.
pub fn online_distros() -> Vec<String> {
    parse_table_names(&run_list(&LIST_ONLINE))
}

/// Installed distributions with their state, version and default marker.
pub fn listing() -> Vec<DistroInfo> {
    parse_listing(&run_list(&LIST_VERBOSE))
}

/// Whether the Windows Subsystem for Linux itself is installed.
///
/// `wsl.exe` ships with Windows even when the subsystem is not enabled, so it has to be
/// asked: `--status` answers without the "press any key to install" prompt that the list
/// commands show on such a machine.
pub fn installed() -> bool {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new(WSL)
            .arg("--status")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// What `wsl.exe` wrote for a listing command, or nothing when WSL is missing.
#[cfg(target_os = "windows")]
fn run_list(args: &[&str]) -> String {
    if !installed() {
        return String::new();
    }
    let Ok(output) = std::process::Command::new(WSL)
        .args(args)
        .stdin(Stdio::null())
        .output()
    else {
        return String::new();
    };
    decode_console_output(&output.stdout)
}

#[cfg(not(target_os = "windows"))]
fn run_list(_args: &[&str]) -> String {
    String::new()
}

/// Decode what `wsl.exe` wrote to a redirected stream.
///
/// WSL writes UTF-16LE whatever the console code page; reading it as UTF-8 would
/// interleave NUL bytes. Some output is plain UTF-8, so both are accepted.
pub fn decode_console_output(bytes: &[u8]) -> String {
    let body = bytes.strip_prefix(UTF16LE_BOM).unwrap_or(bytes);
    if body.len() < bytes.len() || looks_like_utf16(body) {
        let (pairs, _) = body.as_chunks::<2>();
        let units: Vec<u16> = pairs
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Whether every other byte is NUL, which is what ASCII written as UTF-16LE looks like.
fn looks_like_utf16(bytes: &[u8]) -> bool {
    let (pairs, _) = bytes.as_chunks::<2>();
    bytes.len() >= 2
        && bytes.len().is_multiple_of(2)
        && pairs.iter().filter(|pair| pair[1] == 0).count() * 2 >= pairs.len()
}

/// Names from `wsl --list --quiet`: one distribution per line, and nothing else.
///
/// A machine with WSL but no distributions answers with an advice paragraph instead, so a
/// line only counts when it holds a single word.
fn parse_quiet_names(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let mut parts = line.trim_start_matches('*').split_whitespace();
            let name = parts.next()?;
            parts.next().is_none().then(|| name.to_owned())
        })
        .collect()
}

/// Names from the `wsl --list --online` table: first column, rows after the NAME header.
///
/// Everything before the header is WSL's own prose and everything after it is rows, so the
/// header is what marks the start of the table.
fn parse_table_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_table = false;
    for line in text.lines().map(str::trim) {
        if !in_table {
            in_table = line.starts_with("NAME");
            continue;
        }
        if let Some(name) = line.split_whitespace().next() {
            names.push(name.trim_start_matches('*').to_owned());
        }
    }
    names
}

/// One row per installed distribution from a `wsl --list --verbose` table.
///
/// Rows are only read after the NAME header and only when the last column is a version, so
/// the advice WSL prints when nothing is installed never becomes a distribution.
fn parse_listing(text: &str) -> Vec<DistroInfo> {
    let mut listing = Vec::new();
    let mut in_table = false;
    for line in text.lines().map(str::trim) {
        if !in_table {
            in_table = line.starts_with("NAME");
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let default = line.starts_with('*');
        let mut parts = line.trim_start_matches('*').split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        let rest: Vec<&str> = parts.collect();
        let Some(last) = rest.last() else {
            continue;
        };
        let Ok(version) = last.parse::<u8>() else {
            continue;
        };
        listing.push(DistroInfo {
            name: name.to_owned(),
            state: rest[..rest.len() - 1].join(" "),
            version,
            default,
        });
    }
    listing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf16le_listings() {
        let text = "Ubuntu\r\nDebian\r\n";
        let mut bytes = UTF16LE_BOM.to_vec();
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_console_output(&bytes), text);
    }

    #[test]
    fn decodes_utf8_listings() {
        assert_eq!(decode_console_output(b"Ubuntu\r\n"), "Ubuntu\r\n");
    }

    #[test]
    fn parses_quiet_names_and_the_online_table() {
        assert_eq!(parse_quiet_names("Ubuntu\r\nDebian\r\n"), vec!["Ubuntu", "Debian"]);
        assert_eq!(parse_quiet_names("* Ubuntu\r\n"), vec!["Ubuntu"]);
        assert_eq!(
            parse_table_names(
                "The following is a list of valid distributions that can be installed.\r\n\r\n\
                 NAME            FRIENDLY NAME\r\n\
                 Ubuntu          Ubuntu\r\n\
                 Debian          Debian GNU/Linux\r\n"
            ),
            vec!["Ubuntu", "Debian"]
        );
    }

    #[test]
    fn the_no_distribution_advice_is_not_a_listing() {
        // WSL answers a listing with this paragraph instead of a table when nothing is
        // installed; every word of it used to become a distribution row.
        let advice = "Windows Subsystem for Linux has no installed distributions.\r\n\
                      You can resolve this by installing a distribution with the \
                      instructions below:\r\n\
                      Use 'wsl.exe --list --online' to list available distributions\r\n\
                      and 'wsl.exe --install <Distro>' to install.";
        assert!(parse_quiet_names(advice).is_empty());
        assert!(parse_table_names(advice).is_empty());
        assert!(parse_listing(advice).is_empty());
    }

    #[test]
    fn parses_state_version_and_the_default_marker() {
        let listing = parse_listing(
            "  NAME            STATE           VERSION\r\n\
             * Ubuntu          Running         2\r\n\
               Debian          Stopped         1\r\n",
        );
        assert_eq!(
            listing,
            vec![
                DistroInfo {
                    name: "Ubuntu".to_owned(),
                    state: "Running".to_owned(),
                    version: 2,
                    default: true,
                },
                DistroInfo {
                    name: "Debian".to_owned(),
                    state: "Stopped".to_owned(),
                    version: 1,
                    default: false,
                },
            ]
        );
    }

    #[test]
    fn launches_the_chosen_distribution() {
        assert_eq!(
            launch(Some("Ubuntu")),
            (
                WSL.to_owned(),
                vec![DISTRIBUTION.to_owned(), "Ubuntu".to_owned()]
            )
        );
        assert_eq!(launch(None), (WSL.to_owned(), Vec::new()));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn other_platforms_have_no_wsl() {
        assert!(!installed());
        assert!(distros().is_empty());
        assert!(online_distros().is_empty());
    }
}
