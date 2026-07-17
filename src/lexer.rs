use std::collections::VecDeque;

use crate::diag::Diagnostic;
use crate::source::FileId;
use crate::span::Span;
use crate::token::{StrLiteral, StrSegment, Token, TokenKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineHead {
    Ident,
    UpperIdent,
    LBrace,
    PipeGt,
    Text,
}

impl LineHead {
    fn from_kind(kind: &TokenKind) -> Self {
        match kind {
            TokenKind::Ident(_) => LineHead::Ident,
            TokenKind::UpperIdent(_) => LineHead::UpperIdent,
            TokenKind::LBrace => LineHead::LBrace,
            TokenKind::PipeGt => LineHead::PipeGt,
            _ => LineHead::Text,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    NeedIndent,
    Content,
    Done,
}

pub struct Lexer<'a> {
    source: &'a str,
    file: FileId,
    pos: usize,
    indent_stack: Vec<usize>,
    mode: Mode,
    pending: VecDeque<Token>,
    lookahead: Option<(Token, usize)>,
    line_head: bool,
    expect_path: bool,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str, file: FileId) -> Self {
        Self {
            source,
            file,
            pos: 0,
            indent_stack: vec![0],
            mode: Mode::NeedIndent,
            pending: VecDeque::new(),
            lookahead: None,
            line_head: false,
            expect_path: false,
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Token, Diagnostic> {
        if let Some((token, _)) = self.lookahead.take() {
            return Ok(token);
        }
        self.advance_raw()
    }

    pub fn peek(&mut self) -> Result<Token, Diagnostic> {
        if let Some((token, _)) = &self.lookahead {
            return Ok(token.clone());
        }
        let start = self.pos;
        let token = self.advance_raw()?;
        self.lookahead = Some((token.clone(), start));
        Ok(token)
    }

    pub fn classify_line_head(&mut self) -> Result<LineHead, Diagnostic> {
        if let Some((token, _)) = &self.lookahead
            && !is_structural(&token.kind)
        {
            return Ok(LineHead::from_kind(&token.kind));
        }
        if self.mode == Mode::NeedIndent {
            self.process_indentation()?;
        }
        if let Some(token) = self.pending.front()
            && !is_structural(&token.kind)
        {
            return Ok(LineHead::from_kind(&token.kind));
        }
        if self.mode == Mode::Done {
            return Ok(LineHead::Text);
        }
        Ok(classify_at(self.source, self.pos))
    }

    pub fn take_rest_of_line_raw(&mut self) -> Result<(String, Span), Diagnostic> {
        if let Some((token, _)) = self.lookahead.take() {
            if is_structural(&token.kind) {
                self.pending.push_front(token);
            } else {
                self.pos = token.span.start;
            }
        }
        if self.mode == Mode::NeedIndent {
            self.process_indentation()?;
        }
        let start = self.pos;
        let mut end = start;
        while end < self.source.len() && self.source.as_bytes()[end] != b'\n' {
            end += 1;
        }
        let text = self.source[start..end].to_string();
        self.pos = if end < self.source.len() {
            end + 1
        } else {
            end
        };
        self.mode = Mode::NeedIndent;
        self.line_head = false;
        Ok((text, Span::new(start, end)))
    }

    pub fn peek_structural(&mut self) -> Result<Option<TokenKind>, Diagnostic> {
        if let Some((token, _)) = &self.lookahead {
            if is_structural(&token.kind) {
                return Ok(Some(token.kind.clone()));
            }
            return Ok(None);
        }
        if self.mode == Mode::NeedIndent {
            self.process_indentation()?;
        }
        if let Some(token) = self.pending.front() {
            if is_structural(&token.kind) {
                return Ok(Some(token.kind.clone()));
            }
            return Ok(None);
        }
        if self.mode == Mode::Done {
            return Ok(Some(TokenKind::Eof));
        }
        Ok(None)
    }

    pub fn take_text_line(&mut self) -> Result<(StrLiteral, Span), Diagnostic> {
        if let Some((token, _)) = self.lookahead.take() {
            if is_structural(&token.kind) {
                self.pending.push_front(token);
            } else {
                self.pos = token.span.start;
            }
        }
        if self.mode == Mode::NeedIndent {
            self.process_indentation()?;
        }
        let start = self.pos;
        let mut segments = Vec::new();
        let mut text_start = self.pos;
        while !self.at_eof() && self.current_char() != '\n' {
            let ch = self.current_char();
            if ch == '{' {
                if self.peek_char_at(self.pos + 1) == Some('{') {
                    self.pos += 2;
                    continue;
                }
                if self.pos > text_start {
                    segments.push(StrSegment::Text(Span::new(text_start, self.pos)));
                }
                let interp_start = self.pos;
                self.pos += 1;
                let tokens = self.lex_interpolation(interp_start, true)?;
                segments.push(StrSegment::Interp(
                    tokens,
                    Span::new(interp_start, self.pos),
                ));
                text_start = self.pos;
                continue;
            }
            if ch == '}' {
                if self.peek_char_at(self.pos + 1) == Some('}') {
                    self.pos += 2;
                    continue;
                }
                return Err(
                    Diagnostic::error("`}` をテキストに書くには `}}` と書きます")
                        .at(self.file, Span::new(self.pos, self.pos + 1)),
                );
            }
            self.pos += ch.len_utf8();
        }
        let end = self.pos;
        if end > text_start {
            segments.push(StrSegment::Text(Span::new(text_start, end)));
        }
        self.pos = if end < self.source.len() {
            end + 1
        } else {
            end
        };
        self.mode = Mode::NeedIndent;
        self.line_head = false;
        Ok((StrLiteral { segments }, Span::new(start, end)))
    }

    fn advance_raw(&mut self) -> Result<Token, Diagnostic> {
        loop {
            if let Some(token) = self.pending.pop_front() {
                return Ok(token);
            }
            match self.mode {
                Mode::NeedIndent => self.process_indentation()?,
                Mode::Content => return self.lex_content(),
                Mode::Done => {
                    return Ok(Token::new(TokenKind::Eof, Span::new(self.pos, self.pos)));
                }
            }
        }
    }

    fn process_indentation(&mut self) -> Result<(), Diagnostic> {
        loop {
            let line_start = self.pos;
            let mut cursor = self.pos;
            let mut spaces = 0usize;
            let mut tab_pos: Option<usize> = None;
            loop {
                match self.source[cursor..].chars().next() {
                    Some(' ') => {
                        spaces += 1;
                        cursor += 1;
                    }
                    Some('\t') => {
                        if tab_pos.is_none() {
                            tab_pos = Some(cursor);
                        }
                        cursor += 1;
                    }
                    _ => break,
                }
            }
            let ws_end = cursor;
            let at_eof = ws_end >= self.source.len();
            if at_eof {
                self.pos = ws_end;
                break;
            }
            let is_newline = self.source.as_bytes()[ws_end] == b'\n';
            let is_comment = self.source[ws_end..].starts_with("//");
            if is_newline {
                self.pos = ws_end + 1;
                continue;
            }
            if is_comment {
                let mut end = ws_end;
                while end < self.source.len() && self.source.as_bytes()[end] != b'\n' {
                    end += 1;
                }
                self.pos = if end < self.source.len() {
                    end + 1
                } else {
                    end
                };
                continue;
            }
            if let Some(tab_at) = tab_pos {
                return Err(Diagnostic::error("インデントにタブは使えません")
                    .at(self.file, Span::new(tab_at, tab_at + 1)));
            }
            if !spaces.is_multiple_of(2) {
                return Err(Diagnostic::error(format!(
                    "インデントは2スペース単位である必要があります(現在{spaces}スペース)"
                ))
                .at(self.file, Span::new(line_start, ws_end)));
            }
            let level = spaces / 2;
            let current = *self.indent_stack.last().unwrap();
            if level == current {
                self.pos = ws_end;
            } else if level == current + 1 {
                self.indent_stack.push(level);
                self.pending
                    .push_back(Token::new(TokenKind::Indent, Span::new(line_start, ws_end)));
                self.pos = ws_end;
            } else if level > current + 1 {
                return Err(
                    Diagnostic::error("インデントが1段階を超えて深くなっています")
                        .at(self.file, Span::new(line_start, ws_end)),
                );
            } else {
                while *self.indent_stack.last().unwrap() > level {
                    self.indent_stack.pop();
                    self.pending
                        .push_back(Token::new(TokenKind::Dedent, Span::new(line_start, ws_end)));
                }
                if *self.indent_stack.last().unwrap() != level {
                    return Err(Diagnostic::error(
                        "インデントの戻り先が既存のレベルと一致しません",
                    )
                    .at(self.file, Span::new(line_start, ws_end)));
                }
                self.pos = ws_end;
            }
            self.mode = Mode::Content;
            self.line_head = true;
            return Ok(());
        }

        while *self.indent_stack.last().unwrap() > 0 {
            self.indent_stack.pop();
            self.pending
                .push_back(Token::new(TokenKind::Dedent, Span::new(self.pos, self.pos)));
        }
        self.pending
            .push_back(Token::new(TokenKind::Eof, Span::new(self.pos, self.pos)));
        self.mode = Mode::Done;
        Ok(())
    }

    fn lex_content(&mut self) -> Result<Token, Diagnostic> {
        if self.expect_path {
            self.expect_path = false;
            return self.lex_path();
        }

        loop {
            self.skip_inline_whitespace()?;
            if self.at_eof() {
                let span = Span::new(self.pos, self.pos);
                self.mode = Mode::NeedIndent;
                self.line_head = false;
                return Ok(Token::new(TokenKind::Newline, span));
            }
            if self.current_char() == '\n' {
                let span = Span::new(self.pos, self.pos + 1);
                self.pos += 1;
                self.mode = Mode::NeedIndent;
                self.line_head = false;
                return Ok(Token::new(TokenKind::Newline, span));
            }
            if self.source[self.pos..].starts_with("//") {
                self.skip_to_end_of_line();
                continue;
            }
            break;
        }

        let was_line_head = self.line_head;
        self.line_head = false;

        if self.current_char() == '\'' {
            return self.lex_string();
        }

        let token = self.lex_symbol_or_word(was_line_head)?;
        if token.kind == TokenKind::At {
            self.expect_path = true;
        }
        Ok(token)
    }

    fn lex_path(&mut self) -> Result<Token, Diagnostic> {
        if let Some(' ' | '\t') = self.current_char_opt() {
            let ws_start = self.pos;
            self.skip_inline_whitespace()?;
            if self.at_eof() || self.current_char() == '\n' {
                return Err(Diagnostic::error("`@` の後にパスが必要です")
                    .at(self.file, Span::new(ws_start, ws_start)));
            }
            return Err(Diagnostic::error("`@` とパスのあいだに空白は書けません")
                .at(self.file, Span::new(ws_start, self.pos))
                .with_label("パスは `@` に密着させます（例: `@./index`）"));
        }
        if self.at_eof() || self.current_char() == '\n' {
            let pos = self.pos;
            return Err(
                Diagnostic::error("`@` の後にパスが必要です").at(self.file, Span::new(pos, pos))
            );
        }
        let start = self.pos;
        while !self.at_eof() {
            let c = self.current_char();
            if c == ' ' || c == '\t' || c == '\n' {
                break;
            }
            self.pos += c.len_utf8();
        }
        let text = self.source[start..self.pos].to_string();
        Ok(Token::new(
            TokenKind::Path(text),
            Span::new(start, self.pos),
        ))
    }

    fn lex_interpolation(
        &mut self,
        open_pos: usize,
        allow_string: bool,
    ) -> Result<Vec<Token>, Diagnostic> {
        let mut tokens = Vec::new();
        loop {
            self.skip_inline_whitespace()?;
            if self.at_eof() || self.current_char() == '\n' {
                return Err(Diagnostic::error("補間 `{` が閉じられていません")
                    .at(self.file, Span::new(open_pos, open_pos + 1)));
            }
            let ch = self.current_char();
            if ch == '}' {
                self.pos += 1;
                return Ok(tokens);
            }
            if ch == '\'' {
                if allow_string {
                    let token = self.lex_string()?;
                    tokens.push(token);
                    continue;
                }
                return Err(Diagnostic::error("補間式の中に文字列リテラルは書けません")
                    .at(self.file, Span::new(self.pos, self.pos + 1)));
            }
            if ch == '{' {
                return Err(Diagnostic::error("補間式の中に `{` は書けません")
                    .at(self.file, Span::new(self.pos, self.pos + 1)));
            }
            let token = self.lex_symbol_or_word(false)?;
            tokens.push(token);
        }
    }

    fn lex_string(&mut self) -> Result<Token, Diagnostic> {
        let start = self.pos;
        self.pos += 1;
        let mut segments = Vec::new();
        let mut text_start = self.pos;
        loop {
            if self.at_eof() || self.current_char() == '\n' {
                return Err(Diagnostic::error("文字列が閉じられていません")
                    .at(self.file, Span::new(start, start + 1)));
            }
            let ch = self.current_char();
            if ch == '\'' {
                if self.pos > text_start {
                    segments.push(StrSegment::Text(Span::new(text_start, self.pos)));
                }
                self.pos += 1;
                break;
            }
            if ch == '{' {
                if self.peek_char_at(self.pos + 1) == Some('{') {
                    self.pos += 2;
                    continue;
                }
                if self.pos > text_start {
                    segments.push(StrSegment::Text(Span::new(text_start, self.pos)));
                }
                let interp_start = self.pos;
                self.pos += 1;
                let tokens = self.lex_interpolation(interp_start, false)?;
                segments.push(StrSegment::Interp(
                    tokens,
                    Span::new(interp_start, self.pos),
                ));
                text_start = self.pos;
                continue;
            }
            if ch == '}' {
                if self.peek_char_at(self.pos + 1) == Some('}') {
                    self.pos += 2;
                    continue;
                }
                return Err(Diagnostic::error("`}` を文字列に書くには `}}` と書きます")
                    .at(self.file, Span::new(self.pos, self.pos + 1)));
            }
            self.pos += ch.len_utf8();
        }
        Ok(Token::new(
            TokenKind::Str(StrLiteral { segments }),
            Span::new(start, self.pos),
        ))
    }

    fn lex_symbol_or_word(&mut self, was_line_head: bool) -> Result<Token, Diagnostic> {
        let ch = self.current_char();
        match ch {
            '#' => self.single(TokenKind::Hash),
            '@' => self.single(TokenKind::At),
            '!' => self.single(TokenKind::Bang),
            '=' => self.single(TokenKind::Eq),
            ',' => self.single(TokenKind::Comma),
            '{' => self.single(TokenKind::LBrace),
            '}' => self.single(TokenKind::RBrace),
            '<' => self.single(TokenKind::Lt),
            '>' => self.single(TokenKind::Gt),
            '|' => {
                let pos = self.pos;
                if self.peek_char_at(pos + 1) == Some('>') {
                    self.pos += 2;
                    Ok(Token::new(TokenKind::PipeGt, Span::new(pos, self.pos)))
                } else {
                    Err(
                        Diagnostic::error("不明な記号です: `|`（`|>` の意図ですか？）")
                            .at(self.file, Span::new(pos, pos + 1)),
                    )
                }
            }
            '-' => self.lex_hyphen(was_line_head),
            ':' => self.lex_colon(),
            '.' => self.lex_dot(),
            c if c.is_ascii_lowercase() => {
                let span = self.scan_kebab();
                let text = self.source[span.start..span.end].to_string();
                Ok(Token::new(TokenKind::Ident(text), span))
            }
            c if c.is_ascii_uppercase() => {
                let span = self.scan_upper();
                let text = self.source[span.start..span.end].to_string();
                Ok(Token::new(TokenKind::UpperIdent(text), span))
            }
            c if c.is_ascii_digit() => {
                let start = self.pos;
                self.lex_number(start)
            }
            other => {
                let pos = self.pos;
                Err(Diagnostic::error(format!("不明な文字です: `{other}`"))
                    .at(self.file, Span::new(pos, pos + other.len_utf8())))
            }
        }
    }

    fn lex_hyphen(&mut self, was_line_head: bool) -> Result<Token, Diagnostic> {
        let dash_pos = self.pos;
        let next = self.peek_char_at(dash_pos + 1);
        if next == Some('>') {
            self.pos += 2;
            return Ok(Token::new(TokenKind::Arrow, Span::new(dash_pos, self.pos)));
        }
        if was_line_head && next == Some(' ') {
            self.pos += 1;
            return Ok(Token::new(
                TokenKind::ListMarker,
                Span::new(dash_pos, self.pos),
            ));
        }
        if matches!(next, Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
            return self.lex_number(dash_pos);
        }
        Err(Diagnostic::error("この位置に `-` は書けません")
            .at(self.file, Span::new(dash_pos, dash_pos + 1))
            .with_label(
                "リスト要素(`- `)・負数(`-1`)・kebab-case識別子の一部のいずれでもありません",
            ))
    }

    fn lex_colon(&mut self) -> Result<Token, Diagnostic> {
        let colon_pos = self.pos;
        if matches!(self.peek_char_at(colon_pos + 1), Some(c) if c.is_ascii_lowercase()) {
            self.pos += 1;
            let word_span = self.scan_kebab();
            let word = self.source[word_span.start..word_span.end].to_string();
            let full_span = Span::new(colon_pos, word_span.end);
            if word == "pub" {
                Ok(Token::new(TokenKind::ColonWord(word), full_span))
            } else {
                Err(
                    Diagnostic::error(format!("ステップ1では使えないコロン語です: `:{word}`"))
                        .at(self.file, full_span)
                        .with_label("現在使えるのは `:pub` のみです"),
                )
            }
        } else {
            self.pos += 1;
            Ok(Token::new(TokenKind::Colon, Span::new(colon_pos, self.pos)))
        }
    }

    fn lex_dot(&mut self) -> Result<Token, Diagnostic> {
        let dot_pos = self.pos;
        let preceded_by_space_or_start =
            dot_pos == 0 || matches!(self.source.as_bytes()[dot_pos - 1], b' ' | b'\t' | b'\n');
        if preceded_by_space_or_start {
            self.pos += 1;
            if matches!(self.current_char_opt(), Some(c) if c.is_ascii_lowercase()) {
                let word_span = self.scan_kebab();
                let word = self.source[word_span.start..word_span.end].to_string();
                Ok(Token::new(
                    TokenKind::DotIdent(word),
                    Span::new(dot_pos, word_span.end),
                ))
            } else {
                Err(Diagnostic::error("`.` の直後に識別子が必要です")
                    .at(self.file, Span::new(dot_pos, self.pos)))
            }
        } else {
            self.pos += 1;
            Ok(Token::new(TokenKind::Dot, Span::new(dot_pos, self.pos)))
        }
    }

    fn lex_number(&mut self, start: usize) -> Result<Token, Diagnostic> {
        while matches!(self.current_char_opt(), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
        }
        let mut is_float = false;
        if self.current_char_opt() == Some('.')
            && matches!(self.peek_char_at(self.pos + 1), Some(c) if c.is_ascii_digit())
        {
            is_float = true;
            self.pos += 1;
            while matches!(self.current_char_opt(), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let text = &self.source[start..self.pos];
        let span = Span::new(start, self.pos);
        if is_float {
            let value: f64 = text.parse().map_err(|_| {
                Diagnostic::error("数値リテラルを解釈できません").at(self.file, span)
            })?;
            Ok(Token::new(TokenKind::Float(value), span))
        } else {
            let value: i64 = text.parse().map_err(|_| {
                Diagnostic::error("数値リテラルを解釈できません").at(self.file, span)
            })?;
            Ok(Token::new(TokenKind::Int(value), span))
        }
    }

    fn scan_kebab(&mut self) -> Span {
        let start = self.pos;
        self.pos += 1;
        loop {
            match self.current_char_opt() {
                Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {
                    self.pos += 1;
                }
                Some('-') => {
                    if matches!(self.peek_char_at(self.pos + 1), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
                    {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        Span::new(start, self.pos)
    }

    fn scan_upper(&mut self) -> Span {
        let start = self.pos;
        self.pos += 1;
        while matches!(self.current_char_opt(), Some(c) if c.is_ascii_alphanumeric()) {
            self.pos += 1;
        }
        Span::new(start, self.pos)
    }

    fn single(&mut self, kind: TokenKind) -> Result<Token, Diagnostic> {
        let start = self.pos;
        self.pos += 1;
        Ok(Token::new(kind, Span::new(start, self.pos)))
    }

    fn skip_inline_whitespace(&mut self) -> Result<(), Diagnostic> {
        loop {
            match self.current_char_opt() {
                Some(' ') => self.pos += 1,
                Some('\t') => {
                    let pos = self.pos;
                    return Err(Diagnostic::error("タブは使えません")
                        .at(self.file, Span::new(pos, pos + 1)));
                }
                _ => return Ok(()),
            }
        }
    }

    fn skip_to_end_of_line(&mut self) {
        while !self.at_eof() && self.current_char() != '\n' {
            self.pos += self.current_char().len_utf8();
        }
    }

    fn at_eof(&self) -> bool {
        self.pos >= self.source.len()
    }

    fn current_char(&self) -> char {
        self.source[self.pos..].chars().next().unwrap()
    }

    fn current_char_opt(&self) -> Option<char> {
        self.source[self.pos..].chars().next()
    }

    fn peek_char_at(&self, pos: usize) -> Option<char> {
        if pos >= self.source.len() {
            None
        } else {
            self.source[pos..].chars().next()
        }
    }
}

fn is_structural(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Indent | TokenKind::Dedent | TokenKind::Newline | TokenKind::Eof
    )
}

fn classify_at(source: &str, pos: usize) -> LineHead {
    let mut chars = source[pos..].chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => LineHead::Ident,
        Some(c) if c.is_ascii_uppercase() => LineHead::UpperIdent,
        Some('{') => LineHead::LBrace,
        Some('|') if chars.next() == Some('>') => LineHead::PipeGt,
        _ => LineHead::Text,
    }
}

pub fn tokenize(source: &str, file: FileId) -> Result<Vec<Token>, Diagnostic> {
    let mut lexer = Lexer::new(source, file);
    let mut tokens = Vec::new();
    loop {
        let token = lexer.next()?;
        let is_eof = token.kind == TokenKind::Eof;
        tokens.push(token);
        if is_eof {
            return Ok(tokens);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceMap;
    use std::path::PathBuf;

    fn file_id() -> FileId {
        let mut sources = SourceMap::new();
        sources.add_file(PathBuf::from("test.clum"), String::new())
    }

    fn tokens_of(src: &str) -> Vec<TokenKind> {
        tokenize(src, file_id())
            .unwrap_or_else(|e| panic!("字句解析に失敗しました: {}", render_for(src, &e)))
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    fn try_tokens_of(src: &str) -> Result<Vec<Token>, Diagnostic> {
        tokenize(src, file_id())
    }

    fn render_for(src: &str, diagnostic: &Diagnostic) -> String {
        let mut sources = SourceMap::new();
        sources.add_file(PathBuf::from("test.clum"), src.to_string());
        diagnostic.render(&sources)
    }

    fn error_message(src: &str) -> String {
        let err = try_tokens_of(src).expect_err("エラーを期待しました");
        render_for(src, &err)
    }

    #[test]
    fn empty_source_yields_only_eof() {
        assert_eq!(tokens_of(""), vec![TokenKind::Eof]);
    }

    #[test]
    fn simple_symbols() {
        assert_eq!(
            tokens_of("# ! |> -> = , : { } < >"),
            vec![
                TokenKind::Hash,
                TokenKind::Bang,
                TokenKind::PipeGt,
                TokenKind::Arrow,
                TokenKind::Eq,
                TokenKind::Comma,
                TokenKind::Colon,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::Lt,
                TokenKind::Gt,
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn at_reads_relative_path() {
        assert_eq!(
            tokens_of("@./components/card"),
            vec![
                TokenKind::At,
                TokenKind::Path("./components/card".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn at_without_space_reads_path_immediately() {
        assert_eq!(
            tokens_of("@js/console"),
            vec![
                TokenKind::At,
                TokenKind::Path("js/console".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn at_without_path_is_error() {
        let message = error_message("@\n");
        assert!(message.contains("パスが必要です"));
    }

    #[test]
    fn at_with_space_before_path_is_error() {
        let message = error_message("@ ./index\n");
        assert!(message.contains("`@` とパスのあいだに空白は書けません"));
        assert!(message.contains("@./index"));
    }

    #[test]
    fn at_with_only_spaces_is_error() {
        let message = error_message("@  \n");
        assert!(message.contains("パスが必要です"));
    }

    #[test]
    fn at_at_eof_without_path_is_error() {
        let message = error_message("@");
        assert!(message.contains("パスが必要です"));
    }

    #[test]
    fn path_followed_by_comment_is_stripped() {
        assert_eq!(
            tokens_of("@./index // メモ\n"),
            vec![
                TokenKind::At,
                TokenKind::Path("./index".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn path_containing_slash_slash_without_space_is_kept_intact() {
        assert_eq!(
            tokens_of("@js//console"),
            vec![
                TokenKind::At,
                TokenKind::Path("js//console".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn tab_mid_line_is_error() {
        let message = error_message("x\ty");
        assert!(message.contains("タブ"));
    }

    #[test]
    fn tab_inside_string_is_allowed() {
        let src = "'a\tb'";
        let tokens = try_tokens_of(src).unwrap();
        match &tokens[0].kind {
            TokenKind::Str(lit) => match &lit.segments[0] {
                StrSegment::Text(span) => assert_eq!(&src[span.start..span.end], "a\tb"),
                other => panic!("Text segment を期待しましたが {other:?} でした"),
            },
            other => panic!("Str を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn tab_inside_raw_text_is_allowed() {
        let mut sources = SourceMap::new();
        let file = sources.add_file(PathBuf::from("t.clum"), String::new());
        let src = "h .div\n  本\tです\n";
        let mut lexer = Lexer::new(src, file);
        assert_eq!(
            lexer.next().unwrap().kind,
            TokenKind::Ident("h".to_string())
        );
        assert_eq!(
            lexer.next().unwrap().kind,
            TokenKind::DotIdent("div".to_string())
        );
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Newline);
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Indent);
        assert_eq!(lexer.classify_line_head().unwrap(), LineHead::Text);
        let (text, _) = lexer.take_rest_of_line_raw().unwrap();
        assert_eq!(text, "本\tです");
    }

    #[test]
    fn lone_closing_brace_in_string_is_error() {
        let message = error_message("'a}b'");
        assert!(message.contains("`}}`"));
    }

    #[test]
    fn kebab_ident() {
        assert_eq!(
            tokens_of("current-target"),
            vec![
                TokenKind::Ident("current-target".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn kebab_ident_with_digits() {
        assert_eq!(
            tokens_of("a1-b2"),
            vec![
                TokenKind::Ident("a1-b2".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn upper_camel_ident() {
        assert_eq!(
            tokens_of("SaveItem"),
            vec![
                TokenKind::UpperIdent("SaveItem".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn upper_camel_does_not_consume_hyphen() {
        let mut sources = SourceMap::new();
        let file = sources.add_file(PathBuf::from("t.clum"), String::new());
        let mut lexer = Lexer::new("Foo-bar", file);
        let first = lexer.next().unwrap();
        assert_eq!(first.kind, TokenKind::UpperIdent("Foo".to_string()));
    }

    #[test]
    fn int_literal() {
        assert_eq!(
            tokens_of("42"),
            vec![TokenKind::Int(42), TokenKind::Newline, TokenKind::Eof]
        );
    }

    #[test]
    fn negative_int_literal() {
        assert_eq!(
            tokens_of("x -1"),
            vec![
                TokenKind::Ident("x".to_string()),
                TokenKind::Int(-1),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn float_literal() {
        assert_eq!(
            tokens_of("12.5"),
            vec![TokenKind::Float(12.5), TokenKind::Newline, TokenKind::Eof]
        );
    }

    #[test]
    fn negative_float_literal() {
        assert_eq!(
            tokens_of("-12.5"),
            vec![TokenKind::Float(-12.5), TokenKind::Newline, TokenKind::Eof]
        );
    }

    #[test]
    fn dot_ident_after_space() {
        assert_eq!(
            tokens_of("h .div"),
            vec![
                TokenKind::Ident("h".to_string()),
                TokenKind::DotIdent("div".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn field_access_dot_is_bare() {
        assert_eq!(
            tokens_of("page.title"),
            vec![
                TokenKind::Ident("page".to_string()),
                TokenKind::Dot,
                TokenKind::Ident("title".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn chained_field_access() {
        assert_eq!(
            tokens_of("e.current-target.value"),
            vec![
                TokenKind::Ident("e".to_string()),
                TokenKind::Dot,
                TokenKind::Ident("current-target".to_string()),
                TokenKind::Dot,
                TokenKind::Ident("value".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn dot_ident_at_line_head() {
        assert_eq!(
            tokens_of(".all"),
            vec![
                TokenKind::DotIdent("all".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn list_marker_at_line_head() {
        assert_eq!(
            tokens_of("xs =\n  - 1\n  - 2\n"),
            vec![
                TokenKind::Ident("xs".to_string()),
                TokenKind::Eq,
                TokenKind::Newline,
                TokenKind::Indent,
                TokenKind::ListMarker,
                TokenKind::Int(1),
                TokenKind::Newline,
                TokenKind::ListMarker,
                TokenKind::Int(2),
                TokenKind::Newline,
                TokenKind::Dedent,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn list_marker_dash_space_one_is_marker_then_positive_int() {
        assert_eq!(
            tokens_of("- 1"),
            vec![
                TokenKind::ListMarker,
                TokenKind::Int(1),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn negative_literal_written_tight_is_not_list_marker() {
        assert_eq!(
            tokens_of("-1"),
            vec![TokenKind::Int(-1), TokenKind::Newline, TokenKind::Eof]
        );
    }

    #[test]
    fn hyphen_space_mid_line_is_error() {
        let message = error_message("f a, - 1");
        assert!(message.contains("`-`"));
    }

    #[test]
    fn arrow_after_ident_no_space() {
        assert_eq!(
            tokens_of("x->"),
            vec![
                TokenKind::Ident("x".to_string()),
                TokenKind::Arrow,
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn comment_only_line_produces_no_tokens() {
        assert_eq!(
            tokens_of("// hello\nx\n"),
            vec![
                TokenKind::Ident("x".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn comment_mid_line_truncates_rest() {
        assert_eq!(
            tokens_of("x // trailing comment\n"),
            vec![
                TokenKind::Ident("x".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn indent_dedent_and_newline_sequence() {
        assert_eq!(
            tokens_of("a\n  b\nc\n"),
            vec![
                TokenKind::Ident("a".to_string()),
                TokenKind::Newline,
                TokenKind::Indent,
                TokenKind::Ident("b".to_string()),
                TokenKind::Newline,
                TokenKind::Dedent,
                TokenKind::Ident("c".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn multi_level_dedent_at_once() {
        assert_eq!(
            tokens_of("a\n  b\n    c\nd\n"),
            vec![
                TokenKind::Ident("a".to_string()),
                TokenKind::Newline,
                TokenKind::Indent,
                TokenKind::Ident("b".to_string()),
                TokenKind::Newline,
                TokenKind::Indent,
                TokenKind::Ident("c".to_string()),
                TokenKind::Newline,
                TokenKind::Dedent,
                TokenKind::Dedent,
                TokenKind::Ident("d".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn dedent_emitted_at_eof_without_trailing_newline() {
        assert_eq!(
            tokens_of("a\n  b"),
            vec![
                TokenKind::Ident("a".to_string()),
                TokenKind::Newline,
                TokenKind::Indent,
                TokenKind::Ident("b".to_string()),
                TokenKind::Newline,
                TokenKind::Dedent,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn blank_lines_do_not_affect_indentation() {
        assert_eq!(
            tokens_of("a\n\n  b\n\nc\n"),
            vec![
                TokenKind::Ident("a".to_string()),
                TokenKind::Newline,
                TokenKind::Indent,
                TokenKind::Ident("b".to_string()),
                TokenKind::Newline,
                TokenKind::Dedent,
                TokenKind::Ident("c".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn tab_in_indent_is_error() {
        let message = error_message("a\n\tb\n");
        assert!(message.contains("タブ"));
    }

    #[test]
    fn odd_indent_is_error() {
        let message = error_message("a\n   b\n");
        assert!(message.contains("2スペース単位"));
    }

    #[test]
    fn jump_indent_is_error() {
        let message = error_message("a\n    b\n");
        assert!(message.contains("1段階を超えて"));
    }

    #[test]
    fn pub_colon_word() {
        assert_eq!(
            tokens_of(":pub"),
            vec![
                TokenKind::ColonWord("pub".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unsupported_colon_word_is_error() {
        let message = error_message(":is");
        assert!(message.contains("コロン語"));
    }

    #[test]
    fn bare_colon_is_not_colon_word() {
        assert_eq!(
            tokens_of("x: T"),
            vec![
                TokenKind::Ident("x".to_string()),
                TokenKind::Colon,
                TokenKind::UpperIdent("T".to_string()),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn simple_string() {
        let tokens = tokens_of("'hello'");
        match &tokens[0] {
            TokenKind::Str(lit) => assert_eq!(lit.segments.len(), 1),
            other => panic!("Str を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn string_with_interpolation_content() {
        let src = "'hello {name}'";
        let tokens = try_tokens_of(src).unwrap();
        match &tokens[0].kind {
            TokenKind::Str(lit) => {
                assert_eq!(lit.segments.len(), 2);
                match &lit.segments[0] {
                    StrSegment::Text(span) => assert_eq!(&src[span.start..span.end], "hello "),
                    other => panic!("Text segment を期待しましたが {other:?} でした"),
                }
                match &lit.segments[1] {
                    StrSegment::Interp(inner, span) => {
                        assert_eq!(inner.len(), 1);
                        assert_eq!(inner[0].kind, TokenKind::Ident("name".to_string()));
                        assert_eq!(&src[span.start..span.end], "{name}");
                    }
                    other => panic!("Interp segment を期待しましたが {other:?} でした"),
                }
            }
            other => panic!("Str を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn string_double_brace_escape() {
        let src = "'{{literal}}'";
        let tokens = try_tokens_of(src).unwrap();
        match &tokens[0].kind {
            TokenKind::Str(lit) => {
                assert_eq!(lit.segments.len(), 1);
                match &lit.segments[0] {
                    StrSegment::Text(span) => {
                        assert_eq!(&src[span.start..span.end], "{{literal}}")
                    }
                    other => panic!("Text segment を期待しましたが {other:?} でした"),
                }
            }
            other => panic!("Str を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn unterminated_string_is_error() {
        let message = error_message("'hello");
        assert!(message.contains("閉じられていません"));
    }

    #[test]
    fn unterminated_string_at_newline_is_error() {
        let message = error_message("'hello\nworld'");
        assert!(message.contains("閉じられていません"));
    }

    #[test]
    fn interpolation_disallows_nested_string() {
        let message = error_message("'{'x'}'");
        assert!(message.contains("文字列リテラル"));
    }

    #[test]
    fn interpolation_disallows_brace() {
        let message = error_message("'{ { }'");
        assert!(message.contains("補間式の中に"));
    }

    #[test]
    fn interpolation_unterminated_is_error() {
        let message = error_message("'{name");
        assert!(message.contains("補間"));
    }

    #[test]
    fn take_rest_of_line_raw_returns_japanese_text() {
        let mut sources = SourceMap::new();
        let file = sources.add_file(PathBuf::from("t.clum"), String::new());
        let src = "h .div\n  こんにちは\n";
        let mut lexer = Lexer::new(src, file);

        assert_eq!(
            lexer.next().unwrap().kind,
            TokenKind::Ident("h".to_string())
        );
        assert_eq!(
            lexer.next().unwrap().kind,
            TokenKind::DotIdent("div".to_string())
        );
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Newline);
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Indent);

        assert_eq!(lexer.classify_line_head().unwrap(), LineHead::Text);
        let (text, span) = lexer.take_rest_of_line_raw().unwrap();
        assert_eq!(text, "こんにちは");
        assert_eq!(&src[span.start..span.end], "こんにちは");

        assert_eq!(lexer.next().unwrap().kind, TokenKind::Dedent);
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Eof);
    }

    #[test]
    fn classify_line_head_variants() {
        let mut sources = SourceMap::new();
        let file = sources.add_file(PathBuf::from("t.clum"), String::new());

        let mut ident_lexer = Lexer::new("h .div\n", file);
        assert_eq!(ident_lexer.classify_line_head().unwrap(), LineHead::Ident);

        let mut upper_lexer = Lexer::new("Card 'x'\n", file);
        assert_eq!(
            upper_lexer.classify_line_head().unwrap(),
            LineHead::UpperIdent
        );

        let mut brace_lexer = Lexer::new("{user.name}\n", file);
        assert_eq!(brace_lexer.classify_line_head().unwrap(), LineHead::LBrace);

        let mut pipe_lexer = Lexer::new("|> build\n", file);
        assert_eq!(pipe_lexer.classify_line_head().unwrap(), LineHead::PipeGt);

        let mut text_lexer = Lexer::new("こんにちは\n", file);
        assert_eq!(text_lexer.classify_line_head().unwrap(), LineHead::Text);
    }

    #[test]
    fn unknown_character_is_error() {
        let message = error_message("$");
        assert!(message.contains("不明な文字"));
    }

    #[test]
    fn span_of_indent_and_ident_are_exact() {
        let src = "a\n  hello\n";
        let tokens = try_tokens_of(src).unwrap();
        let indent = &tokens[2];
        assert_eq!(indent.kind, TokenKind::Indent);
        assert_eq!(indent.span, Span::new(2, 4));
        let ident = &tokens[3];
        assert_eq!(ident.kind, TokenKind::Ident("hello".to_string()));
        assert_eq!(ident.span, Span::new(4, 9));
    }

    #[test]
    fn peek_does_not_consume() {
        let mut sources = SourceMap::new();
        let file = sources.add_file(PathBuf::from("t.clum"), String::new());
        let mut lexer = Lexer::new("a b\n", file);
        let peeked = lexer.peek().unwrap();
        assert_eq!(peeked.kind, TokenKind::Ident("a".to_string()));
        let next = lexer.next().unwrap();
        assert_eq!(next.kind, TokenKind::Ident("a".to_string()));
        let next2 = lexer.next().unwrap();
        assert_eq!(next2.kind, TokenKind::Ident("b".to_string()));
    }

    #[test]
    fn eof_is_idempotent() {
        let mut sources = SourceMap::new();
        let file = sources.add_file(PathBuf::from("t.clum"), String::new());
        let mut lexer = Lexer::new("a\n", file);
        assert_eq!(
            lexer.next().unwrap().kind,
            TokenKind::Ident("a".to_string())
        );
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Newline);
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Eof);
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Eof);
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Eof);
    }

    #[test]
    fn peek_structural_reports_indent_then_dedent() {
        let mut sources = SourceMap::new();
        let file = sources.add_file(PathBuf::from("t.clum"), String::new());
        let src = "h .div\n  こんにちは\n";
        let mut lexer = Lexer::new(src, file);
        assert_eq!(
            lexer.next().unwrap().kind,
            TokenKind::Ident("h".to_string())
        );
        assert_eq!(
            lexer.next().unwrap().kind,
            TokenKind::DotIdent("div".to_string())
        );
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Newline);

        assert_eq!(lexer.peek_structural().unwrap(), Some(TokenKind::Indent));
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Indent);

        assert_eq!(lexer.peek_structural().unwrap(), None);
        assert_eq!(lexer.classify_line_head().unwrap(), LineHead::Text);
        let (text, _) = lexer.take_text_line().unwrap();
        assert_eq!(text.segments.len(), 1);

        assert_eq!(lexer.peek_structural().unwrap(), Some(TokenKind::Dedent));
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Dedent);
        assert_eq!(lexer.peek_structural().unwrap(), Some(TokenKind::Eof));
    }

    #[test]
    fn take_text_line_splits_text_and_interp() {
        let mut sources = SourceMap::new();
        let file = sources.add_file(PathBuf::from("t.clum"), String::new());
        let src = "h .li\n  {page.title} さん\n";
        let mut lexer = Lexer::new(src, file);
        assert_eq!(
            lexer.next().unwrap().kind,
            TokenKind::Ident("h".to_string())
        );
        assert_eq!(
            lexer.next().unwrap().kind,
            TokenKind::DotIdent("li".to_string())
        );
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Newline);
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Indent);

        assert_eq!(lexer.classify_line_head().unwrap(), LineHead::LBrace);
        let (line, _) = lexer.take_text_line().unwrap();
        assert_eq!(line.segments.len(), 2);
        match &line.segments[0] {
            StrSegment::Interp(tokens, _) => {
                assert_eq!(tokens[0].kind, TokenKind::Ident("page".to_string()));
                assert_eq!(tokens[1].kind, TokenKind::Dot);
                assert_eq!(tokens[2].kind, TokenKind::Ident("title".to_string()));
            }
            other => panic!("Interp を期待しましたが {other:?} でした"),
        }
        match &line.segments[1] {
            StrSegment::Text(span) => assert_eq!(&src[span.start..span.end], " さん"),
            other => panic!("Text を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn take_text_line_allows_string_in_interp() {
        let mut sources = SourceMap::new();
        let file = sources.add_file(PathBuf::from("t.clum"), String::new());
        let src = "h .p\n  {'h で始まる文'}\n";
        let mut lexer = Lexer::new(src, file);
        assert_eq!(
            lexer.next().unwrap().kind,
            TokenKind::Ident("h".to_string())
        );
        assert_eq!(
            lexer.next().unwrap().kind,
            TokenKind::DotIdent("p".to_string())
        );
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Newline);
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Indent);

        assert_eq!(lexer.classify_line_head().unwrap(), LineHead::LBrace);
        let (line, _) = lexer.take_text_line().unwrap();
        assert_eq!(line.segments.len(), 1);
        match &line.segments[0] {
            StrSegment::Interp(tokens, _) => {
                assert_eq!(tokens.len(), 1);
                assert!(matches!(tokens[0].kind, TokenKind::Str(_)));
            }
            other => panic!("Interp を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn take_text_line_keeps_double_brace_raw() {
        let mut sources = SourceMap::new();
        let file = sources.add_file(PathBuf::from("t.clum"), String::new());
        let src = "h .p\n  a {{b}} c\n";
        let mut lexer = Lexer::new(src, file);
        for _ in 0..4 {
            let _ = lexer.next().unwrap();
        }
        let (line, _) = lexer.take_text_line().unwrap();
        assert_eq!(line.segments.len(), 1);
        match &line.segments[0] {
            StrSegment::Text(span) => assert_eq!(&src[span.start..span.end], "a {{b}} c"),
            other => panic!("Text を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn take_text_line_lone_closing_brace_is_error() {
        let src = "h .p\n  a}b\n";
        let mut sources = SourceMap::new();
        let file = sources.add_file(PathBuf::from("t.clum"), src.to_string());
        let mut lexer = Lexer::new(src, file);
        for _ in 0..4 {
            let _ = lexer.next().unwrap();
        }
        let err = lexer.take_text_line().expect_err("エラーを期待しました");
        assert!(err.render(&sources).contains("`}}`"));
    }

    #[test]
    fn text_lines_concatenated_via_raw_grab() {
        let mut sources = SourceMap::new();
        let file = sources.add_file(PathBuf::from("t.clum"), String::new());
        let src = "h .div\n  本日は\n  晴天\n  です\n";
        let mut lexer = Lexer::new(src, file);
        assert_eq!(
            lexer.next().unwrap().kind,
            TokenKind::Ident("h".to_string())
        );
        assert_eq!(
            lexer.next().unwrap().kind,
            TokenKind::DotIdent("div".to_string())
        );
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Newline);
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Indent);

        assert_eq!(lexer.classify_line_head().unwrap(), LineHead::Text);
        let (a, _) = lexer.take_rest_of_line_raw().unwrap();
        assert_eq!(lexer.classify_line_head().unwrap(), LineHead::Text);
        let (b, _) = lexer.take_rest_of_line_raw().unwrap();
        assert_eq!(lexer.classify_line_head().unwrap(), LineHead::Text);
        let (c, _) = lexer.take_rest_of_line_raw().unwrap();
        assert_eq!(a, "本日は");
        assert_eq!(b, "晴天");
        assert_eq!(c, "です");

        assert_eq!(lexer.next().unwrap().kind, TokenKind::Dedent);
        assert_eq!(lexer.next().unwrap().kind, TokenKind::Eof);
    }
}
