//! The faces the interface is drawn with.
//!
//! egui ships its own fonts, which is why a fresh build looks unlike the rest of the
//! desktop: every label, button, dialog and chat bubble comes out in Ubuntu Light. The
//! platform's own UI face is installed ahead of them, and egui's stay behind it as the
//! fallback that supplies the glyphs a system face may lack.
//!
//! The terminal's monospace face is chosen in the settings, so the families the settings
//! name are registered as named families and the terminal asks for the one it was given.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use egui::{FontData, FontDefinitions, FontFamily};

/// Where the platform keeps its fonts.
#[cfg(target_os = "windows")]
const FONT_DIRS: &[&str] = &[r"C:\Windows\Fonts"];
#[cfg(target_os = "macos")]
const FONT_DIRS: &[&str] = &["/System/Library/Fonts", "/Library/Fonts"];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const FONT_DIRS: &[&str] = &[
    "/usr/share/fonts",
    "/usr/local/share/fonts",
];

/// Extensions a face is read from. A collection (`.ttc`) is left out: the loader would draw
/// only its first face, and a font that fails to parse takes the window with it.
const FACE_EXTENSIONS: &[&str] = &["ttf", "otf"];

/// Font files one scan of the platform's directories keeps.
const MAX_FACES: usize = 2_000;

/// Most of a `name` table that is ever read.
const MAX_NAME_TABLE_BYTES: usize = 64 * 1024;

/// UI faces worth installing, best first.
const UI_FACES: &[&str] = &[
    "segoeui",
    "arial",
    "helvetica",
    "dejavusans",
    "liberationsans",
    "notosans",
    "ubuntu",
];

/// One font file a family can be drawn from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Face {
    /// Family the file calls itself.
    pub family: String,
    /// Where it lives.
    pub path: PathBuf,
}

/// Every installed family, sorted, for a settings dropdown.
pub fn installed() -> Vec<String> {
    let mut names: Vec<String> = faces().iter().map(|face| face.family.clone()).collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names
}

/// The file a family is drawn from, or `None` when the host does not have it.
pub fn face_for(family: &str) -> Option<&'static Face> {
    faces()
        .iter()
        .find(|face| face.family.eq_ignore_ascii_case(family.trim()))
}

/// The family to draw terminal text with.
///
/// A name the host does not have falls back to egui's own monospace rather than leaving the
/// terminal blank.
pub fn terminal_family(family: &str) -> FontFamily {
    if family.trim().is_empty() || face_for(family).is_none() {
        return FontFamily::Monospace;
    }
    FontFamily::Name(Arc::from(family.trim()))
}

/// Draw the interface with the platform's own UI face and register the terminal families
/// `mono` names.
///
/// Called whenever the set of wanted families grows: rebuilding the font atlas is what makes
/// a font chosen in the settings show up, and doing it only when the set changes keeps it off
/// the frame path.
pub fn ensure(ctx: &egui::Context, mono: &[String]) {
    let mut wanted: Vec<String> = mono
        .iter()
        .filter_map(|family| face_for(family))
        .map(|face| face.family.clone())
        .collect();
    wanted.sort_by_key(|name| name.to_lowercase());
    wanted.dedup();

    let mut applied = APPLIED.lock().unwrap_or_else(|error| error.into_inner());
    if *applied == wanted && UI_FACE.get().is_some() {
        return;
    }

    let mut fonts = FontDefinitions::default();
    let fallback = fonts
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();
    if let Some((key, bytes)) = system_ui_face() {
        fonts
            .font_data
            .insert(key.clone(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, key.clone());
        let _ = UI_FACE.set(key);
    }
    for family in &wanted {
        let Some(face) = face_for(family) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(&face.path) else {
            continue;
        };
        let key = format!("term:{family}");
        fonts
            .font_data
            .insert(key.clone(), Arc::new(FontData::from_owned(bytes)));
        let mut chain = vec![key];
        chain.extend(fallback.iter().cloned());
        fonts
            .families
            .insert(FontFamily::Name(Arc::from(family.as_str())), chain);
    }
    ctx.set_fonts(fonts);
    *applied = wanted;
}

/// The system UI face, as the key it is registered under and its bytes.
fn system_ui_face() -> Option<(String, Vec<u8>)> {
    static CACHE: OnceLock<Option<(String, Vec<u8>)>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let face = faces()
                .iter()
                .filter(|face| is_ui_candidate(&face.path))
                .min_by_key(|face| ui_rank(&face.path))?;
            let key = stem(&face.path).unwrap_or_else(|| "system-ui".to_owned());
            Some((key, std::fs::read(&face.path).ok()?))
        })
        .clone()
}

/// Whether `path` names a face the interface could be drawn in.
fn is_ui_candidate(path: &Path) -> bool {
    stem(path).is_some_and(|stem| UI_FACES.iter().any(|name| stem == *name))
}

/// How good a UI face is, lower being better.
fn ui_rank(path: &Path) -> usize {
    let stem = stem(path).unwrap_or_default();
    UI_FACES
        .iter()
        .position(|name| stem == *name)
        .unwrap_or(UI_FACES.len())
}

/// A path's file stem, lowercased.
fn stem(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_lowercase())
}

/// Every readable face the platform offers, scanned once.
fn faces() -> &'static [Face] {
    static FACES: OnceLock<Vec<Face>> = OnceLock::new();
    FACES.get_or_init(|| {
        let mut faces = Vec::new();
        for dir in FONT_DIRS {
            collect(Path::new(dir), &mut faces);
        }
        faces
    })
}

