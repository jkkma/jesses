//! Parse FFprobe's JSON document one bounded frame at a time. There is no total
//! metadata cap: a long movie uses the same parser/validator memory as a clip.
use std::io::{BufReader, Read};

#[cfg(test)]
use super::super::encode_plan::Frame;

// Includes all fields and side data for one frame (or an unknown envelope field).
// The selected FFprobe fields normally occupy under a few KiB per HDR frame.
const FRAME_BYTES: usize = 1024 * 1024;
const MAX_DEPTH: usize = 128;

pub(crate) fn parse<T: serde::de::DeserializeOwned>(
    reader: &mut dyn Read,
    mut frame: impl FnMut(T),
) -> Result<(), String> {
    let mut json = Json {
        bytes: BufReader::with_capacity(64 * 1024, reader).bytes(),
        pending: None,
        record: Vec::new(),
    };
    json.expect(b'{')?;
    let mut seen_frames = false;
    let mut next = json.required()?;
    if next != b'}' {
        loop {
            if next != b'"' {
                return Err("The frame document contains an invalid field name.".into());
            }
            let key: String =
                serde_json::from_slice(json.value(next)?).map_err(|error| error.to_string())?;
            json.expect(b':')?;
            if key == "frames" {
                if seen_frames {
                    return Err("The frame document contains duplicate frames arrays.".into());
                }
                seen_frames = true;
                json.expect(b'[')?;
                let mut item = json.required()?;
                if item != b']' {
                    loop {
                        let decoded = serde_json::from_slice::<T>(json.value(item)?)
                            .map_err(|error| error.to_string())?;
                        frame(decoded);
                        match json.required()? {
                            b']' => break,
                            b',' => item = json.required()?,
                            _ => return Err("The frames array has an invalid separator.".into()),
                        }
                    }
                }
            } else {
                // Match serde's unknown-field behavior, while still bounding and
                // checking those values instead of accepting arbitrary garbage.
                let first = json.required()?;
                serde_json::from_slice::<serde_json::Value>(json.value(first)?)
                    .map_err(|error| error.to_string())?;
            }
            match json.required()? {
                b'}' => break,
                b',' => next = json.required()?,
                _ => return Err("The frame document has an invalid separator.".into()),
            }
        }
    }
    if !seen_frames {
        return Err("The frame document has no frames array.".into());
    }
    if json.nonwhite()?.is_some() {
        return Err("The frame document contains trailing data.".into());
    }
    Ok(())
}

struct Json<R: Read> {
    bytes: std::io::Bytes<BufReader<R>>,
    pending: Option<u8>,
    record: Vec<u8>,
}

impl<R: Read> Json<R> {
    fn byte(&mut self) -> Result<Option<u8>, String> {
        if let Some(byte) = self.pending.take() {
            return Ok(Some(byte));
        }
        self.bytes
            .next()
            .transpose()
            .map_err(|error| error.to_string())
    }

    fn nonwhite(&mut self) -> Result<Option<u8>, String> {
        loop {
            match self.byte()? {
                Some(b' ' | b'\n' | b'\r' | b'\t') => {}
                byte => return Ok(byte),
            }
        }
    }

    fn required(&mut self) -> Result<u8, String> {
        self.nonwhite()?
            .ok_or_else(|| "The frame document was truncated.".into())
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        if self.required()? != expected {
            return Err(format!(
                "Expected '{}' in the frame document.",
                char::from(expected)
            ));
        }
        Ok(())
    }

