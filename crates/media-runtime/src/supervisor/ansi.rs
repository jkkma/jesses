//! Stateful terminal-control removal for diagnostic pipes only. Captured binary
//! stdout and streamed frame JSON never pass through this filter.
#[derive(Default)]
pub(super) struct Filter(State);

#[derive(Default)]
enum State {
    #[default]
    Text,
    Escape,
    Csi,
    String,
    StringEscape,
}

impl Filter {
    pub(super) fn push(&mut self, byte: u8) -> Option<u8> {
        // A malformed/truncated control sequence must not hide later records.
        if matches!(byte, b'\r' | b'\n') {
            self.0 = State::Text;
            return Some(byte);
        }
        match self.0 {
            State::Text => {
                if byte == 0x1b {
                    self.0 = State::Escape;
                    None
                } else {
                    Some(byte)
                }
            }
            State::Escape => {
                self.0 = match byte {
                    b'[' => State::Csi,
                    b']' | b'P' | b'X' | b'^' | b'_' => State::String,
                    // Intermediate bytes introduce a longer escape sequence,
                    // for example ESC ( B selecting the ASCII character set.
                    0x20..=0x2f => State::Escape,
                    _ => State::Text,
                };
                None
            }
            State::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    self.0 = State::Text;
                }
                None
            }
            State::String => {
                self.0 = match byte {
                    0x07 => State::Text,
                    0x1b => State::StringEscape,
                    _ => State::String,
                };
                None
            }
            State::StringEscape => {
                self.0 = match byte {
                    b'\\' | 0x07 => State::Text,
                    0x1b => State::StringEscape,
                    _ => State::String,
                };
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(chunks: &[&[u8]]) -> Vec<u8> {
        let mut filter = Filter::default();
        chunks
            .iter()
            .flat_map(|chunk| {
                chunk
                    .iter()
                    .filter_map(|byte| filter.push(*byte))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn real_svt_fork_progress_is_plain_across_every_possible_pipe_split() {
        let colored = b"Encoding: \x1b[33m   48/48 Frames\x1b[0m @ \x1b[32m74.13\x1b[0m fps\r";
        for split in 0..=colored.len() {
            assert_eq!(
                filter(&[&colored[..split], &colored[split..]]),
                b"Encoding:    48/48 Frames @ 74.13 fps\r"
            );
        }
    }

    #[test]
    fn removes_title_hyperlink_and_cursor_controls_without_altering_unicode() {
        let text = "\x1b]0;hidden title\x07日本語 \x1b]8;;https://example.invalid\x1b\\video\x1b]8;;\x1b\\\x1b[2K\n";
        assert_eq!(filter(&[text.as_bytes()]), "日本語 video\n".as_bytes());
    }

    #[test]
    fn truncated_sequences_cannot_swallow_later_records() {
        assert_eq!(
            filter(&[b"first\x1b[33\nsecond\x1b]unfinished\nthird"]),
            b"first\nsecond\nthird"
        );
    }
}
