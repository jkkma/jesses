/// Tracks the high-water frame counts from AV1AN's two Indicatif templates.
/// Their elapsed time, speed and ETA keep changing on idle redraws.
#[derive(Default)]
pub(super) struct Progress {
    encoded: u64,
    scanned: u64,
}

impl Progress {
    pub(super) fn advances(&mut self, record: &[u8]) -> bool {
        let Ok(record) = std::str::from_utf8(record) else {
            return false;
        };
        let (previous, current) = if let Some(position) = encode_position(record) {
            (&mut self.encoded, position)
        } else if let Some(position) = scene_position(record) {
            (&mut self.scanned, position)
        } else {
            return false;
        };
        let advanced = current > *previous;
        *previous = (*previous).max(current);
        advanced
    }
}

fn encode_position(record: &str) -> Option<u64> {
    // Aggregate encode row after the closing bar glyph: "5% 7/100 (…" .
    // A ConPTY record can also contain several worker rows before that glyph.
    let (_, tail) = record.rsplit_once('▌')?;
    let mut tokens = tail.split_ascii_whitespace();
    let percent = tokens.next()?.strip_suffix('%')?.parse::<u8>().ok()?;
    let (position, total) = tokens.next()?.split_once('/')?;
    let position = position.parse().ok()?;
    let total = total.parse().ok()?;
    (percent <= 100 && total > 0 && position <= total && tokens.next()?.starts_with('('))
        .then_some(position)
}

fn scene_position(record: &str) -> Option<u64> {
    // The scene scanner has a square-bracketed bar and an absolute count.
    let (head, tail) = record.rsplit_once(']')?;
    let (_, bar) = head.rsplit_once('[')?;
    if bar.is_empty()
        || !bar.chars().all(|c| {
            matches!(
                c,
                ' ' | '█' | '▓' | '▒' | '░' | '▉' | '▊' | '▋' | '▌' | '▍' | '▎' | '▏'
            )
        })
    {
        return None;
    }
    let mut tokens = tail.split_ascii_whitespace();
    let position = tokens.next()?.parse().ok()?;
    (tokens.next()? == "frames" && tokens.next()?.starts_with('(')).then_some(position)
}

#[cfg(test)]
mod tests {
    use super::Progress;

    #[test]
    fn encode_and_scene_positions_advance_independently() {
        let mut progress = Progress::default();
        for (record, expected) in [
            ("00:00:00 [██      ] 0 frames (0 fps)", false),
            ("00:00:01 [ ██     ] 5 frames (5 fps)", true),
            ("00:00:02 [  ██    ] 5 frames (2.5 fps)", false),
            ("00:00:03 [   ██   ] 6 frames (2 fps)", true),
            ("[Chunk 12] 8 frames: 3 fps", false),
            ("[Chunks] ▐██░░▌ 5% 1/100 (1 fps, eta 9m)", true),
            ("[Chunks] ▐██░░▌ 5% 1/120 (0.2 fps, eta 5m)", false),
            ("[Chunks] ▐██░░▌ 5% 0/120 (0 fps, eta unknown)", false),
            ("[Chunks] ▐██░░▌ 5% 1/120 (0.2 fps, eta 5m)", false),
            (
                "[Chunk 1] 8 frames: 3 fps   [0/4 Chunks] ▐██░░▌ 5% 7/100 (3 fps, eta 3m)",
                true,
            ),
            ("[Chunks] ▐██░░▌ 5% 1000/120 (9 fps, eta 0s)", false),
        ] {
            assert_eq!(progress.advances(record.as_bytes()), expected, "{record}");
        }
    }
}
