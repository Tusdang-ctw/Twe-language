// Phase 12 session 2: asset bundling format v1.
//
// `.twebundle` is a flat, version-tagged binary archive that the
// Phase-12 build pipeline produces and the runtime later opens at
// startup. Format goals: random-access lookup by path, no external
// dependencies, fixed-size index entries for fast scan, no
// compression (session 8 adds optional zstd via a flags field).
//
// Layout:
//
//   [ 8 bytes  ] magic "TWEBUND1"
//   [ 4 bytes  ] format version u32 LE                   (= 1)
//   [ 4 bytes  ] flags u32 LE                            (= 0; reserved for session 8)
//   [ 4 bytes  ] entry count u32 LE
//   [ 4 bytes  ] body region offset u32 LE               (absolute, from file start)
//   [N entries ] index records (see below)
//   [ ...      ] concatenated bodies starting at body region offset
//
// Each index record:
//
//   [ 2 bytes  ] path length u16 LE                      (>= 1, <= 65535)
//   [ M bytes  ] path bytes (UTF-8, forward-slash canonical)
//   [ 8 bytes  ] body offset u64 LE                      (absolute, from file start)
//   [ 8 bytes  ] body length u64 LE
//
// No padding, no checksums. SHA256 / CRC32 ride session 8 alongside
// compression — both are storage features the format flags account
// for. Today's threat model is "did the bundle get truncated
// reading?" which the body-length field already covers.
//
// All multi-byte ints are little-endian. Bundle keys are case-
// preserving and case-sensitive — matching how forward-slash POSIX
// paths work, even on Windows where the host filesystem is case-
// insensitive.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

pub const MAGIC: &[u8; 8] = b"TWEBUND1";
pub const FORMAT_VERSION: u32 = 1;

/// Phase 12 session 4: footer magic for the self-extracting `.exe`
/// produced by `twec build`. The build pipeline copies a runtime
/// binary, appends a `.twebundle`, and writes a 24-byte footer:
///
///   [ 8 bytes ] bundle offset u64 LE  (absolute, where TWEBUND1 starts)
///   [ 8 bytes ] bundle length u64 LE
///   [ 8 bytes ] magic "TWEBOOT1"
///
/// At runtime, `detect_in_self` checks the last 24 bytes of the
/// running executable and, if the magic matches, opens the bundle
/// at the recorded offset.
pub const BOOT_MAGIC: &[u8; 8] = b"TWEBOOT1";
pub const BOOT_FOOTER_SIZE: u64 = 24;

/// One entry as it lives in the bundle index. The `body_offset` /
/// `body_length` are absolute offsets from the file start, so a
/// `BundleReader` can seek straight to a file body without rescanning
/// the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleEntry {
    pub path: String,
    pub body_offset: u64,
    pub body_length: u64,
}

/// Parsed bundle header — the index plus the body-region anchor.
#[derive(Clone, Debug)]
pub struct BundleHeader {
    pub version: u32,
    pub flags: u32,
    pub body_offset: u64,
    pub entries: Vec<BundleEntry>,
}

impl BundleHeader {
    pub fn find(&self, path: &str) -> Option<&BundleEntry> {
        self.entries.iter().find(|e| e.path == path)
    }
}

/// Encode a bundle from an in-memory list of (path, body) pairs. The
/// caller owns ordering — entries are written in iteration order;
/// the build pipeline sorts upstream so the on-disk order is
/// reproducible. Returns the number of bytes written.
pub fn encode<W: Write>(w: &mut W, files: &[(String, Vec<u8>)]) -> io::Result<u64> {
    if files.len() > u32::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many bundle entries (>= 4 billion)",
        ));
    }
    for (path, _) in files {
        if path.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "bundle entry path is empty",
            ));
        }
        if path.len() > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("bundle entry path too long ({} > 65535)", path.len()),
            ));
        }
    }

    // Compute the body region offset: 8 magic + 4 version + 4 flags
    // + 4 count + 4 body_offset + sum(per-entry index size).
    let header_size: u64 = 8 + 4 + 4 + 4 + 4;
    let mut index_size: u64 = 0;
    for (path, _) in files {
        // 2 path-len + path bytes + 8 body-offset + 8 body-length.
        index_size += 2 + path.len() as u64 + 8 + 8;
    }
    let body_offset = header_size + index_size;

    // Header.
    w.write_all(MAGIC)?;
    w.write_all(&FORMAT_VERSION.to_le_bytes())?;
    w.write_all(&0u32.to_le_bytes())?;
    w.write_all(&(files.len() as u32).to_le_bytes())?;
    w.write_all(&(body_offset as u32).to_le_bytes())?;

    // Index — assigns absolute body offsets ahead of writing any body.
    let mut cursor = body_offset;
    for (path, body) in files {
        w.write_all(&(path.len() as u16).to_le_bytes())?;
        w.write_all(path.as_bytes())?;
        w.write_all(&cursor.to_le_bytes())?;
        w.write_all(&(body.len() as u64).to_le_bytes())?;
        cursor = cursor.checked_add(body.len() as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "bundle body region overflow")
        })?;
    }

    // Bodies — concatenated, no padding.
    for (_, body) in files {
        w.write_all(body)?;
    }

    Ok(cursor)
}

