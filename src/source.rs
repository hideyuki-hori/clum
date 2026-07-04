use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(u32);

pub struct SourceFile {
    id: FileId,
    path: PathBuf,
    content: String,
    line_starts: Vec<usize>,
}

impl SourceFile {
    fn new(id: FileId, path: PathBuf, content: String) -> Self {
        let line_starts = line_starts(&content);
        Self {
            id,
            path,
            content,
            line_starts,
        }
    }

    pub fn id(&self) -> FileId {
        self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let line_index = match self.line_starts.binary_search(&offset) {
            Ok(index) => index,
            Err(index) => index - 1,
        };
        let line_start = self.line_starts[line_index];
        let col = self.content[line_start..offset].chars().count() + 1;
        (line_index + 1, col)
    }

    pub fn line_text(&self, line: usize) -> &str {
        let index = line - 1;
        let start = self.line_starts[index];
        let end = self
            .line_starts
            .get(index + 1)
            .copied()
            .unwrap_or(self.content.len());
        self.content[start..end].trim_end_matches(['\n', '\r'])
    }
}

fn line_starts(content: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, ch) in content.char_indices() {
        if ch == '\n' {
            starts.push(index + 1);
        }
    }
    starts
}

#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_file(&mut self, path: PathBuf, content: String) -> FileId {
        let id = FileId(self.files.len() as u32);
        self.files.push(SourceFile::new(id, path, content));
        id
    }

    pub fn get(&self, id: FileId) -> &SourceFile {
        &self.files[id.0 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(content: &str) -> SourceFile {
        SourceFile::new(FileId(0), PathBuf::from("test.clum"), content.to_string())
    }

    #[test]
    fn line_col_start_of_first_line() {
        let file = make("abc\ndef\n");
        assert_eq!(file.line_col(0), (1, 1));
    }

    #[test]
    fn line_col_within_first_line() {
        let file = make("abc\ndef\n");
        assert_eq!(file.line_col(2), (1, 3));
    }

    #[test]
    fn line_col_start_of_second_line() {
        let file = make("abc\ndef\n");
        assert_eq!(file.line_col(4), (2, 1));
    }

    #[test]
    fn line_col_within_second_line() {
        let file = make("abc\ndef\n");
        assert_eq!(file.line_col(6), (2, 3));
    }

    #[test]
    fn line_col_at_newline_char_is_end_of_that_line() {
        let file = make("abc\ndef\n");
        assert_eq!(file.line_col(3), (1, 4));
    }

    #[test]
    fn line_col_multibyte_line() {
        let file = make("こんにちは\n世界");
        assert_eq!(file.line_col(0), (1, 1));
        let second_char = "こ".len();
        assert_eq!(file.line_col(second_char), (1, 2));
        let fifth_char = "こんにち".len();
        assert_eq!(file.line_col(fifth_char), (1, 5));
    }

    #[test]
    fn line_col_second_line_multibyte() {
        let file = make("こんにちは\n世界");
        let second_line_start = "こんにちは\n".len();
        assert_eq!(file.line_col(second_line_start), (2, 1));
    }

    #[test]
    fn line_col_end_of_file_without_trailing_newline() {
        let file = make("abc\ndef");
        let end = file.content().len();
        assert_eq!(file.line_col(end), (2, 4));
    }

    #[test]
    fn line_col_end_of_file_with_trailing_newline() {
        let file = make("abc\ndef\n");
        let end = file.content().len();
        assert_eq!(file.line_col(end), (3, 1));
    }

    #[test]
    fn line_text_strips_trailing_newline() {
        let file = make("abc\ndef\n");
        assert_eq!(file.line_text(1), "abc");
        assert_eq!(file.line_text(2), "def");
    }

    #[test]
    fn line_text_last_line_without_trailing_newline() {
        let file = make("abc\ndef");
        assert_eq!(file.line_text(2), "def");
    }

    #[test]
    fn source_map_assigns_sequential_ids_and_returns_files() {
        let mut sources = SourceMap::new();
        let a = sources.add_file(PathBuf::from("a.clum"), "a".to_string());
        let b = sources.add_file(PathBuf::from("b.clum"), "b".to_string());
        assert_ne!(a, b);
        assert_eq!(sources.get(a).path(), Path::new("a.clum"));
        assert_eq!(sources.get(b).path(), Path::new("b.clum"));
    }
}
