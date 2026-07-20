use std::path::PathBuf;

use crate::eval;
use crate::resolve;
use crate::source::SourceMap;
use crate::typeck;

const USAGE: &str = "使い方: clum build <エントリファイル>";

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
        eprintln!("error: build コマンドにはエントリファイルを指定してください");
        eprintln!("{USAGE}");
        return 1;
    };

    let path = PathBuf::from(path_arg);
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
    fn run_build_with_missing_entry_fails() {
        assert_eq!(
            run(&["build".to_string(), "/does/not/exist.clum".to_string()]),
            1
        );
    }

    #[test]
    fn run_build_with_dir_path_fails() {
        let dir = std::env::temp_dir().join(format!("clum-cli-dir-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        assert_eq!(
            run(&["build".to_string(), dir.to_string_lossy().into_owned()]),
            1
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_build_with_window_as_entry_fails() {
        let dir = std::env::temp_dir().join(format!("clum-cli-win-{}", std::process::id()));
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
    fn run_build_with_entry_file_succeeds() {
        let dir = std::env::temp_dir().join(format!("clum-cli-ok-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("site.clum");
        fs::write(&path, "").unwrap();

        assert_eq!(
            run(&["build".to_string(), path.to_string_lossy().into_owned()]),
            0
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