/// Read every face under `dir`, following sub-directories.
fn collect(dir: &Path, faces: &mut Vec<Face>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if faces.len() >= MAX_FACES {
            return;
        }
        let path = entry.path();
        if path.is_dir() {
            collect(&path, faces);
            continue;
        }
        let is_face = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
            .is_some_and(|extension| FACE_EXTENSIONS.contains(&extension.as_str()));
        if !is_face {
            continue;
        }
        let Some(family) = family_name(&path) else {
            continue;
        };
        faces.push(Face { family, path });
    }
}

/// The family a font file calls itself, read from its `name` table.
fn family_name(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 12];
    file.read_exact(&mut header).ok()?;
    let tables = u16::from_be_bytes([header[4], header[5]]) as usize;
    let mut directory = vec![0u8; tables.checked_mul(16)?];
    file.read_exact(&mut directory).ok()?;
    let (offset, length) = directory
        .as_chunks::<16>()
        .0
        .iter()
        .find(|record| &record[0..4] == b"name")
        .map(|record| {
            (
                u32::from_be_bytes([record[8], record[9], record[10], record[11]]),
                u32::from_be_bytes([record[12], record[13], record[14], record[15]]),
            )
        })?;
    file.seek(SeekFrom::Start(u64::from(offset))).ok()?;
    let mut table = vec![0u8; (length as usize).min(MAX_NAME_TABLE_BYTES)];
    file.read_exact(&mut table).ok()?;
    read_family(&table)
}

/// The family out of an OpenType `name` table: the typographic name when there is one.
fn read_family(table: &[u8]) -> Option<String> {
    let count = u16::from_be_bytes([*table.get(2)?, *table.get(3)?]) as usize;
    let strings = u16::from_be_bytes([*table.get(4)?, *table.get(5)?]) as usize;
    let mut best: Option<(u8, String)> = None;
    for index in 0..count {
        let record = table.get(6 + index * 12..6 + index * 12 + 12)?;
        let platform = u16::from_be_bytes([record[0], record[1]]);
        let name = u16::from_be_bytes([record[6], record[7]]);
        let length = u16::from_be_bytes([record[8], record[9]]) as usize;
        let offset = u16::from_be_bytes([record[10], record[11]]) as usize;
        // The typographic family says what the user picks in menus; the plain family name
        // is the fallback for fonts old enough to have only that.
        let rank = match (name, platform) {
            (16, 3) => 0u8,
            (1, 3) => 1,
            (16, 1) => 2,
            (1, 1) => 3,
            _ => continue,
        };
        let Some(bytes) = table.get(strings + offset..strings + offset + length) else {
            continue;
        };
        let text = if platform == 3 {
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            bytes.iter().map(|byte| *byte as char).collect()
        };
        let text = text.trim().to_owned();
        if text.is_empty() {
            continue;
        }
        if best.as_ref().is_none_or(|(kept, _)| rank < *kept) {
            best = Some((rank, text));
        }
    }
    best.map(|(_, family)| family)
}

/// Families the font atlas holds, so a rebuild is not asked for twice.
static APPLIED: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Key the system UI face was registered under.
static UI_FACE: OnceLock<String> = OnceLock::new();

#[cfg(test)]
mod tests {
    use super::*;

    /// A `name` table with one Windows record for `family`, name id 16.
    fn name_table(family: &str) -> Vec<u8> {
        let utf16: Vec<u8> = family
            .encode_utf16()
            .flat_map(|unit| unit.to_be_bytes())
            .collect();
        let mut table = Vec::new();
        table.extend(0u16.to_be_bytes()); // format
        table.extend(1u16.to_be_bytes()); // one record
        table.extend(18u16.to_be_bytes()); // strings start after the record
        table.extend(3u16.to_be_bytes()); // platform: Windows
        table.extend(1u16.to_be_bytes()); // encoding
        table.extend(0x0409u16.to_be_bytes()); // language
        table.extend(16u16.to_be_bytes()); // typographic family
        table.extend((utf16.len() as u16).to_be_bytes());
        table.extend(0u16.to_be_bytes()); // offset into the string area
        table.extend(utf16);
        table
    }

    #[test]
    fn the_typographic_family_is_read_from_a_name_table() {
        assert_eq!(
            read_family(&name_table("Segoe UI")),
            Some("Segoe UI".to_owned())
        );
        assert_eq!(read_family(&[]), None);
        assert_eq!(read_family(&[0, 0, 0, 0, 0, 0]), None);
    }

    #[test]
    fn a_font_with_no_name_table_is_skipped() {
        let dir = std::env::temp_dir().join(format!("wassh-faces-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let empty = dir.join("empty.ttf");
        std::fs::write(&empty, b"\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00")
            .expect("write");
        assert_eq!(family_name(&empty), None);
        let mut faces = Vec::new();
        collect(&dir, &mut faces);
        assert!(faces.is_empty(), "{faces:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn the_platform_offers_families_to_choose_from() {
        let families = installed();
        assert!(families.len() > 5, "only {} families found", families.len());
        assert!(face_for("Segoe UI").is_some(), "{families:?}");
        assert!(
            face_for("no such family").is_none(),
            "a name nobody has must not resolve"
        );
    }

    #[test]
    fn a_family_the_host_lacks_falls_back_to_egui_monospace() {
        assert_eq!(
            terminal_family("no-such-font-anywhere"),
            FontFamily::Monospace
        );
        assert_eq!(terminal_family(""), FontFamily::Monospace);
    }
}
