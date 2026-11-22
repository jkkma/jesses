//! Restore an exact sample aspect ratio in the MP4 muxer's existing fields.
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Range;

#[derive(Clone, Copy)]
struct Atom {
    tag: [u8; 4],
    start: u64,
    end: u64,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn read_at<const N: usize>(file: &mut (impl Read + Seek), start: u64) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn next_atom(file: &mut (impl Read + Seek), range: &mut Range<u64>) -> io::Result<Option<Atom>> {
    if range.is_empty() {
        return Ok(None);
    }
    let start = range.start;
    let remaining = range.end - start;
    if remaining < 8 {
        return Err(invalid("Incomplete MP4 atom header."));
    }
    let bytes = read_at::<8>(file, start)?;
    let short_size = u32::from_be_bytes(bytes[..4].try_into().unwrap());
    let (size, header) = match short_size {
        0 => (remaining, 8),
        1 if remaining >= 16 => (u64::from_be_bytes(read_at(file, start + 8)?), 16),
        1 => return Err(invalid("Incomplete extended MP4 atom header.")),
        _ => (u64::from(short_size), 8),
    };
    if !(header..=remaining).contains(&size) {
        return Err(invalid("Invalid MP4 atom length."));
    }
    range.start += size;
    Ok(Some(Atom {
        tag: bytes[4..].try_into().unwrap(),
        start: start + header,
        end: range.start,
    }))
}

fn find(
    file: &mut (impl Read + Seek),
    mut range: Range<u64>,
    tag: &[u8; 4],
    mut index: u32,
) -> io::Result<Atom> {
    while let Some(atom) = next_atom(file, &mut range)? {
        if &atom.tag == tag {
            if index == 0 {
                return Ok(atom);
            }
            index -= 1;
        }
    }
    Err(invalid(
        "The expected track or aspect metadata is missing from the MP4.",
    ))
}

fn fixed_display_size(width: u16, height: u16, sar: (u32, u32)) -> [u32; 2] {
    let width = u128::from(width) * u128::from(sar.0) * 65536 / u128::from(sar.1);
    let height = u128::from(height) * 65536;
    let limit = u128::from(u32::MAX);
    let scale = width.max(height).max(limit);
    // Keep both dimensions representable in tkhd's 16.16 fields. The pasp
    // rational below remains exact, including for unusually wide displays.
    [
        (width * limit / scale).max(1) as u32,
        (height * limit / scale).max(1) as u32,
    ]
}

pub(super) fn restore(
    file: &mut (impl Read + Write + Seek),
    index: u32,
    sar: (u32, u32),
) -> io::Result<()> {
    let end = file.seek(SeekFrom::End(0))?;
    let moov = find(file, 0..end, b"moov", 0)?;
    let trak = find(file, moov.start..moov.end, b"trak", index)?;
    let tkhd = find(file, trak.start..trak.end, b"tkhd", 0)?;
    let mdia = find(file, trak.start..trak.end, b"mdia", 0)?;
    let minf = find(file, mdia.start..mdia.end, b"minf", 0)?;
    let stbl = find(file, minf.start..minf.end, b"stbl", 0)?;
    let stsd = find(file, stbl.start..stbl.end, b"stsd", 0)?;
    // stsd starts with full-box flags and an entry count. A visual sample
    // entry has a 78-byte fixed header before its child boxes.
    let av1 = find(file, stsd.start + 8..stsd.end, b"av01", 0)?;
    let pasp = find(file, av1.start + 78..av1.end, b"pasp", 0)?;
    if pasp.end - pasp.start != 8
        || !matches!(
            (read_at::<1>(file, tkhd.start)?[0], tkhd.end - tkhd.start),
            (0, 84) | (1, 96)
        )
    {
        return Err(invalid("Invalid MP4 aspect metadata layout."));
    }
    let [w0, w1, h0, h1] = read_at::<4>(file, av1.start + 24)?;
    let width = u16::from_be_bytes([w0, w1]);
    let height = u16::from_be_bytes([h0, h1]);
    if width == 0 || height == 0 || sar.0 == 0 || sar.1 == 0 {
        return Err(invalid(
            "Invalid MP4 picture dimensions or sample aspect ratio.",
        ));
    }
    let [width, height] = fixed_display_size(width, height, sar);
    file.seek(SeekFrom::Start(pasp.start))?;
    file.write_all(&sar.0.to_be_bytes())?;
    file.write_all(&sar.1.to_be_bytes())?;
    file.seek(SeekFrom::Start(tkhd.end - 8))?;
    file.write_all(&width.to_be_bytes())?;
    file.write_all(&height.to_be_bytes())?;
    Ok(())
}
