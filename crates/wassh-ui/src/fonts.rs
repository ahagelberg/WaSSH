//! The faces the interface is drawn with.
//!
//! egui ships its own fonts, which is why a fresh build looks unlike the rest of the
//! desktop: every label, button, dialog and chat bubble comes out in Ubuntu Light. The
//! platform's own UI face is installed ahead of them, and egui's stay behind it as the
//! fallback that supplies the glyphs a system face may lack.

use std::sync::Arc;

use egui::FontFamily;

/// Name the system face is registered under.
const SYSTEM_FACE: &str = "system-ui";

/// Where the platform keeps a proportional UI face, best first.
///
/// Only single-font files are listed: a collection (`.ttc`) is not something the font
/// loader can be trusted to open, and a font that fails to parse takes the window with it.
#[cfg(target_os = "windows")]
const SYSTEM_FONTS: &[&str] = &[
    r"C:\Windows\Fonts\segoeui.ttf",
    r"C:\Windows\Fonts\arial.ttf",
];

#[cfg(target_os = "macos")]
const SYSTEM_FONTS: &[&str] = &[
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/Library/Fonts/Arial.ttf",
];

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const SYSTEM_FONTS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
];

/// Draw the interface with the platform's own UI face.
///
/// Called once, when the window is created. A platform with none of the listed faces keeps
/// egui's own, so this is never a reason for the window not to open.
pub fn install(ctx: &egui::Context) {
    let Some((path, bytes)) = read_first(SYSTEM_FONTS) else {
        return;
    };
    let name = std::path::Path::new(&path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| SYSTEM_FACE.to_owned());
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert(name.clone(), Arc::new(egui::FontData::from_owned(bytes)));
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, name);
    ctx.set_fonts(fonts);
}

/// The bytes of the first candidate that reads, with the path they came from.
fn read_first(candidates: &[&str]) -> Option<(String, Vec<u8>)> {
    candidates
        .iter()
        .find_map(|path| std::fs::read(path).ok().map(|bytes| ((*path).to_owned(), bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_readable_candidate_wins() {
        let dir = std::env::temp_dir().join(format!("wassh-fonts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let present = dir.join("present.ttf");
        std::fs::write(&present, b"not really a font").expect("write");
        let missing = dir.join("missing.ttf");
        let picked = read_first(&[
            missing.to_str().expect("utf-8 path"),
            present.to_str().expect("utf-8 path"),
        ]);
        assert_eq!(
            picked.map(|(path, _)| path),
            Some(present.display().to_string())
        );
        assert!(read_first(&[]).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_offers_a_system_face() {
        let (path, bytes) = read_first(SYSTEM_FONTS).expect("no system font found");
        assert!(!bytes.is_empty(), "{path} is empty");
    }

    #[test]
    fn installing_and_drawing_a_frame_works_whatever_fonts_are_present() {
        let ctx = egui::Context::default();
        install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(200.0, 100.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| {
            ui.label("Hello from the interface");
        });
        let painted = output
            .shapes
            .iter()
            .any(|clipped| matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text().starts_with("Hello")));
        assert!(painted, "the label was not painted");
        output.drop_without_applying_deltas();
    }

    /// Width of one word, drawn by `ctx`, in points.
    fn width_of(ctx: &egui::Context, word: &str) -> f32 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(400.0, 100.0),
            )),
            ..Default::default()
        };
        let mut width = 0.0;
        let output = ctx.run_ui(input, |ui| {
            width = ui
                .painter()
                .layout_no_wrap(
                    word.to_owned(),
                    egui::FontId::proportional(13.0),
                    egui::Color32::WHITE,
                )
                .size()
                .x;
        });
        output.drop_without_applying_deltas();
        width
    }

    #[test]
    fn the_interface_is_measured_in_the_system_face() {
        if read_first(SYSTEM_FONTS).is_none() {
            return;
        }
        let plain = egui::Context::default();
        let installed = egui::Context::default();
        install(&installed);
        assert_ne!(
            width_of(&plain, "Handgloves"),
            width_of(&installed, "Handgloves"),
            "the system face replaced nothing, so the labels still use egui's own"
        );
    }
}
