use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn files_under(root: &Path, dir: &Path, output: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(root, &path, output);
        } else {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap();
            // macOS tar/rsync may create AppleDouble metadata beside real
            // fixtures. They are transfer artifacts, not fixture content.
            if name != "SHA256SUMS" && name != "SOURCES.md" && !name.starts_with("._") {
                output.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
}

#[test]
fn fixture_hashes_match_the_locked_manifest() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let data = root.join("tests/data");
    let manifest = std::fs::read_to_string(data.join("SHA256SUMS")).unwrap();
    let locked: BTreeMap<String, String> = manifest
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next().unwrap().to_owned();
            let path = fields.next().unwrap().to_owned();
            (path, hash)
        })
        .collect();

    let mut files = Vec::new();
    files_under(root, &data, &mut files);
    files.sort();
    let paths: Vec<String> = files
        .iter()
        .map(|path| path.to_str().unwrap().replace('\\', "/"))
        .collect();
    assert_eq!(
        paths,
        locked.keys().cloned().collect::<Vec<_>>(),
        "fixture set differs from SHA256SUMS"
    );

    let (program, prefix): (&str, &[&str]) =
        if Command::new("sha256sum").arg("--version").output().is_ok() {
            ("sha256sum", &[])
        } else {
            ("shasum", &["-a", "256"])
        };
    let output = Command::new(program)
        .args(prefix)
        .args(&paths)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(output.status.success(), "hashing fixtures failed");
    let actual: BTreeMap<String, String> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next().unwrap().to_owned();
            let path = fields.next().unwrap().to_owned();
            (path, hash)
        })
        .collect();
    assert_eq!(actual, locked, "fixture bytes differ from SHA256SUMS");
}
