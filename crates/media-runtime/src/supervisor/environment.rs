//! Preserve the native Unicode environment, including hidden `=C:` drive state.
use std::{io, os::windows::ffi::OsStrExt};
use windows_sys::Win32::{
    Globalization::CompareStringOrdinal,
    System::Environment::{FreeEnvironmentStringsW, GetEnvironmentStringsW},
};

pub(super) fn environment_with_changes(
    changes: &super::super::ChildEnvironment,
) -> io::Result<Vec<u16>> {
    let mut replacements = Vec::new();
    if let Some(path) = &changes.path {
        replacements.push((
            "PATH".encode_utf16().collect(),
            Some(path.encode_wide().collect()),
        ));
    }
    for (name, value) in &changes.variables {
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid child environment name.",
            ));
        }
        replacements.push((
            name.encode_utf16().collect(),
            value.as_ref().map(|value| value.encode_wide().collect()),
        ));
    }
    if replacements
        .iter()
        .any(|(_, value): &(Vec<u16>, Option<Vec<u16>>)| {
            value.as_ref().is_some_and(|value| value.contains(&0))
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Child environment value contains NUL.",
        ));
    }
    // GetEnvironmentStringsW gives this caller a stable, read-only snapshot.
    let raw = unsafe { GetEnvironmentStringsW() };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    struct Snapshot(*mut u16);
    impl Drop for Snapshot {
        fn drop(&mut self) {
            // SAFETY: this pointer came from GetEnvironmentStringsW and is freed once.
            unsafe { FreeEnvironmentStringsW(self.0) };
        }
    }
    let snapshot = Snapshot(raw);
    let mut length = 0;
    // SAFETY: the API returns a complete block terminated by two UTF-16 NULs.
    unsafe {
        while *snapshot.0.add(length) != 0 || *snapshot.0.add(length + 1) != 0 {
            length += 1;
        }
    }
    // SAFETY: length was measured within the API-owned double-NUL-terminated block.
    let inherited = unsafe { std::slice::from_raw_parts(snapshot.0, length + 2) };
    Ok(replace_variables(inherited, &replacements))
}

fn replace_variables(inherited: &[u16], replacements: &[(Vec<u16>, Option<Vec<u16>>)]) -> Vec<u16> {
    let mut entries: Vec<Vec<u16>> = inherited
        .split(|unit| *unit == 0)
        .filter(|entry| !entry.is_empty())
        .map(<[u16]>::to_vec)
        .collect();
    for (name, value) in replacements {
        entries.retain(|entry| {
            !entry.get(..name.len() + 1).is_some_and(|prefix| {
                prefix.last() == Some(&(b'=' as u16))
                    && prefix[..name.len()]
                        .iter()
                        .zip(name)
                        .all(|(unit, expected)| {
                            *unit <= 0x7f
                                && *expected <= 0x7f
                                && (*unit as u8).eq_ignore_ascii_case(&(*expected as u8))
                        })
            })
        });
        if let Some(value) = value {
            let mut replacement = name.clone();
            replacement.push(b'=' as u16);
            replacement.extend_from_slice(value);
            entries.push(replacement);
        }
    }
    // Windows requires case-insensitive Unicode ordering. Comparing complete
    // entries retains the native hidden drive variables and arbitrary values.
    entries.sort_by(|left, right| {
        let name_length = |entry: &[u16]| {
            entry
                .iter()
                .enumerate()
                .skip(1)
                .find(|(_, unit)| **unit == b'=' as u16)
                .map_or(entry.len(), |(index, _)| index)
        };
        // SAFETY: both slices remain alive; explicit lengths avoid NUL reliance.
        let order = unsafe {
            CompareStringOrdinal(
                left.as_ptr(),
                name_length(left) as i32,
                right.as_ptr(),
                name_length(right) as i32,
                1,
            )
        };
        order.cmp(&2)
    });
    let mut block = Vec::new();
    for entry in entries {
        block.extend(entry);
        block.push(0);
    }
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    block
}

#[cfg(test)]
fn replace_path(inherited: &[u16], path: &[u16]) -> Vec<u16> {
    replace_variables(
        inherited,
        &[("PATH".encode_utf16().collect(), Some(path.to_vec()))],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_override_preserves_unicode_drive_state_and_other_values() {
        let source: Vec<u16> = "=C:=C:\\original\0=D:=D:\\日本語\0Alpha=one=two\0Path=C:\\old\0PATH=C:\\duplicate\0Zebra=終\0\0"
            .encode_utf16().collect();
        let selected: Vec<u16> = "C:\\選択 encoder;C:\\old".encode_utf16().collect();
        let actual = replace_path(&source, &selected);
        assert_eq!(
            String::from_utf16(&actual).unwrap(),
            "=C:=C:\\original\0=D:=D:\\日本語\0Alpha=one=two\0PATH=C:\\選択 encoder;C:\\old\0Zebra=終\0\0"
        );
        assert_eq!(
            replace_path(&[], &[]),
            "PATH=\0\0".encode_utf16().collect::<Vec<_>>()
        );
    }

    #[test]
    fn inherited_invalid_unicode_is_preserved_byte_for_byte() {
        let source = [b'X' as u16, b'=' as u16, 0xd800, 0, 0];
        let actual = replace_path(&source, &[]);
        assert!(actual.windows(4).any(|part| part == &source[..4]));
    }

    #[test]
    fn portable_runtime_overrides_remove_aliases_without_mutating_drive_state() {
        let source: Vec<u16> = "=C:=C:\\作業\0Vsscript_Path=old\0VSSCRIPT_PATH=duplicate\0PythonPath=external\0Other=kept\0\0".encode_utf16().collect();
        let result = replace_variables(
            &source,
            &[
                (
                    "VSSCRIPT_PATH".encode_utf16().collect(),
                    Some("C:\\package\\vsscript.dll".encode_utf16().collect()),
                ),
                ("PYTHONPATH".encode_utf16().collect(), None),
            ],
        );
        assert_eq!(
            String::from_utf16(&result).unwrap(),
            "=C:=C:\\作業\0Other=kept\0VSSCRIPT_PATH=C:\\package\\vsscript.dll\0\0"
        );
        assert_eq!(replace_variables(&[], &[]), [0, 0]);
    }
}
