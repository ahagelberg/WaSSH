//! The faces the interface is drawn with.
//!
//! egui ships its own fonts, which is why a fresh build looks unlike the rest of the
//! desktop: every label, button, dialog and chat bubble comes out in Ubuntu Light. The
//! platform's own UI face is installed ahead of them, and egui's stay behind it as the
//! fallback that supplies the glyphs a system face may lack.
//!
//! The terminal's monospace face is chosen in the settings, so the families the settings
//! name are registered as named families and the terminal asks for the one it was given.
//! Families shipped with the app come first, so the list does not depend on what the host
//! happens to have installed — the default family is one of them. Only families whose
//! characters share one advance are offered, and each is drawn from the file of its regular
//! style, never from whichever bold or italic file the scan met first.

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

/// Bytes of one record in a font file's table directory.
const TABLE_RECORD_BYTES: usize = 16;

/// Font files one scan of the platform's directories keeps.
const MAX_FACES: usize = 2_000;

/// Most of a `name` table that is ever read.
const MAX_NAME_TABLE_BYTES: usize = 64 * 1024;

/// Where in its table each field the scan reads starts: `head.macStyle`, `post.isFixedPitch`,
/// and `OS/2`'s weight class and panose proportion.
const MAC_STYLE_AT: usize = 44;
const FIXED_PITCH_AT: usize = 12;
const OS2_WEIGHT_AT: usize = 4;
const OS2_PANOSE_PROPORTION_AT: usize = 35;

/// Bytes each of those fields takes.
const MAC_STYLE_WIDTH: usize = 2;
const POST_FIXED_PITCH_WIDTH: usize = 4;
const WEIGHT_WIDTH: usize = 2;

/// `head.macStyle` bit marking a file drawn in bold, and the one marking it drawn slanted.
const MAC_STYLE_BOLD: u16 = 1;
const MAC_STYLE_ITALIC: u16 = 2;

/// `OS/2.usWeightClass` of a regular face.
const REGULAR_WEIGHT: u16 = 400;

/// Panose proportion of a face whose characters all advance by one width.
const PANOSE_MONOSPACED: u8 = 9;

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

/// The regular face of a family the app draws terminal text with, whatever the host has.
struct Bundled {
    /// Family name the settings store.
    family: &'static str,
    /// The family's regular file, compiled in.
    bytes: &'static [u8],
}

/// Monospace families shipped with the app, each with its licence beside it in `assets`.
const BUNDLED: &[Bundled] = &[
    Bundled {
        family: "Cascadia Mono",
        bytes: include_bytes!("../assets/fonts/cascadia-mono/CascadiaMono.ttf"),
    },
    Bundled {
        family: "Fira Code",
        bytes: include_bytes!("../assets/fonts/fira-code/FiraCode-Regular.ttf"),
    },
    Bundled {
        family: "Hack",
        bytes: include_bytes!("../assets/fonts/hack/Hack-Regular.ttf"),
    },
    Bundled {
        family: "IBM Plex Mono",
        bytes: include_bytes!("../assets/fonts/ibm-plex-mono/IBMPlexMono-Regular.ttf"),
    },
    Bundled {
        family: "JetBrains Mono",
        bytes: include_bytes!("../assets/fonts/jetbrains-mono/JetBrainsMono-Regular.ttf"),
    },
    Bundled {
        family: "Source Code Pro",
        bytes: include_bytes!("../assets/fonts/source-code-pro/SourceCodePro-Regular.ttf"),
    },
];

/// Where a family's regular file comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A file the host has installed.
    File(PathBuf),
    /// A face compiled into the app.
    Bundled(&'static [u8]),
}

/// One font file a family can be drawn from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Face {
    /// Family the file calls itself.
    pub family: String,
    /// Where its bytes are.
    pub source: Source,
    /// Draws every character at one advance, so a grid of cells can be built from it.
    pub monospace: bool,
}

impl Face {
    /// The file, when the host is where it lives.
    fn path(&self) -> Option<&Path> {
        match &self.source {
            Source::File(path) => Some(path),
            Source::Bundled(_) => None,
        }
    }

    /// The bytes the family is drawn from.
    pub fn bytes(&self) -> Option<Vec<u8>> {
        match &self.source {
            Source::File(path) => std::fs::read(path).ok(),
            Source::Bundled(bytes) => Some(bytes.to_vec()),
        }
    }
}

/// Where a file stands among the files of its family; lower is the one a grid is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct StyleRank {
    /// `head.macStyle` marks the file bold and/or slanted.
    marked: u8,
    /// Distance from the regular weight, so a Light or Black file ranks below it.
    weight_gap: u16,
}

impl StyleRank {
    fn of(mac_style: u16, weight: u16) -> Self {
        let bold = u8::from(mac_style & MAC_STYLE_BOLD != 0);
        let italic = u8::from(mac_style & MAC_STYLE_ITALIC != 0);
        Self {
            marked: bold + italic,
            weight_gap: weight.abs_diff(REGULAR_WEIGHT),
        }
    }
}

