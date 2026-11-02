//! Nmkoder uses stable .NET CurrentCulture OrderBy, with numeric sorting off.
use super::*;
use icu_collator::{
    Collator, CollatorBorrowed, CollatorPreferences,
    options::{AlternateHandling, CollatorOptions, Strength},
    preferences::CollationNumericOrdering,
};
pub(super) const ENGINE: &str = "icu4x-2.3.0";
fn posix(value: &str) -> String {
    let value = value.split('.').next().unwrap_or(value);
    let (base, modifier) = value.split_once('@').unwrap_or((value, ""));
    if matches!(base, "C" | "POSIX" | "en_US_POSIX" | "") {
        return "und".into();
    }
    let mut tag = base.replace('_', "-");
    if let Some(collation) = modifier.strip_prefix("collation=") {
        let collation = match collation {
            "phonebook" => "phonebk",
            "traditional" => "trad",
            other => other,
        };
        tag.push_str(&format!("-u-co-{collation}"));
    }
    tag
}
#[cfg(unix)]
fn environment() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .map(|value| posix(&value))
}
#[cfg(windows)]
pub(super) fn locale() -> String {
    let mut name = [0u16; 85];
    let n = unsafe {
        windows_sys::Win32::Globalization::GetUserDefaultLocaleName(
            name.as_mut_ptr(),
            name.len() as i32,
        )
    };
    if n > 1 {
        String::from_utf16(&name[..n as usize - 1])
            .map(|name| posix(&name))
            .unwrap_or_else(|_| "und".into())
    } else {
        "und".into()
    }
}
#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn locale() -> String {
    environment().unwrap_or_else(|| "und".into())
}
#[cfg(target_os = "macos")]
pub(super) fn locale() -> String {
    if let Some(locale) = environment().filter(|locale| locale != "und") {
        return locale;
    }
    use std::ffi::c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFLocaleCopyCurrent() -> *const c_void;
        fn CFLocaleGetIdentifier(locale: *const c_void) -> *const c_void;
        fn CFStringGetCString(
            string: *const c_void,
            buffer: *mut i8,
            size: isize,
            encoding: u32,
        ) -> u8;
        fn CFRelease(value: *const c_void);
    }
    // A current locale's identifier is borrowed until its retained locale is released.
    unsafe {
        let locale = CFLocaleCopyCurrent();
        if locale.is_null() {
            return "und".into();
        }
        let identifier = CFLocaleGetIdentifier(locale);
        let mut bytes = [0i8; 256];
        let ok = !identifier.is_null()
            && CFStringGetCString(
                identifier,
                bytes.as_mut_ptr(),
                bytes.len() as isize,
                0x08000100,
            ) != 0;
        let name = if ok {
            std::ffi::CStr::from_ptr(bytes.as_ptr())
                .to_str()
                .ok()
                .map(posix)
        } else {
            None
        };
        CFRelease(locale);
        name.unwrap_or_else(|| "und".into())
    }
}
pub(super) fn collator(locale: &str) -> Result<CollatorBorrowed<'static>, AppError> {
    let locale = locale.parse::<icu_locale_core::Locale>().map_err(|e| {
        AppError::new(
            "IMAGE_SEQUENCE_LOCALE_INVALID",
            format!("Cannot use the source filename-sort locale: {e}"),
            None,
        )
    })?;
    let mut preferences: CollatorPreferences = locale.into();
    preferences.numeric_ordering = Some(CollationNumericOrdering::False);
    let mut options = CollatorOptions::default();
    options.strength = Some(Strength::Tertiary);
    options.alternate_handling = Some(AlternateHandling::NonIgnorable);
    Collator::try_new(preferences, options).map_err(|e| {
        AppError::new(
            "IMAGE_SEQUENCE_LOCALE_INVALID",
            format!("Filename collation data is unavailable: {e}"),
            None,
        )
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn posix_locale_selection_does_not_confuse_ui_language_with_current_culture() {
        assert_eq!(posix("C.UTF-8"), "und");
        assert_eq!(posix("en_US_POSIX"), "und");
        assert_eq!(posix("es_PY.UTF-8"), "es-PY");
        assert_eq!(posix("de_DE@collation=phonebook"), "de-DE-u-co-phonebk");
    }
    #[test]
    #[ignore = "requires .NET10 SDK; compares actual source OrderBy with compiled ICU collation"]
    fn filenames_match_actual_dotnet_current_culture_orderby() {
        use std::process::{Command, Stdio};
        let root = std::env::temp_dir().join(format!(
            "jesses-dotnet-sort-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sequence-sort");
        let names: Vec<String> =
            serde_json::from_slice(&fs::read(fixture.join("names.json")).unwrap()).unwrap();
        for (culture, environment_locale) in [
            ("CURRENT", None),
            ("", None),
            ("en-US", None),
            ("es-PY", None),
            ("sv-SE", None),
            ("tr-TR", None),
            ("de-DE", None),
            ("de-DE_phoneb", None),
            ("CURRENT", Some("sv_SE.UTF-8")),
            ("CURRENT", Some("tr_TR.UTF-8")),
        ] {
            let mut command = Command::new("dotnet");
            command
                .args(["run", "--project"])
                .arg(fixture.join("SequenceSort.csproj"))
                .args(["--configuration", "Release", "--artifacts-path"])
                .arg(&root)
                .args(["--", culture])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Some(locale) = environment_locale {
                command.env("LC_ALL", locale);
            }
            let mut child = command.spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&serde_json::to_vec(&names).unwrap())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let report: serde_json::Value = serde_json::from_slice(&output.stdout)
                .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stdout)));
            let actual_locale = report["sortLocale"].as_str().unwrap();
            let selected = if let Some(locale) = environment_locale {
                posix(locale)
            } else if culture == "CURRENT" {
                locale()
            } else if actual_locale.is_empty() {
                "und".into()
            } else {
                posix(actual_locale)
            };
            let collator = collator(&selected).unwrap();
            let mut ordered = names.clone();
            ordered.sort_by(|a, b| collator.compare(a, b));
            let expected = report["ordered"]
                .as_array()
                .unwrap()
                .iter()
                .map(|name| name.as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            assert_eq!(
                ordered, expected,
                ".NET locale {actual_locale:?}, ICU locale {selected:?}"
            );
            eprintln!(
                "Matched .NET locale {actual_locale:?} with ICU locale {selected:?} ({} names)",
                names.len()
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