    fn value(&mut self, first: u8) -> Result<&[u8], String> {
        self.record.clear();
        self.record.push(first);
        let mut depth = usize::from(matches!(first, b'{' | b'['));
        let mut quoted = first == b'"';
        let string_value = quoted;
        let mut escaped = false;
        loop {
            let next = self.byte()?;
            let Some(byte) = next else {
                if depth == 0 && !quoted && !string_value {
                    return Ok(&self.record);
                }
                return Err("A frame metadata value was truncated.".into());
            };
            if depth == 0
                && !quoted
                && matches!(byte, b',' | b']' | b'}' | b' ' | b'\n' | b'\r' | b'\t')
            {
                self.pending = Some(byte);
                return Ok(&self.record);
            }
            if self.record.len() == FRAME_BYTES {
                return Err("A single frame metadata record exceeds the 1 MiB limit.".into());
            }
            self.record.push(byte);
            if quoted {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    quoted = false;
                    if string_value && depth == 0 {
                        return Ok(&self.record);
                    }
                }
            } else {
                match byte {
                    b'"' => quoted = true,
                    b'{' | b'[' => {
                        depth += 1;
                        if depth > MAX_DEPTH {
                            return Err(
                                "Frame metadata nesting exceeds the supported limit.".into()
                            );
                        }
                    }
                    b'}' | b']' if depth > 0 => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(&self.record);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(input: &[u8]) -> Result<usize, String> {
        let mut count = 0;
        parse(&mut &*input, |_: Frame| count += 1)?;
        Ok(count)
    }

    #[test]
    fn strict_document_and_frame_boundaries() {
        assert_eq!(
            count(br#" {"extra":{"a":[1,true,null]},"frames":[{},{}]} "#),
            Ok(2)
        );
        assert_eq!(count(br#"{"frames":[]}"#), Ok(0));
        for invalid in [
            r#"{}"#,
            r#"{"frames":null}"#,
            r#"{"frames":[{}"#,
            r#"{"frames":[{},]}"#,
            r#"{"frames":[{}],}"#,
            r#"{"frames":[],"frames":[]}"#,
            r#"{"frames":[{}]}{}"#,
            r#"{"frames":[{}],"extra":tru}"#,
            r#"{"frames":[{"width":"128"}]}"#,
            r#"{"frames":[{"side_data_list":[{]}]}"#,
            r#"{"frames":[{"width":128,"width":128}]}"#,
        ] {
            assert!(count(invalid.as_bytes()).is_err(), "{invalid}");
        }
    }

    #[test]
    fn escaped_strings_and_arbitrary_pipe_boundaries() {
        struct Chunks<'a>(&'a [u8]);
        impl Read for Chunks<'_> {
            fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
                let len = target.len().min(self.0.len()).min(3);
                target[..len].copy_from_slice(&self.0[..len]);
                self.0 = &self.0[len..];
                Ok(len)
            }
        }
        let bytes =
            r#"{"frames":[{"side_data_list":[{"note":"} ] { \\"}],"pix_fmt":"日本語"},{}]}"#
                .as_bytes();
        let mut frames = 0;
        parse(&mut Chunks(bytes), |_: Frame| frames += 1).unwrap();
        assert_eq!(frames, 2);
    }

    #[test]
    fn per_record_limit_rejects_oversized_unknown_fields_and_side_data() {
        for prefix in [
            r#"{"frames":[{"ignored":""#,
            r#"{"frames":[{"side_data_list":[{"note":""#,
            r#"{"ignored":""#,
        ] {
            let mut input = prefix.as_bytes().to_vec();
            input.extend(std::iter::repeat_n(b'x', FRAME_BYTES + 1));
            let error = count(&input).unwrap_err();
            assert!(error.contains("1 MiB"), "{error}");
        }
        let deep = format!(
            r#"{{"frames":[{{"ignored":{}"x"{}}}]}}"#,
            "[".repeat(129),
            "]".repeat(129)
        );
        assert!(count(deep.as_bytes()).unwrap_err().contains("nesting"));
    }

    #[test]
    fn late_truncation_never_accepts_the_valid_prefix() {
        let prefix = br#"{"frames":[{},{},{},{},{},{},"#;
        let mut observed = 0;
        assert!(parse(&mut &prefix[..], |_: Frame| observed += 1).is_err());
        assert_eq!(observed, 6);
    }

    #[test]
    fn scans_more_than_the_old_capture_limit_without_retaining_frames() {
        struct RepeatRecord {
            bytes: Vec<u8>,
            offset: usize,
            remaining: usize,
        }
        impl Read for RepeatRecord {
            fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
                if self.remaining == 0 || target.is_empty() {
                    return Ok(0);
                }
                let len = target.len().min(self.bytes.len() - self.offset);
                target[..len].copy_from_slice(&self.bytes[self.offset..self.offset + len]);
                self.offset += len;
                if self.offset == self.bytes.len() {
                    self.offset = 0;
                    self.remaining -= 1;
                }
                Ok(len)
            }
        }
        let bytes = format!(r#"{{"ignored":"{}"}},"#, "x".repeat(2048)).into_bytes();
        assert!(bytes.len() * 40_000 > 64 * 1024 * 1024);
        let records = RepeatRecord {
            bytes,
            offset: 0,
            remaining: 40_000,
        };
        let mut reader = (&br#"{"frames":["#[..]).chain(records).chain(&b"{}]}"[..]);
        let mut count = 0;
        parse(&mut reader, |_: Frame| count += 1).unwrap();
        assert_eq!(count, 40_001);
    }
}