/// Decode just the header (magic + index) from a stream. The body
/// region is left unread — `BundleReader` seeks into it on demand.
pub fn decode_header<R: Read>(r: &mut R) -> io::Result<BundleHeader> {
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)
        .map_err(|e| invalid("could not read bundle magic", e))?;
    if &magic != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a twebundle (magic mismatch)",
        ));
    }
    let version = read_u32(r, "version")?;
    if version != FORMAT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "twebundle format version {version} not supported (expected {FORMAT_VERSION})"
            ),
        ));
    }
    let flags = read_u32(r, "flags")?;
    let count = read_u32(r, "entry count")?;
    let body_offset = read_u32(r, "body offset")? as u64;

    let mut entries = Vec::with_capacity(count as usize);
    for i in 0..count {
        let path_len = read_u16(r, "path length")?;
        if path_len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bundle entry {i} has zero-length path"),
            ));
        }
        let mut path_bytes = vec![0u8; path_len as usize];
        r.read_exact(&mut path_bytes)
            .map_err(|e| invalid("path bytes truncated", e))?;
        let path = String::from_utf8(path_bytes).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bundle entry {i} has invalid UTF-8 path"),
            )
        })?;
        let body_off = read_u64(r, "body offset")?;
        let body_len = read_u64(r, "body length")?;
        entries.push(BundleEntry {
            path,
            body_offset: body_off,
            body_length: body_len,
        });
    }

    Ok(BundleHeader {
        version,
        flags,
        body_offset,
        entries,
    })
}

/// Random-access reader over a bundle on disk. Holds the file handle
/// open for the lifetime of the reader; cheap to keep around because
/// the index is small and lookups seek by absolute offset.
pub struct BundleReader {
    file: File,
    /// Path → (body_offset, body_length). HashMap for O(1) lookup —
    /// the original entry order is recoverable from `header.entries`
    /// if needed but the reader itself doesn't expose it.
    index: HashMap<String, (u64, u64)>,
    pub header: BundleHeader,
    /// When the bundle is embedded in a host file (session 4 self-
    /// extracting `.exe`), this is the offset within that file where
    /// the bundle starts. All reads add this to the bundle-internal
    /// offsets so the same encoder output works whether it's a
    /// standalone `.twebundle` or appended to a runtime binary.
    base_offset: u64,
}

impl BundleReader {
    /// Open a standalone `.twebundle` file.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        Self::from_file_at(file, 0)
    }

    /// Open a bundle that lives at a specific offset inside a larger
    /// file (session 4 path-redirected reader). The caller passes
    /// the absolute file offset where the magic bytes start.
    pub fn open_at(path: &Path, base_offset: u64) -> io::Result<Self> {
        let file = File::open(path)?;
        Self::from_file_at(file, base_offset)
    }

    fn from_file_at(mut file: File, base_offset: u64) -> io::Result<Self> {
        file.seek(SeekFrom::Start(base_offset))?;
        let header = decode_header(&mut file)?;
        let mut index = HashMap::with_capacity(header.entries.len());
        for e in &header.entries {
            index.insert(e.path.clone(), (e.body_offset, e.body_length));
        }
        Ok(Self {
            file,
            index,
            header,
            base_offset,
        })
    }

    pub fn has(&self, path: &str) -> bool {
        self.index.contains_key(path)
    }

    pub fn entry_count(&self) -> usize {
        self.header.entries.len()
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.index.keys().map(String::as_str)
    }

    /// Read a file body by path. Returns `None` if the path isn't in
    /// the index; an `Err` only on actual IO failure.
    pub fn read(&mut self, path: &str) -> io::Result<Option<Vec<u8>>> {
        let Some(&(off, len)) = self.index.get(path) else {
            return Ok(None);
        };
        self.file.seek(SeekFrom::Start(self.base_offset + off))?;
        let mut buf = vec![0u8; len as usize];
        self.file.read_exact(&mut buf)?;
        Ok(Some(buf))
    }
}

// Phase 12 session 4: self-extracting binary helpers. Build pipeline
// uses `append_to_binary` to glue a bundle onto a runtime exe;
// runtime startup uses `detect_in_self` to find one if it's there.

