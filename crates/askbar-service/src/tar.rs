//! A minimal ustar archive of regular files, for backup exports: the server's `control.db` and
//! space files, and the admin's bundle of them with its own database. Writes and reads only what
//! these exports hold: regular files, names up to 100 bytes, no links or directories.

/// The 512-byte header of a regular file.
fn header(name: &str, size: u64, mtime: i64) -> [u8; 512] {
    debug_assert!(is_plain_name(name), "{name}");
    let mut h = [0u8; 512];
    h[..name.len()].copy_from_slice(name.as_bytes());
    let put = |h: &mut [u8; 512], at: usize, s: String| h[at..at + s.len()].copy_from_slice(s.as_bytes());
    put(&mut h, 100, "0000644\0".into());
    put(&mut h, 108, "0000000\0".into());
    put(&mut h, 116, "0000000\0".into());
    put(&mut h, 124, format!("{size:011o}\0"));
    put(&mut h, 136, format!("{:011o}\0", mtime.max(0)));
    h[148..156].copy_from_slice(b"        ");
    h[156] = b'0';
    put(&mut h, 257, "ustar\0".into());
    put(&mut h, 263, "00".into());
    let sum: u32 = h.iter().map(|b| *b as u32).sum();
    put(&mut h, 148, format!("{sum:06o}\0 "));
    h
}

/// Appends one file to `out`.
pub fn append(out: &mut Vec<u8>, name: &str, data: &[u8], mtime: i64) {
    out.extend_from_slice(&header(name, data.len() as u64, mtime));
    out.extend_from_slice(data);
    out.resize(out.len().div_ceil(512) * 512, 0);
}

/// Writes one file to `out` as it is read: the header (which needs `size`, so the caller says it), exactly `size` bytes from
/// `reader`, and the padding. Nothing is held in memory but one buffer, so a large file can go to a stream. A reader that gives
/// more or fewer bytes than `size` is an error: the archive would not be one.
pub fn write_file<W: std::io::Write, R: std::io::Read>(
    out: &mut W,
    name: &str,
    size: u64,
    reader: &mut R,
    mtime: i64,
) -> std::io::Result<()> {
    out.write_all(&header(name, size, mtime))?;
    let mut left = size;
    let mut buf = [0u8; 64 * 1024];
    while left > 0 {
        let want = buf.len().min(left as usize);
        let got = reader.read(&mut buf[..want])?;
        if got == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "a file was shorter than its size"));
        }
        out.write_all(&buf[..got])?;
        left -= got as u64;
    }
    if reader.read(&mut [0u8; 1])? != 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "a file was longer than its size"));
    }
    out.write_all(&[0u8; 512][..((512 - size % 512) % 512) as usize])
}

/// [`write_file`] for bytes in memory.
pub fn write_bytes<W: std::io::Write>(out: &mut W, name: &str, data: &[u8], mtime: i64) -> std::io::Result<()> {
    write_file(out, name, data.len() as u64, &mut &data[..], mtime)
}

/// Ends an archive written with [`write_file`] (two empty blocks).
pub fn write_end<W: std::io::Write>(out: &mut W) -> std::io::Result<()> {
    out.write_all(&[0u8; 1024])
}

/// Ends the archive (two empty blocks).
pub fn finish(out: &mut Vec<u8>) {
    out.extend_from_slice(&[0u8; 1024]);
}

/// A relative name of plain segments: no `..`, no leading `/`, no empty segment, at most 100 bytes.
pub fn is_plain_name(name: &str) -> bool {
    name.len() <= 100 && !name.is_empty() && name.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// One regular file's place in an archive: its name, where its data starts and how long it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

/// What a header says: `None` for the end of the archive (an empty block), else the name and size. A header that is not a regular
/// ustar file with a plain name is an error: a restore reads only what a backup writes.
fn parse_header(h: &[u8; 512]) -> std::io::Result<Option<(String, u64)>> {
    let bad = |what: &'static str| std::io::Error::new(std::io::ErrorKind::InvalidData, what);
    if h.iter().all(|b| *b == 0) {
        return Ok(None);
    }
    if &h[257..262] != b"ustar" || h[156] != b'0' {
        return Err(bad("not an archive of regular files"));
    }
    let name = std::str::from_utf8(&h[..100]).map_err(|_| bad("a name that is not text"))?.trim_end_matches('\0').to_string();
    let size = std::str::from_utf8(&h[124..135])
        .ok()
        .map(|t| t.trim_matches(|c| c == '\0' || c == ' '))
        .and_then(|t| u64::from_str_radix(t, 8).ok())
        .ok_or_else(|| bad("a size that is not a number"))?;
    if !is_plain_name(&name) {
        return Err(bad("a name that is not plain"));
    }
    Ok(Some((name, size)))
}

/// The files of an archive on disk, by place, without reading their data: for a bundle too large to hold in memory, whose parts
/// are read one at a time with a seek. An archive that ends without its end marker, or whose last file runs past the end of the
/// input, is an error (a download that stopped early is never taken for a whole one).
pub fn index<R: std::io::Read + std::io::Seek>(r: &mut R) -> std::io::Result<Vec<Entry>> {
    use std::io::SeekFrom;
    let total = r.seek(SeekFrom::End(0))?;
    r.seek(SeekFrom::Start(0))?;
    let mut out = Vec::new();
    let mut at = 0u64;
    loop {
        if at + 512 > total {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the archive has no end"));
        }
        r.seek(SeekFrom::Start(at))?;
        let mut h = [0u8; 512];
        r.read_exact(&mut h)?;
        let Some((name, size)) = parse_header(&h)? else { return Ok(out) };
        let offset = at + 512;
        if offset + size > total {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "a file runs past the end of the archive"));
        }
        out.push(Entry { name, offset, size });
        at = offset + size.div_ceil(512) * 512;
    }
}