/// A font file, and where its tables sit.
struct FontFile {
    file: std::fs::File,
    tables: Vec<([u8; 4], (u32, u32))>,
}

impl FontFile {
    /// Open `path` and read its table directory.
    fn open(path: &Path) -> Option<Self> {
        let mut file = std::fs::File::open(path).ok()?;
        let mut header = [0u8; 12];
        file.read_exact(&mut header).ok()?;
        let count = u16::from_be_bytes([header[4], header[5]]) as usize;
        let mut directory = vec![0u8; count.checked_mul(TABLE_RECORD_BYTES)?];
        file.read_exact(&mut directory).ok()?;
        let tables = directory
            .as_chunks::<TABLE_RECORD_BYTES>()
            .0
            .iter()
            .map(|record| {
                (
                    [record[0], record[1], record[2], record[3]],
                    (
                        u32::from_be_bytes([record[8], record[9], record[10], record[11]]),
                        u32::from_be_bytes([record[12], record[13], record[14], record[15]]),
                    ),
                )
            })
            .collect();
        Some(Self { file, tables })
    }

    /// The first `max` bytes of the table `tag`.
    fn table(&mut self, tag: &[u8; 4], max: usize) -> Option<Vec<u8>> {
        let (offset, length) = self
            .tables
            .iter()
            .find_map(|(found, range)| (found == tag).then_some(*range))?;
        self.file.seek(SeekFrom::Start(u64::from(offset))).ok()?;
        let mut bytes = vec![0u8; (length as usize).min(max)];
        self.file.read_exact(&mut bytes).ok()?;
        Some(bytes)
    }

    /// `N` bytes of the table `tag`, read from `at`.
    fn field<const N: usize>(&mut self, tag: &[u8; 4], at: usize) -> Option<[u8; N]> {
        let table = self.table(tag, at + N)?;
        <[u8; N]>::try_from(table.get(at..at + N)?).ok()
    }
}

/// What a font file says about itself that the settings list needs.
struct FaceInfo {
    family: String,
    monospace: bool,
    style: StyleRank,
}

impl FaceInfo {
    fn read(path: &Path) -> Option<Self> {
        let mut file = FontFile::open(path)?;
        let family = read_family(&file.table(b"name", MAX_NAME_TABLE_BYTES)?)?;
        Some(Self {
            family,
            monospace: is_monospaced(&mut file),
            style: style_rank(&mut file),
        })
    }
}

/// Whether every character of the face advances by one width.
///
/// Windows marks such a face in the panose proportion and most faces also set `post`'s
/// fixed-pitch flag; either one is enough, because some fonts fill in only one of them.
fn is_monospaced(file: &mut FontFile) -> bool {
    let flagged = file
        .field::<POST_FIXED_PITCH_WIDTH>(b"post", FIXED_PITCH_AT)
        .is_some_and(|bytes| u32::from_be_bytes(bytes) != 0);
    let proportion = file.field::<1>(b"OS/2", OS2_PANOSE_PROPORTION_AT);
    flagged || proportion == Some([PANOSE_MONOSPACED])
}

/// How the file's style ranks against the other files of its family.
fn style_rank(file: &mut FontFile) -> StyleRank {
    let mac_style = file
        .field::<MAC_STYLE_WIDTH>(b"head", MAC_STYLE_AT)
        .map_or(0, u16::from_be_bytes);
    let weight = file
        .field::<WEIGHT_WIDTH>(b"OS/2", OS2_WEIGHT_AT)
        .map_or(REGULAR_WEIGHT, u16::from_be_bytes);
    StyleRank::of(mac_style, weight)
}

/// Every family a terminal grid can be drawn in — the ones shipped with the app and the ones
/// the host has — sorted, for a settings dropdown.
pub fn offered() -> Vec<String> {
    faces()
        .iter()
        .filter(|face| face.monospace)
        .map(|face| face.family.clone())
        .collect()
}

/// The file a family is drawn from, or `None` when the host does not have it.
pub fn face_for(family: &str) -> Option<&'static Face> {
    faces()
        .iter()
        .find(|face| face.family.eq_ignore_ascii_case(family.trim()))
}

/// The family to draw terminal text with.
///
/// A name no family answers to falls back to egui's own monospace rather than leaving the
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
        let data = match &face.source {
            Source::Bundled(bytes) => FontData::from_static(bytes),
            Source::File(path) => match std::fs::read(path) {
                Ok(bytes) => FontData::from_owned(bytes),
                Err(_) => continue,
            },
        };
        let key = format!("term:{family}");
        fonts.font_data.insert(key.clone(), Arc::new(data));
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
            let path = faces()
                .iter()
                .filter_map(|face| face.path())
                .filter(|path| is_ui_candidate(path))
                .min_by_key(|path| ui_rank(path))?;
            let key = stem(path).unwrap_or_else(|| "system-ui".to_owned());
            Some((key, std::fs::read(path).ok()?))
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

