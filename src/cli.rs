use std::fs;
use std::path::PathBuf;

use crate::diag::Diagnostic;
use crate::parser;
use crate::source::SourceMap;

const USAGE: &str = "使い方: clum build <path>";

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
        eprintln!("error: build コマンドにはビルド対象のファイルパスを指定してください");
        eprintln!("{USAGE}");
        return 1;
    };

    let path = PathBuf::from(path_arg);
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) => {
            let display = path.display();
            let message = format!("ファイル `{display}` を読み込めません: {err}");
            let diagnostic = Diagnostic::error(message);
            eprintln!("{}", diagnostic.render(&SourceMap::new()));
            return 1;
        }
    };

    let mut sources = SourceMap::new();
    let file = sources.add_file(path, content);
    match parser::parse(sources.get(file).content(), file) {
        Ok(_) => 0,
        Err(diagnostic) => {
            eprintln!("{}", diagnostic.render(&sources));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn run_build_with_unreadable_path_fails() {
        assert_eq!(
            run(&["build".to_string(), "/does/not/exist.clum".to_string()]),
            1
        );
    }

    #[test]
    fn run_build_with_readable_path_succeeds() {
        let dir = std::env::temp_dir().join(format!("clum-cli-unit-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.clum");
        fs::write(&path, "").unwrap();

        assert_eq!(
            run(&["build".to_string(), path.to_string_lossy().into_owned()]),
            0
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