/// Reads an archive as it arrives, calling `f` with each file's name, size and a reader of exactly its data (what `f` leaves
/// unread is skipped). Nothing is held in memory but one buffer. An archive that ends before its end marker, or in the middle of
/// a file, is an error: a stream that was cut short is never taken for a whole one.
pub fn for_each<R: std::io::Read>(
    r: &mut R,
    mut f: impl FnMut(&str, u64, &mut dyn std::io::Read) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::io::Read;
    loop {
        let mut h = [0u8; 512];
        r.read_exact(&mut h)?;
        let Some((name, size)) = parse_header(&h)? else { return Ok(()) };
        let mut data = r.by_ref().take(size);
        f(&name, size, &mut data)?;
        // What the callback left, then the padding up to the block.
        std::io::copy(&mut data, &mut std::io::sink())?;
        if data.limit() != 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "a file was cut short"));
        }
        let pad = (512 - size % 512) % 512;
        std::io::copy(&mut r.by_ref().take(pad), &mut std::io::sink()).and_then(|n| {
            if n == pad {
                Ok(())
            } else {
                Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the archive was cut short"))
            }
        })?;
    }
}

/// The files in an archive `append` wrote, in order. `None` if it is not one (a bad header, a
/// name that is not plain, or a file that runs past the end).
pub fn read(bytes: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 512 <= bytes.len() {
        let h = &bytes[at..at + 512];
        if h.iter().all(|b| *b == 0) {
            return Some(out);
        }
        if &h[257..262] != b"ustar" || h[156] != b'0' {
            return None;
        }
        let name = std::str::from_utf8(&h[..100]).ok()?.trim_end_matches('\0').to_string();
        let size = usize::from_str_radix(std::str::from_utf8(&h[124..135]).ok()?.trim_matches(|c| c == '\0' || c == ' '), 8).ok()?;
        if !is_plain_name(&name) || at + 512 + size > bytes.len() {
            return None;
        }
        out.push((name, bytes[at + 512..at + 512 + size].to_vec()));
        at += 512 + size.div_ceil(512) * 512;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_appended_reads_back_in_order() {
        let mut t = Vec::new();
        append(&mut t, "control.db", b"one", 0);
        append(&mut t, "spaces/01J0000000000000000000000.db", &vec![7u8; 1500], 1_700_000_000);
        append(&mut t, "empty", b"", 0);
        finish(&mut t);
        assert_eq!(t.len() % 512, 0);
        let files = read(&t).unwrap();
        assert_eq!(
            files.iter().map(|(n, d)| (n.as_str(), d.len())).collect::<Vec<_>>(),
            [("control.db", 3), ("spaces/01J0000000000000000000000.db", 1500), ("empty", 0)]
        );
        assert_eq!(files[0].1, b"one");
    }

    #[test]
    fn an_archive_is_indexed_and_streamed_and_one_that_stops_early_is_an_error() {
        let mut t = Vec::new();
        append(&mut t, "a.db", &vec![1u8; 700], 0);
        append(&mut t, "dir/b.db", b"bee", 0);
        append(&mut t, "empty", b"", 0);
        finish(&mut t);
        let entries = index(&mut std::io::Cursor::new(&t)).unwrap();
        assert_eq!(entries.iter().map(|e| (e.name.as_str(), e.size)).collect::<Vec<_>>(), [("a.db", 700), ("dir/b.db", 3), ("empty", 0)]);
        assert_eq!(&t[entries[1].offset as usize..][..3], b"bee");
        // Streamed: each file's data, and a callback that reads nothing still moves on to the next file.
        let mut seen = Vec::new();
        for_each(&mut &t[..], |name, size, r| {
            let mut data = Vec::new();
            if name != "a.db" {
                std::io::Read::read_to_end(r, &mut data)?;
            }
            seen.push((name.to_string(), size, data.len()));
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, [("a.db".into(), 700, 0), ("dir/b.db".into(), 3, 3), ("empty".into(), 0, 0)]);
        // Cut anywhere before the first empty block (the end), both ways refuse.
        for cut in [0, 100, 512, 600, 1024 + 512, t.len() - 1024, t.len() - 1024 + 100] {
            assert!(index(&mut std::io::Cursor::new(&t[..cut])).is_err(), "indexed a cut at {cut}");
            assert!(for_each(&mut &t[..cut], |_, _, _| Ok(())).is_err(), "streamed a cut at {cut}");
        }
        let mut evil = t.clone();
        evil[..4].copy_from_slice(b"../a");
        assert!(index(&mut std::io::Cursor::new(&evil)).is_err() && for_each(&mut &evil[..], |_, _, _| Ok(())).is_err());
    }

    #[test]
    fn names_must_be_plain_and_damaged_archives_are_refused() {
        for bad in ["", "/etc/passwd", "../x", "a/../b", "a//b", "./a", &"x".repeat(101)] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
        assert!(is_plain_name("server/spaces/a.db"));
        let mut t = Vec::new();
        append(&mut t, "a", b"hello", 0);
        finish(&mut t);
        assert!(read(&t[..600]).is_none(), "a file that runs past the end");
        let mut bad = t.clone();
        bad[257] = b'x';
        assert!(read(&bad).is_none(), "not ustar");
        let mut evil = t.clone();
        evil[..4].copy_from_slice(b"../a");
        assert!(read(&evil).is_none(), "a name that climbs out");
        assert!(read(b"").is_none(), "no end marker");
    }
}