/// Copy a runtime executable into `out_path`, append `bundle_bytes`,
/// and write the 24-byte boot footer. Returns the absolute offset
/// in the output file where the bundle starts (== the runtime
/// length).
pub fn append_to_binary(
    runtime_path: &Path,
    bundle_bytes: &[u8],
    out_path: &Path,
) -> io::Result<u64> {
    let runtime = std::fs::read(runtime_path)?;
    let runtime_len = runtime.len() as u64;
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut out = File::create(out_path)?;
    out.write_all(&runtime)?;
    let bundle_offset = runtime_len;
    out.write_all(bundle_bytes)?;
    out.write_all(&bundle_offset.to_le_bytes())?;
    out.write_all(&(bundle_bytes.len() as u64).to_le_bytes())?;
    out.write_all(BOOT_MAGIC)?;
    out.flush()?;
    // Preserve the executable bit on Unix so the produced file is
    // launchable. Windows ignores file mode for executability.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(out_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(out_path, perms)?;
    }
    Ok(bundle_offset)
}

/// Probe the running executable for an embedded bundle. Returns
/// `Ok(None)` when no footer is present (the binary is a plain
/// `twec.exe`); `Err` only when the executable path itself can't
/// be read or the footer is malformed in a way that suggests
/// corruption.
pub fn detect_in_self() -> io::Result<Option<BundleReader>> {
    let path = std::env::current_exe()?;
    detect_in_file(&path)
}

pub fn detect_in_file(path: &Path) -> io::Result<Option<BundleReader>> {
    let mut file = File::open(path)?;
    let len = file.seek(SeekFrom::End(0))?;
    if len < BOOT_FOOTER_SIZE {
        return Ok(None);
    }
    file.seek(SeekFrom::End(-(BOOT_FOOTER_SIZE as i64)))?;
    let mut footer = [0u8; BOOT_FOOTER_SIZE as usize];
    file.read_exact(&mut footer)?;
    if &footer[16..24] != BOOT_MAGIC {
        return Ok(None);
    }
    let bundle_offset = u64::from_le_bytes(footer[0..8].try_into().unwrap());
    let bundle_length = u64::from_le_bytes(footer[8..16].try_into().unwrap());
    // Sanity-check the recorded region is inside the file.
    if bundle_offset + bundle_length + BOOT_FOOTER_SIZE > len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "embedded bundle footer points outside the host file",
        ));
    }
    drop(file);
    let reader = BundleReader::open_at(path, bundle_offset)?;
    Ok(Some(reader))
}

// Phase 12 session 3: process-global active-bundle slot. The
// runtime sets this once at startup (session 4 wires it from the
// self-extracting `.exe` path; tests + future explicit `--bundle`
// flags can install one directly). Stdlib loaders that previously
// hit the filesystem now route through `read_asset_bytes`, which
// tries the bundle first and falls back to disk.
//
// `Mutex<Option<...>>` over `OnceLock<Mutex<...>>` so tests can
// install + clear repeatedly. The runtime is single-threaded so
// contention is theoretical, but holding the mutex across a read
// keeps the file cursor consistent if a future session ever runs
// loaders off the play loop's thread.
static ACTIVE_BUNDLE: Mutex<Option<BundleReader>> = Mutex::new(None);

/// Install a bundle as the process's active asset source. Replaces
/// any previously-installed bundle.
pub fn set_active_bundle(reader: BundleReader) {
    let mut guard = ACTIVE_BUNDLE.lock().expect("active bundle mutex poisoned");
    *guard = Some(reader);
}

/// Drop any installed active bundle. After this returns,
/// `read_asset_bytes` falls through to the filesystem only.
pub fn clear_active_bundle() {
    let mut guard = ACTIVE_BUNDLE.lock().expect("active bundle mutex poisoned");
    *guard = None;
}

/// True when a bundle is installed.
pub fn has_active_bundle() -> bool {
    ACTIVE_BUNDLE
        .lock()
        .map(|g| g.is_some())
        .unwrap_or(false)
}

/// Resolve a path, trying the active bundle first and then the
/// filesystem. The fallback preserves behavior for scripts run via
/// `twec play` / `twec run` outside a bundle, and for paths the
/// bundle doesn't include (e.g. assets the dev hasn't moved into
/// `assets/` yet).
pub fn read_asset_bytes(path: &str) -> io::Result<Vec<u8>> {
    {
        let mut guard = ACTIVE_BUNDLE
            .lock()
            .expect("active bundle mutex poisoned");
        if let Some(reader) = guard.as_mut() {
            if let Some(bytes) = reader.read(path)? {
                return Ok(bytes);
            }
        }
    }
    std::fs::read(path)
}

