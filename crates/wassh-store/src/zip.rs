//! A tiny store-only ZIP writer and reader.
//!
//! Backups are JSON files, which barely compress, so the archive stores them verbatim: no
//! deflate, no third-party crate, and archive contents stay readable with any unzip tool.

/// Signature of a local file header.
const LOCAL_HEADER: u32 = 0x0403_4b50;
/// Signature of a central directory entry.
const CENTRAL_HEADER: u32 = 0x0201_4b50;
/// Signature of the end-of-central-directory record.
const END_OF_DIRECTORY: u32 = 0x0605_4b50;
/// Version we claim to need: 2.0, which every unzip understands.
const VERSION: u16 = 20;
/// Method 0: stored.
const METHOD_STORE: u16 = 0;
/// Largest comment the end record can hold.
const MAX_COMMENT: usize = 65_535;

/// CRC-32 (IEEE) of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Builds an archive in memory.
#[derive(Default)]
pub struct ZipWriter {
    entries: Vec<(String, Vec<u8>)>,
}

impl ZipWriter {
    /// A new, empty archive.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `data` under `name` (forward slashes; directories are implied by the name).
    pub fn add(&mut self, name: &str, data: Vec<u8>) {
        self.entries.push((name.to_owned(), data));
    }

    /// Number of files added so far.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing was added.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Serialise the archive.
    pub fn finish(self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut directory = Vec::new();

        for (name, data) in &self.entries {
            let name_bytes = name.as_bytes();
            let crc = crc32(data);
            let offset = out.len() as u32;

            // Local file header.
            push_u32(&mut out, LOCAL_HEADER);
            push_u16(&mut out, VERSION);
            push_u16(&mut out, 0); // flags
            push_u16(&mut out, METHOD_STORE);
            push_u16(&mut out, 0); // modification time
            push_u16(&mut out, 0); // modification date
            push_u32(&mut out, crc);
            push_u32(&mut out, data.len() as u32);
            push_u32(&mut out, data.len() as u32);
            push_u16(&mut out, name_bytes.len() as u16);
            push_u16(&mut out, 0); // extra length
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(data);

            // Central directory entry.
            push_u32(&mut directory, CENTRAL_HEADER);
            push_u16(&mut directory, VERSION); // made by
            push_u16(&mut directory, VERSION); // needed
            push_u16(&mut directory, 0);
            push_u16(&mut directory, METHOD_STORE);
            push_u16(&mut directory, 0);
            push_u16(&mut directory, 0);
            push_u32(&mut directory, crc);
            push_u32(&mut directory, data.len() as u32);
            push_u32(&mut directory, data.len() as u32);
            push_u16(&mut directory, name_bytes.len() as u16);
            push_u16(&mut directory, 0); // extra
            push_u16(&mut directory, 0); // comment
            push_u16(&mut directory, 0); // disk
            push_u16(&mut directory, 0); // internal attributes
            push_u32(&mut directory, 0); // external attributes
            push_u32(&mut directory, offset);
            directory.extend_from_slice(name_bytes);
        }

        let directory_offset = out.len() as u32;
        let directory_size = directory.len() as u32;
        out.extend_from_slice(&directory);

        // End of central directory.
        push_u32(&mut out, END_OF_DIRECTORY);
        push_u16(&mut out, 0); // this disk
        push_u16(&mut out, 0); // disk with the directory
        push_u16(&mut out, self.entries.len() as u16);
        push_u16(&mut out, self.entries.len() as u16);
        push_u32(&mut out, directory_size);
        push_u32(&mut out, directory_offset);
        push_u16(&mut out, 0); // comment length
        out
    }
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let slice = bytes.get(at..at + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Read the archive, returning `(name, data)` pairs.
///
/// Only stored entries are accepted: writing compressed archives is not supported, and a
/// silently mis-read backup would be worse than an error.
pub fn read(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let end = find_end_record(bytes).ok_or_else(|| "not a zip archive".to_owned())?;
    let count = read_u16(bytes, end + 10).ok_or("truncated directory")? as usize;
    let directory_offset = read_u32(bytes, end + 16).ok_or("truncated directory")? as usize;

    let mut entries = Vec::with_capacity(count);
    let mut cursor = directory_offset;
    for _ in 0..count {
        if read_u32(bytes, cursor) != Some(CENTRAL_HEADER) {
            return Err("damaged central directory".to_owned());
        }
        let method = read_u16(bytes, cursor + 10).ok_or("truncated entry")?;
        let crc = read_u32(bytes, cursor + 16).ok_or("truncated entry")?;
        let size = read_u32(bytes, cursor + 24).ok_or("truncated entry")? as usize;
        let name_length = read_u16(bytes, cursor + 28).ok_or("truncated entry")? as usize;
        let extra_length = read_u16(bytes, cursor + 30).ok_or("truncated entry")? as usize;
        let comment_length = read_u16(bytes, cursor + 32).ok_or("truncated entry")? as usize;
        let local_offset = read_u32(bytes, cursor + 42).ok_or("truncated entry")? as usize;
        let name_bytes = bytes
            .get(cursor + 46..cursor + 46 + name_length)
            .ok_or("truncated name")?;
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        cursor += 46 + name_length + extra_length + comment_length;

        if method != METHOD_STORE {
            return Err(format!("{name}: compressed entries are not supported"));
        }
        if read_u32(bytes, local_offset) != Some(LOCAL_HEADER) {
            return Err(format!("{name}: bad local header"));
        }
        let local_name = read_u16(bytes, local_offset + 26).ok_or("truncated header")? as usize;
        let local_extra = read_u16(bytes, local_offset + 28).ok_or("truncated header")? as usize;
        let start = local_offset + 30 + local_name + local_extra;
        let data = bytes
            .get(start..start + size)
            .ok_or_else(|| format!("{name}: truncated data"))?
            .to_vec();
        if crc32(&data) != crc {
            return Err(format!("{name}: checksum mismatch"));
        }
        entries.push((name, data));
    }
    Ok(entries)
}

/// Find the end-of-central-directory record, which may be followed by a comment.
fn find_end_record(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 22 {
        return None;
    }
    let start = bytes.len().saturating_sub(22 + MAX_COMMENT);
    (start..=bytes.len() - 22)
        .rev()
        .find(|offset| read_u32(bytes, *offset) == Some(END_OF_DIRECTORY))
}

/// Whether `name` stays inside the extraction directory.
///
/// Backups are user data from another machine, so archive-slip attempts are rejected rather
/// than trusted.
pub fn is_safe_entry_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.starts_with('\\')
        && !name.contains(':')
        && name
            .split(['/', '\\'])
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_the_reference_values() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"hello"), 0x3610_A686);
    }

    #[test]
    fn an_archive_round_trips() {
        let mut writer = ZipWriter::new();
        writer.add("hosts.json", b"{\"hosts\":[]}".to_vec());
        writer.add("nested/settings.json", b"{}".to_vec());
        assert_eq!(writer.len(), 2);
        let bytes = writer.finish();

        let entries = read(&bytes).expect("readable archive");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "hosts.json");
        assert_eq!(entries[0].1, b"{\"hosts\":[]}".to_vec());
        assert_eq!(entries[1].0, "nested/settings.json");
        assert_eq!(entries[1].1, b"{}".to_vec());
    }

    #[test]
    fn an_empty_archive_is_valid() {
        let bytes = ZipWriter::new().finish();
        assert_eq!(read(&bytes).expect("readable").len(), 0);
    }

    #[test]
    fn a_damaged_archive_is_rejected() {
        assert!(read(b"not a zip at all").is_err());
        let mut bytes = ZipWriter::new().finish();
        bytes.truncate(bytes.len() - 1);
        assert!(read(&bytes).is_err());
    }

    #[test]
    fn a_corrupted_payload_is_caught_by_the_checksum() {
        let mut writer = ZipWriter::new();
        writer.add("hosts.json", b"payload".to_vec());
        let mut bytes = writer.finish();
        // Flip a byte inside the stored data.
        let index = bytes
            .windows(7)
            .position(|window| window == b"payload")
            .expect("data");
        bytes[index] = b'P';
        let error = read(&bytes).expect_err("checksum mismatch");
        assert!(error.contains("checksum"), "{error}");
    }

    #[test]
    fn archive_names_cannot_escape_the_target_directory() {
        for safe in ["hosts.json", "plugins/a.json", "a-b_c.json"] {
            assert!(is_safe_entry_name(safe), "{safe:?}");
        }
        for unsafe_name in [
            "",
            "/etc/passwd",
            "../secrets.json",
            "a/../../b",
            "C:/windows",
            "nested//empty",
        ] {
            assert!(!is_safe_entry_name(unsafe_name), "{unsafe_name:?}");
        }
    }
}
