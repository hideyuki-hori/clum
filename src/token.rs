use crate::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Indent,
    Dedent,
    Newline,
    Eof,
    Ident(String),
    UpperIdent(String),
    DotIdent(String),
    Dot,
    ColonWord(String),
    Path(String),
    Hash,
    At,
    Bang,
    PipeGt,
    Arrow,
    Eq,
    Comma,
    Colon,
    LBrace,
    RBrace,
    Lt,
    Gt,
    ListMarker,
    Int(i64),
    Float(f64),
    Str(StrLiteral),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrLiteral {
    pub segments: Vec<StrSegment>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrSegment {
    Text(Span),
    Interp(Vec<Token>, Span),
}
