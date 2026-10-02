//! The fonts the terminal offers: the monospace family the app bundles, the
//! monospace families the user has imported into the data directory, and the
//! monospace families installed on the machine.
//!
//! Monospace only, on purpose. The terminal paints a grid — the cursor, the
//! selection and every highlight are anchored to cell arithmetic — and a
//! proportional face inside that grid leaves gaps between narrow glyphs and
//! drifts against the cursor no matter how the painter compensates. The two
//! proportional faces bundled with the app (MiSans, HarmonyOS Sans SC) are
//! registered for the text system at large but are deliberately absent here.

use std::path::PathBuf;

/// The terminal's default family, which is embedded in the binary and
/// therefore not a system face. It comes first in the list and is what an
/// empty `font_family` setting means: it is glyph-complete for the terminal's
/// own box drawing, and it is the only family the app can guarantee is there.
pub const BUILT_IN_MONO: &str = "Meatshell Mono";

/// The proportional faces bundled with the app for the text system at large.
/// They are never terminal candidates — a proportional face in a cell grid is
/// the drift this module exists to prevent — and a config that still names one
/// is migrated to the default by the config layer, which owns the retired list.
pub const BUNDLED_PROPORTIONAL: &[&str] = &["MiSans", "HarmonyOS Sans SC"];

/// The data directory's font folder, where imports are copied. Created on
/// demand by [`import_font`]; a missing directory simply means no imports.
pub fn imported_fonts_dir() -> PathBuf {
    crate::config::data_dir().join("fonts")
}

/// Copy a font file the user picked into the import directory, and return the
/// family it declares.
///
/// Proportional faces are refused: imports exist to be terminal fonts, and a
/// terminal font has to be monospace. The family name is read from the file's
/// own `name` table rather than trusted from the filename — a renamed font
/// must still be offered by the name the renderer will match.
pub fn import_font(source: &std::path::Path) -> std::io::Result<String> {
    let bytes = std::fs::read(source)?;
    let mut db = fontdb::Database::new();
    db.load_font_data(bytes.clone());
    let face = db
        .faces()
        .next()
        .ok_or_else(|| not_a_font(source))?;
    if !face.monospaced {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{} is not a monospace font, and the terminal can only use monospace fonts",
                source.display()
            ),
        ));
    }
    let family = face
        .families
        .first()
        .map(|(name, _)| name.clone())
        .ok_or_else(|| not_a_font(source))?;
    let dir = imported_fonts_dir();
    std::fs::create_dir_all(&dir)?;
    // Keep the original file name when it is free; suffix a copy rather than
    // overwrite — two fonts named the same are two files the user wanted both of.
    let file_name = source
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| "imported.ttf".into());
    let mut target = dir.join(&file_name);
    if target.exists() {
        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "imported".into());
        let ext = source
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        for n in 1.. {
            target = dir.join(format!("{stem} ({n}){ext}"));
            if !target.exists() {
                break;
            }
        }
    }
    std::fs::write(&target, &bytes)?;
    Ok(family)
}

fn not_a_font(source: &std::path::Path) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("{} is not a font file this app can read", source.display()),
    )
}

