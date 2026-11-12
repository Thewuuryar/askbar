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
