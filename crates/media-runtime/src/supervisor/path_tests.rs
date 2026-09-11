use super::*;
use std::collections::BTreeMap;

fn environment() -> BTreeMap<String, String> {
    std::env::vars_os()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect()
}

pub(super) fn dump_environment(path: &Path) {
    std::fs::write(
        path.join("environment.json"),
        serde_json::to_vec(&environment()).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn concurrent_child_path_overrides_preserve_parent_and_other_variables() {
    let first = Fixture::new("environment-parent");
    let second = Fixture::new("environment-parent");
    let plain = Fixture::new("environment");
    let before = environment();
    let first_path = OsString::from("chosen 日本語 encoder one");
    let second_path = OsString::from("chosen encoder two");
    let (_owner, cancel) = watch::channel(false);
    let first_spec = first.spec();
    let second_spec = second.spec();
    let plain_spec = plain.spec();
    let (one, two, unchanged) = tokio::join!(
        run_capture_with_path(
            &first_spec,
            cancel.clone(),
            65536,
            Duration::from_secs(10),
            Some(&first_path)
        ),
        run_capture_with_path(
            &second_spec,
            cancel.clone(),
            65536,
            Duration::from_secs(10),
            Some(&second_path)
        ),
        run_capture(&plain_spec, cancel, 65536, Duration::from_secs(10)),
    );
    assert!(one.unwrap().status.success());
    assert!(two.unwrap().status.success());
    assert!(unchanged.unwrap().status.success());
    let read = |path: PathBuf| -> BTreeMap<String, String> {
        serde_json::from_slice(&std::fs::read(path.join("environment.json")).unwrap()).unwrap()
    };
    let without_path = |mut entries: BTreeMap<String, String>| {
        entries.retain(|key, _| !key.eq_ignore_ascii_case("PATH"));
        entries
    };
    for (fixture, expected) in [(&first, first_path), (&second, second_path)] {
        for directory in [fixture.0.clone(), fixture.0.join("child")] {
            let actual = read(directory);
            assert_eq!(
                actual
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
                    .unwrap()
                    .1,
                &expected.to_string_lossy()
            );
            assert_eq!(without_path(actual), without_path(before.clone()));
        }
    }
    assert_eq!(read(plain.0.clone()), before);
    assert_eq!(environment(), before);
}