/// The monospace families the user has imported, sorted and deduplicated. A
/// file the parser cannot read is skipped: a broken download in the folder
/// must not empty the picker.
fn scan_imported_monospace() -> Vec<String> {
    let dir = imported_fonts_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("ttf") | Some("otf") | Some("otc") | Some("ttc")
            )
        })
        .filter_map(|path| {
            let mut db = fontdb::Database::new();
            db.load_font_file(&path).ok()?;
            let face = db.faces().next()?;
            if !face.monospaced {
                return None;
            }
            face.families.first().map(|(name, _)| name.clone())
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Every family the terminal picker should offer: the bundled monospace first,
/// then the user's imported monospace fonts, then the machine's installed
/// monospace faces.
///
/// Sorted and deduplicated within each group: a picker whose entries move
/// between openings is a picker nobody can learn. Proportional faces — bundled
/// or installed — are not terminal candidates and are filtered out.
pub fn available_families() -> Vec<String> {
    let mut out: Vec<String> = vec![BUILT_IN_MONO.to_string()];
    for family in scan_imported_monospace() {
        if !out.contains(&family) {
            out.push(family);
        }
    }
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut system: Vec<String> = db
        .faces()
        .filter(|face| face.monospaced)
        .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
        .filter(|name| !out.contains(name) && !is_bundled_proportional(name))
        .collect();
    system.sort();
    system.dedup();
    out.extend(system);
    out
}

fn is_bundled_proportional(name: &str) -> bool {
    BUNDLED_PROPORTIONAL.iter().any(|bundled| *bundled == name)
}

/// Read the family name out of an sfnt font's `name` table.
///
/// Handles bare TTF/OTF and the `ttc` collection wrapper (the first face is
/// read). The preferred record is the typographic family (name ID 16), falling
/// back to the legacy family (ID 1); the string is read as UTF-16BE from the
/// Windows/Microsoft records or Unicode platform, and as Latin-1 from the
/// Macintosh record, which is what the spec means by those encodings.
pub(crate) fn family_name_of_bytes(bytes: &[u8]) -> Option<String> {
    // A collection starts with 'ttcf' and carries an array of face offsets; a
    // bare font starts with its sfnt version (0x00010000, 'OTTO', or 'true').
    let face_offset = if bytes.len() >= 12 && &bytes[0..4] == b"ttcf" {
        u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize
    } else {
        0
    };
    let face = bytes.get(face_offset..)?;
    if face.len() < 12 {
        return None;
    }
    let num_tables = u16::from_be_bytes([face[4], face[5]]) as usize;
    let name_table = (0..num_tables)
        .filter_map(|i| {
            let rec = face.get(12 + i * 16..12 + i * 16 + 16)?;
            Some((&rec[0..4], u32::from_be_bytes(rec[8..12].try_into().ok()?)))
        })
        .find(|(tag, _)| *tag == b"name")
        .map(|(_, offset)| offset as usize)?;
    let name = face.get(name_table..)?;
    if name.len() < 6 {
        return None;
    }
    let count = u16::from_be_bytes([name[2], name[3]]) as usize;
    let strings_at = u16::from_be_bytes([name[4], name[5]]) as usize;
    // Records are ranked: typographic family beats legacy; a Unicode-encoded
    // record beats the Macintosh Latin-1 one. The best rank seen wins, and a
    // later record never overwrites an equal-or-better one.
    let mut best_rank = 0u8;
    let mut best_name: Option<String> = None;
    for i in 0..count {
        let rec = name.get(6 + i * 12..6 + i * 12 + 12)?;
        let platform = u16::from_be_bytes([rec[0], rec[1]]);
        let name_id = u16::from_be_bytes([rec[6], rec[7]]);
        let rank: u8 = match (name_id, platform) {
            (16, _) => 4,
            (1, 3) | (1, 0) => 3,
            (1, 1) => 2,
            _ => 0,
        };
        if rank <= best_rank {
            continue;
        }
        let encoding = u16::from_be_bytes([rec[2], rec[3]]);
        let length = u16::from_be_bytes([rec[8], rec[9]]) as usize;
        let offset = u16::from_be_bytes([rec[10], rec[11]]) as usize;
        let raw = name.get(strings_at + offset..strings_at + offset + length)?;
        let decoded = if platform == 1 && encoding == 0 {
            raw.iter().map(|&b| b as char).collect::<String>()
        } else {
            // UTF-16BE, two bytes per char — the Microsoft and Unicode records.
            raw.chunks_exact(2)
                .filter_map(|pair| {
                    char::from_u32(u16::from_be_bytes([pair[0], pair[1]]) as u32)
                })
                .collect::<String>()
        };
        let decoded = decoded.trim().to_string();
        if decoded.is_empty() {
            continue;
        }
        best_rank = rank;
        best_name = Some(decoded);
    }
    best_name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_mono_is_offered_first_and_only_once() {
        let families = available_families();
        assert_eq!(
            families.first().map(String::as_str),
            Some(BUILT_IN_MONO),
            "the picker leads with the bundled monospace default"
        );
        assert_eq!(
            families
                .iter()
                .filter(|name| *name == BUILT_IN_MONO)
                .count(),
            1,
            "a duplicate entry is a picker with two of the same row"
        );
    }

    /// The proportional faces ship in the binary for the text system at large,
    /// but a proportional face in a cell grid is the drift this module exists
    /// to prevent — they must never appear in the terminal picker.
    #[test]
    fn the_proportional_bundled_faces_are_not_terminal_candidates() {
        let families = available_families();
        for proportional in BUNDLED_PROPORTIONAL {
            assert!(
                !families.iter().any(|name| name == proportional),
                "{proportional} is proportional and must not be offered for the terminal"
            );
        }
    }

    /// The real shipped fonts, as their name tables read them: the terminal's
    /// default must be the face the binary registers under that very name.
    #[test]
    fn the_shipped_mono_reads_its_picker_name() {
        let bytes = include_bytes!("../../../assets/fonts/MeatshellMono-Regular.ttf");
        assert_eq!(family_name_of_bytes(bytes).as_deref(), Some(BUILT_IN_MONO));
    }

    #[test]
    fn a_truncated_font_is_rejected_not_panic() {
        let bytes = include_bytes!("../../../assets/fonts/MeatshellMono-Regular.ttf");
        assert_eq!(family_name_of_bytes(&bytes[..100]), None);
        assert_eq!(family_name_of_bytes(&[]), None);
    }
}
