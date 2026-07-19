use std::fs;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_clum"))
}

#[test]
fn missing_subcommand_fails_with_exit_code_1() {
    let output = bin().output().expect("バイナリの実行に失敗しました");
    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stderr.is_empty());
}

fn tmp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("clum-cli-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("一時ディレクトリの作成に失敗しました");
    dir
}

#[test]
fn missing_path_fails_with_exit_code_1() {
    let output = bin()
        .arg("build")
        .output()
        .expect("バイナリの実行に失敗しました");
    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stderr.is_empty());
}

#[test]
fn unknown_subcommand_fails_with_exit_code_1() {
    let output = bin()
        .arg("run")
        .output()
        .expect("バイナリの実行に失敗しました");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("run"));
}

#[test]
fn nonexistent_dir_fails_with_japanese_error() {
    let dir = tmp_dir("nonexistent");
    let path = dir.join("does-not-exist");

    let output = bin()
        .arg("build")
        .arg(&path)
        .output()
        .expect("バイナリの実行に失敗しました");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("error:"));
    assert!(stderr.contains("開けません"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn file_path_argument_fails_with_guidance() {
    let dir = tmp_dir("file-arg");
    let path = dir.join("main.clum");
    fs::write(&path, "").expect("テスト用ファイルの書き込みに失敗しました");

    let output = bin()
        .arg("build")
        .arg(&path)
        .output()
        .expect("バイナリの実行に失敗しました");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ファイルパス指定は廃止されました"));
    assert!(stderr.contains("clum build <ディレクトリ>"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn dir_without_window_fails_with_guidance() {
    let dir = tmp_dir("no-window");

    let output = bin()
        .arg("build")
        .arg(&dir)
        .output()
        .expect("バイナリの実行に失敗しました");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("窓口 `_.clum` がありません"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn dir_with_window_succeeds_with_no_output() {
    let dir = tmp_dir("readable");
    fs::write(dir.join("_.clum"), "").expect("テスト用ファイルの書き込みに失敗しました");

    let output = bin()
        .arg("build")
        .arg(&dir)
        .output()
        .expect("バイナリの実行に失敗しました");
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());

    let _ = fs::remove_dir_all(&dir);
}