fn read_u16<R: Read>(r: &mut R, ctx: &str) -> io::Result<u16> {
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf)
        .map_err(|e| invalid(&format!("could not read {ctx}"), e))?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32<R: Read>(r: &mut R, ctx: &str) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)
        .map_err(|e| invalid(&format!("could not read {ctx}"), e))?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64<R: Read>(r: &mut R, ctx: &str) -> io::Result<u64> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf)
        .map_err(|e| invalid(&format!("could not read {ctx}"), e))?;
    Ok(u64::from_le_bytes(buf))
}

fn invalid(msg: &str, src: io::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{msg}: {src}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip_three_files() {
        let files = vec![
            ("main.twe".to_string(), b"print(\"hi\")\n".to_vec()),
            ("assets/walk.png".to_string(), vec![0x89, 0x50, 0x4e, 0x47]),
            ("assets/snd.ogg".to_string(), b"OggS".to_vec()),
        ];
        let mut buf = Vec::new();
        let total = encode(&mut buf, &files).expect("encode");
        assert_eq!(total as usize, buf.len());

        let mut cursor = Cursor::new(&buf);
        let header = decode_header(&mut cursor).expect("decode");
        assert_eq!(header.version, FORMAT_VERSION);
        assert_eq!(header.flags, 0);
        assert_eq!(header.entries.len(), 3);
        assert_eq!(header.entries[0].path, "main.twe");
        assert_eq!(header.entries[0].body_length, 12);
        assert_eq!(header.entries[1].body_length, 4);
    }

    #[test]
    fn reader_round_trips_bodies() {
        let dir = std::env::temp_dir().join(format!(
            "twec_bundle_round_{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let bundle_path = dir.join("test.twebundle");
        let files = vec![
            ("main.twe".to_string(), b"print(1)\n".to_vec()),
            ("assets/data.bin".to_string(), (0u8..=255u8).collect()),
        ];
        {
            let mut f = File::create(&bundle_path).expect("create");
            encode(&mut f, &files).expect("encode");
        }
        let mut reader = BundleReader::open(&bundle_path).expect("open");
        assert_eq!(reader.entry_count(), 2);
        assert!(reader.has("main.twe"));
        assert!(reader.has("assets/data.bin"));
        assert!(!reader.has("missing"));
        let main = reader.read("main.twe").unwrap().expect("present");
        assert_eq!(main, b"print(1)\n");
        let data = reader.read("assets/data.bin").unwrap().expect("present");
        assert_eq!(data.len(), 256);
        assert_eq!(data[0], 0);
        assert_eq!(data[255], 255);
        assert!(reader.read("nope").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reader_works_at_nonzero_base_offset() {
        // Session 4 path: bundle appended to a host file. We prepend
        // a fake "runtime header" of arbitrary bytes and verify the
        // reader still resolves bodies through `base_offset`.
        let dir = std::env::temp_dir().join(format!(
            "twec_bundle_offset_{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let host_path = dir.join("hosted.bin");
        let prefix: Vec<u8> = (0u8..123u8).collect();
        let files = vec![("greeting.twe".to_string(), b"print(\"yo\")\n".to_vec())];
        let bundle_offset;
        {
            let mut f = File::create(&host_path).expect("create");
            f.write_all(&prefix).unwrap();
            bundle_offset = f.stream_position().unwrap();
            encode(&mut f, &files).expect("encode");
        }
        let mut reader = BundleReader::open_at(&host_path, bundle_offset).expect("open");
        let body = reader.read("greeting.twe").unwrap().expect("present");
        assert_eq!(body, b"print(\"yo\")\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_non_bundle_input() {
        let mut cursor = Cursor::new(b"not a bundle".to_vec());
        let err = decode_header(&mut cursor).expect_err("must reject");
        assert!(err.to_string().contains("magic"), "got: {err}");
    }

    #[test]
    fn rejects_wrong_version() {
        // Hand-craft a header with version 99.
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&99u32.to_le_bytes()); // version
        buf.extend_from_slice(&0u32.to_le_bytes()); // flags
        buf.extend_from_slice(&0u32.to_le_bytes()); // count
        buf.extend_from_slice(&24u32.to_le_bytes()); // body offset
        let mut cursor = Cursor::new(buf);
        let err = decode_header(&mut cursor).expect_err("must reject");
        assert!(err.to_string().contains("version"), "got: {err}");
    }

    #[test]
    fn rejects_empty_path() {
        let files = vec![("".to_string(), b"x".to_vec())];
        let mut buf = Vec::new();
        let err = encode(&mut buf, &files).expect_err("must reject");
        assert!(err.to_string().contains("empty"), "got: {err}");
    }
}
