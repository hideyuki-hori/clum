use std::path::PathBuf;

use crate::diag::Diagnostic;
use crate::eval;
use crate::resolve;
use crate::source::SourceMap;
use crate::typeck;

const USAGE: &str = "使い方: clum build <ディレクトリ>";

pub fn run(args: &[String]) -> u8 {
    match args.first().map(String::as_str) {
        None => {
            eprintln!("error: サブコマンドを指定してください");
            eprintln!("{USAGE}");
            1
        }
        Some("build") => run_build(args.get(1)),
        Some(other) => {
            eprintln!("error: 不明なサブコマンドです: `{other}`");
            eprintln!("{USAGE}");
            1
        }
    }
}

fn run_build(path_arg: Option<&String>) -> u8 {
    let Some(path_arg) = path_arg else {
        eprintln!("error: build コマンドにはビルド対象のディレクトリを指定してください");
        eprintln!("{USAGE}");
        return 1;
    };

    let path = PathBuf::from(path_arg);
    if path.is_file() {
        let diagnostic = Diagnostic::error(format!(
            "`{path_arg}` はファイルです。ファイルパス指定は廃止されました"
        ))
        .with_label("`clum build <ディレクトリ>` の形で指定します（そのディレクトリの窓口 `_.clum` が実行されます）");
        eprintln!("{}", diagnostic.render(&SourceMap::new()));
        return 1;
    }

    let mut sources = SourceMap::new();
    let program = match resolve::resolve_program(&mut sources, &path) {
        Ok(program) => program,
        Err(diagnostic) => {
            eprintln!("{}", diagnostic.render(&sources));
            return 1;
        }
    };
    let warnings = match typeck::check_program(&program) {
        Ok(warnings) => warnings,
        Err(diagnostic) => {
            eprintln!("{}", diagnostic.render(&sources));
            return 1;
        }
    };
    for warning in &warnings {
        eprintln!("{}", warning.render(&sources));
    }

    match eval::eval_program(&sources, &program) {
        Ok(()) => 0,
        Err(diagnostic) => {
            eprintln!("{}", diagnostic.render(&sources));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn run_with_no_args_fails() {
        assert_eq!(run(&[]), 1);
    }

    #[test]
    fn run_with_unknown_subcommand_fails() {
        assert_eq!(run(&["run".to_string()]), 1);
    }

    #[test]
    fn run_build_with_no_path_fails() {
        assert_eq!(run(&["build".to_string()]), 1);
    }

    #[test]
    fn run_build_with_missing_dir_fails() {
        assert_eq!(
            run(&["build".to_string(), "/does/not/exist".to_string()]),
            1
        );
    }

    #[test]
    fn run_build_with_file_path_fails() {
        let dir = std::env::temp_dir().join(format!("clum-cli-file-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("_.clum");
        fs::write(&path, "").unwrap();

        assert_eq!(
            run(&["build".to_string(), path.to_string_lossy().into_owned()]),
            1
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_build_without_window_fails() {
        let dir = std::env::temp_dir().join(format!("clum-cli-nowin-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        assert_eq!(
            run(&["build".to_string(), dir.to_string_lossy().into_owned()]),
            1
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_build_with_window_succeeds() {
        let dir = std::env::temp_dir().join(format!("clum-cli-ok-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("_.clum"), "").unwrap();

        assert_eq!(
            run(&["build".to_string(), dir.to_string_lossy().into_owned()]),
            0
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
