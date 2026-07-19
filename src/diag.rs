use crate::source::{FileId, SourceMap};
use crate::span::Span;

#[derive(Debug)]
enum Severity {
    Error,
    Warning,
}

impl Severity {
    fn label(&self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

#[derive(Debug)]
pub struct Diagnostic {
    severity: Severity,
    message: String,
    file: Option<FileId>,
    span: Option<Span>,
    label: Option<String>,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            file: None,
            span: None,
            label: None,
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            file: None,
            span: None,
            label: None,
        }
    }

    pub fn at(mut self, file: FileId, span: Span) -> Self {
        self.file = Some(file);
        self.span = Some(span);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn render(&self, sources: &SourceMap) -> String {
        let mut out = String::new();
        out.push_str(self.severity.label());
        out.push_str(": ");
        out.push_str(&self.message);

        if let (Some(file_id), Some(span)) = (self.file, self.span) {
            let file = sources.get(file_id);
            let (line, col) = file.line_col(span.start);
            let width = line.to_string().chars().count();
            let caret_len = file.content()[span.start..span.end].chars().count().max(1);

            out.push('\n');
            out.push_str(&" ".repeat(width));
            out.push_str("--> ");
            out.push_str(&file.path().display().to_string());
            out.push(':');
            out.push_str(&line.to_string());
            out.push(':');
            out.push_str(&col.to_string());

            out.push('\n');
            out.push_str(&" ".repeat(width));
            out.push_str(" |");

            out.push('\n');
            out.push_str(&format!("{line:>width$} | {}", file.line_text(line)));

            out.push('\n');
            out.push_str(&" ".repeat(width));
            out.push_str(" | ");
            out.push_str(&" ".repeat(col - 1));
            out.push_str(&"^".repeat(caret_len));
            if let Some(label) = &self.label {
                out.push(' ');
                out.push_str(label);
            }
        } else if let Some(label) = &self.label {
            out.push('\n');
            out.push_str("  = ");
            out.push_str(label);
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn renders_error_with_span_and_label() {
        let mut sources = SourceMap::new();
        let content = "line1\nline2\nline3\nline4\n    h .dvi\n".to_string();
        let dot_index = content.find("    h .dvi").unwrap() + 6;
        let file = sources.add_file(PathBuf::from("examples/index.clum"), content);

        let diagnostic = Diagnostic::error("`Tag` に `.dvi` はありません")
            .at(file, Span::new(dot_index, dot_index + 4))
            .with_label("もしかして `.div` ですか？");

        let expected = [
            "error: `Tag` に `.dvi` はありません",
            " --> examples/index.clum:5:7",
            "  |",
            "5 |     h .dvi",
            "  |       ^^^^ もしかして `.div` ですか？",
        ]
        .join("\n");

        assert_eq!(diagnostic.render(&sources), expected);
    }

    #[test]
    fn renders_with_two_digit_line_number() {
        let mut sources = SourceMap::new();
        let mut content = String::new();
        for n in 1..=9 {
            content.push_str(&format!("line{n}\n"));
        }
        content.push_str("  x .dvi\n");
        let dot_index = content.rfind("  x .dvi").unwrap() + 4;
        let file = sources.add_file(PathBuf::from("a.clum"), content);

        let diagnostic = Diagnostic::error("test").at(file, Span::new(dot_index, dot_index + 4));

        let expected = [
            "error: test",
            "  --> a.clum:10:5",
            "   |",
            "10 |   x .dvi",
            "   |     ^^^^",
        ]
        .join("\n");

        assert_eq!(diagnostic.render(&sources), expected);
    }

    #[test]
    fn renders_warning_without_location() {
        let sources = SourceMap::new();
        let diagnostic = Diagnostic::warning("必須属性がありません");
        assert_eq!(diagnostic.render(&sources), "warning: 必須属性がありません");
    }

    #[test]
    fn renders_label_without_location() {
        let sources = SourceMap::new();
        let diagnostic =
            Diagnostic::error("窓口がありません").with_label("`_.clum` を置いてください");
        assert_eq!(
            diagnostic.render(&sources),
            "error: 窓口がありません\n  = `_.clum` を置いてください"
        );
    }
}
