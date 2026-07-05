use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct TmpDir {
    path: PathBuf,
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn golden_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn is_excluded(name: &OsStr) -> bool {
    name == OsStr::new("expected")
        || name == OsStr::new("expected_error.txt")
        || name == OsStr::new("expected_warnings.txt")
}

fn copy_case_inputs(src: &Path, dst: &Path) {
    fs::create_dir_all(dst)
        .unwrap_or_else(|e| panic!("{}: 作成に失敗しました: {e}", dst.display()));
    for entry in fs::read_dir(src)
        .unwrap_or_else(|e| panic!("{}: 読み取りに失敗しました: {e}", src.display()))
    {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if is_excluded(&name) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if from.is_dir() {
            copy_case_inputs(&from, &to);
        } else {
            fs::copy(&from, &to)
                .unwrap_or_else(|e| panic!("{}: コピーに失敗しました: {e}", from.display()));
        }
    }
}

fn collect_relative_files(root: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    collect_relative_files_into(root, root, &mut results);
    results.sort();
    results
}

fn collect_relative_files_into(root: &Path, current: &Path, results: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(current)
        .unwrap_or_else(|e| panic!("{}: 読み取りに失敗しました: {e}", current.display()))
    {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            collect_relative_files_into(root, &path, results);
        } else {
            results.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

fn run_case(case_dir: &Path, update: bool) {
    let case_name = case_dir.file_name().unwrap().to_string_lossy().into_owned();

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = TmpDir {
        path: env::temp_dir().join(format!(
            "clum-golden-{case_name}-{}-{nanos}",
            std::process::id()
        )),
    };
    copy_case_inputs(case_dir, &tmp.path);

    let output = Command::new(env!("CARGO_BIN_EXE_clum"))
        .arg("build")
        .arg("main.clum")
        .current_dir(&tmp.path)
        .output()
        .unwrap_or_else(|e| panic!("case {case_name}: バイナリの実行に失敗しました: {e}"));

    let expected_error_path = case_dir.join("expected_error.txt");
    if expected_error_path.is_file() {
        assert_eq!(
            output.status.code(),
            Some(1),
            "case {case_name}: 終了コードが一致しません"
        );
        let actual_stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if update {
            fs::write(&expected_error_path, &actual_stderr).unwrap_or_else(|e| {
                panic!("case {case_name}: expected_error.txt の書き込みに失敗しました: {e}")
            });
        } else {
            let expected_stderr = fs::read_to_string(&expected_error_path).unwrap_or_else(|e| {
                panic!("case {case_name}: expected_error.txt の読み取りに失敗しました: {e}")
            });
            assert_eq!(
                actual_stderr, expected_stderr,
                "case {case_name}: stderr が一致しません"
            );
        }
        return;
    }

    assert_eq!(
        output.status.code(),
        Some(0),
        "case {case_name}: 終了コードが一致しません"
    );

    let expected_warnings_path = case_dir.join("expected_warnings.txt");
    if expected_warnings_path.is_file() {
        let actual_stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if update {
            fs::write(&expected_warnings_path, &actual_stderr).unwrap_or_else(|e| {
                panic!("case {case_name}: expected_warnings.txt の書き込みに失敗しました: {e}")
            });
        } else {
            let expected_stderr = fs::read_to_string(&expected_warnings_path).unwrap_or_else(|e| {
                panic!("case {case_name}: expected_warnings.txt の読み取りに失敗しました: {e}")
            });
            assert_eq!(
                actual_stderr, expected_stderr,
                "case {case_name}: 警告の stderr が一致しません"
            );
        }
    }

    let expected_dir = case_dir.join("expected");
    if expected_dir.is_dir() {
        for relative in collect_relative_files(&expected_dir) {
            let expected_path = expected_dir.join(&relative);
            let actual_path = tmp.path.join(&relative);
            if update {
                let actual_bytes = fs::read(&actual_path).unwrap_or_else(|e| {
                    panic!(
                        "case {case_name}: 実際の出力 {} が読めません: {e}",
                        actual_path.display()
                    )
                });
                fs::write(&expected_path, actual_bytes).unwrap_or_else(|e| {
                    panic!(
                        "case {case_name}: {} の書き込みに失敗しました: {e}",
                        expected_path.display()
                    )
                });
            } else {
                let expected_bytes = fs::read(&expected_path).unwrap_or_else(|e| {
                    panic!(
                        "case {case_name}: {} の読み取りに失敗しました: {e}",
                        expected_path.display()
                    )
                });
                let actual_bytes = fs::read(&actual_path).unwrap_or_else(|e| {
                    panic!(
                        "case {case_name}: 実際の出力 {} が読めません: {e}",
                        actual_path.display()
                    )
                });
                assert_eq!(
                    actual_bytes,
                    expected_bytes,
                    "case {case_name}: {} の内容が一致しません",
                    relative.display()
                );
            }
        }
    }
}

#[test]
fn golden_cases() {
    let root = golden_root();
    let update = env::var_os("UPDATE_GOLDEN").is_some();

    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };

    let mut cases: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    cases.sort();

    for case in cases {
        run_case(&case, update);
    }
}