/// Every family the terminal can be drawn in: the ones shipped with the app first, then the
/// platform's, each drawn from the file of its regular style.
fn faces() -> &'static [Face] {
    static FACES: OnceLock<Vec<Face>> = OnceLock::new();
    FACES.get_or_init(|| {
        let mut shipped: Vec<Face> = BUNDLED
            .iter()
            .map(|bundled| Face {
                family: bundled.family.to_owned(),
                source: Source::Bundled(bundled.bytes),
                monospace: true,
            })
            .collect();
        let mut found = Vec::new();
        for dir in FONT_DIRS {
            collect(Path::new(dir), &mut found);
        }
        // A family ships one file per style. Keep the upright file closest to the regular
        // weight, and order what is left the way the settings list shows it.
        found.sort_by(|(left, left_style), (right, right_style)| {
            left.family
                .to_lowercase()
                .cmp(&right.family.to_lowercase())
                .then(left_style.cmp(right_style))
                .then(left.path().cmp(&right.path()))
        });
        found.dedup_by(|left, right| left.0.family.eq_ignore_ascii_case(&right.0.family));
        // A shipped family answers for its name even where the host has its own copy, so the
        // terminal is drawn the same on every machine.
        let system: Vec<Face> = found
            .into_iter()
            .map(|(face, _)| face)
            .filter(|face| {
                !shipped
                    .iter()
                    .any(|known| known.family.eq_ignore_ascii_case(&face.family))
            })
            .collect();
        shipped.extend(system);
        shipped.sort_by_key(|face| face.family.to_lowercase());
        shipped
    })
}

/// Read every face under `dir`, following sub-directories.
fn collect(dir: &Path, faces: &mut Vec<(Face, StyleRank)>) {
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
        let Some(info) = FaceInfo::read(&path) else {
            continue;
        };
        faces.push((
            Face {
                family: info.family,
                source: Source::File(path),
                monospace: info.monospace,
            },
            info.style,
        ));
    }
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
        assert!(FaceInfo::read(&empty).is_none());
        let mut faces = Vec::new();
        collect(&dir, &mut faces);
        assert!(faces.is_empty(), "{faces:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn the_platform_offers_families_to_choose_from() {
        let families = offered();
        assert!(families.len() > 5, "only {} families found", families.len());
        assert!(face_for("Segoe UI").is_some(), "{families:?}");
        assert!(
            face_for("no such family").is_none(),
            "a name nobody has must not resolve"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn only_monospaced_families_are_offered() {
        let families = offered();
        let Some(face) = face_for("Courier New") else {
            return;
        };
        assert!(face.monospace, "Courier New is drawn in one advance");
        assert!(
            families.iter().any(|family| family == "Courier New"),
            "{families:?}"
        );
        // A proportional face stays reachable for a setting that already names it, but the
        // dropdown must not offer it.
        assert!(face_for("Segoe UI").is_some());
        assert!(
            !families.iter().any(|family| family == "Segoe UI"),
            "{families:?}"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn a_family_is_drawn_from_its_regular_file() {
        let regular = StyleRank::of(0, REGULAR_WEIGHT);
        let Some(path) = face_for("Consolas").and_then(Face::path) else {
            return;
        };
        assert_eq!(
            FaceInfo::read(path).expect("the chosen face").style,
            regular,
            "{} is not the family's regular file",
            path.display()
        );
        // The host really ships the other styles under the same family name.
        let italic = path.with_file_name("consolai.ttf");
        if italic.exists() {
            let style = FaceInfo::read(&italic).expect("the italic face").style;
            assert!(style > regular, "{style:?} ranks with the regular file");
        }
    }

    #[test]
    fn the_families_shipped_with_the_app_are_offered() {
        let families = offered();
        for bundled in BUNDLED {
            assert!(
                families.iter().any(|family| family == bundled.family),
                "{} is not offered: {families:?}",
                bundled.family
            );
            assert_eq!(
                terminal_family(bundled.family),
                FontFamily::Name(Arc::from(bundled.family)),
                "{} does not resolve to a family of its own",
                bundled.family
            );
        }
        let mut sorted = families.clone();
        sorted.sort_by_key(|family| family.to_lowercase());
        sorted.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        assert_eq!(
            sorted.len(),
            families.len(),
            "a family is offered twice: {families:?}"
        );
    }

    #[test]
    fn a_shipped_face_is_the_regular_monospaced_file() {
        let dir = std::env::temp_dir().join(format!("wassh-bundled-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        for bundled in BUNDLED {
            let path = dir.join(format!("{}.ttf", bundled.family.replace(' ', "-")));
            std::fs::write(&path, bundled.bytes).expect("write");
            let info = FaceInfo::read(&path)
                .unwrap_or_else(|| panic!("{} does not parse", bundled.family));
            assert_eq!(info.family, bundled.family, "the manifest names another family");
            assert!(
                info.monospace,
                "{} is not drawn in one advance",
                bundled.family
            );
            assert_eq!(
                info.style,
                StyleRank::of(0, REGULAR_WEIGHT),
                "{} is not its regular file",
                bundled.family
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_family_no_one_has_falls_back_to_egui_monospace() {
        assert_eq!(
            terminal_family("no-such-font-anywhere"),
            FontFamily::Monospace
        );
        assert_eq!(terminal_family(""), FontFamily::Monospace);
    }
}
