use std::collections::VecDeque;
use std::mem::discriminant;

use crate::ast::{
    Attr, Binding, BindingKind, Child, Component, Decl, Element, Expr, Field, Import, Item, Module,
    Name, RecordField, StrPart, TypeExpr, VecElem,
};
use crate::diag::Diagnostic;
use crate::lexer::{Lexer, LineHead};
use crate::source::FileId;
use crate::span::Span;
use crate::token::{StrLiteral, StrSegment, Token, TokenKind};

pub fn parse(source: &str, file: FileId) -> Result<Module, Diagnostic> {
    let mut parser = Parser::new(source, file);
    parser.parse_module()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEnd {
    Open,
    Closed,
}

struct Parser<'a> {
    source: &'a str,
    file: FileId,
    lexer: Lexer<'a>,
    buf: VecDeque<Token>,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str, file: FileId) -> Self {
        Self {
            source,
            file,
            lexer: Lexer::new(source, file),
            buf: VecDeque::new(),
        }
    }

    fn err(&self, message: impl Into<String>, span: Span) -> Diagnostic {
        Diagnostic::error(message).at(self.file, span)
    }

    fn fill(&mut self, n: usize) -> Result<(), Diagnostic> {
        while self.buf.len() <= n {
            let token = self.lexer.next()?;
            self.buf.push_back(token);
        }
        Ok(())
    }

    fn peek(&mut self, n: usize) -> Result<Token, Diagnostic> {
        self.fill(n)?;
        Ok(self.buf[n].clone())
    }

    fn bump(&mut self) -> Result<Token, Diagnostic> {
        if let Some(token) = self.buf.pop_front() {
            return Ok(token);
        }
        self.lexer.next()
    }

    fn expect(&mut self, want: &TokenKind, message: &str) -> Result<Token, Diagnostic> {
        let token = self.peek(0)?;
        if discriminant(&token.kind) == discriminant(want) {
            self.bump()
        } else {
            Err(self.err(message, token.span))
        }
    }

    fn expect_ident(&mut self, message: &str) -> Result<Name, Diagnostic> {
        let token = self.peek(0)?;
        match token.kind {
            TokenKind::Ident(text) => {
                self.bump()?;
                Ok(Name::new(text, token.span))
            }
            _ => Err(self.err(message, token.span)),
        }
    }

    fn expect_upper(&mut self, message: &str) -> Result<Name, Diagnostic> {
        let token = self.peek(0)?;
        match token.kind {
            TokenKind::UpperIdent(text) => {
                self.bump()?;
                Ok(Name::new(text, token.span))
            }
            _ => Err(self.err(message, token.span)),
        }
    }

    fn parse_module(&mut self) -> Result<Module, Diagnostic> {
        let mut items = Vec::new();
        loop {
            let token = self.peek(0)?;
            match &token.kind {
                TokenKind::Eof => break,
                TokenKind::Hash => items.push(Item::Decl(self.parse_decl()?)),
                TokenKind::At => items.push(Item::Import(self.parse_import()?)),
                TokenKind::ColonWord(word) if word == "pub" => {
                    items.push(Item::Binding(self.parse_pub_binding()?));
                }
                TokenKind::Ident(_) => items.push(Item::Binding(self.parse_binding(false)?)),
                TokenKind::UpperIdent(_) => {
                    items.push(Item::Expr(self.parse_record_construction()?))
                }
                _ => return Err(self.err("トップレベルに書けない行です", token.span)),
            }
        }
        Ok(Module {
            items,
            span: Span::new(0, self.source.len()),
        })
    }

    fn parse_decl(&mut self) -> Result<Decl, Diagnostic> {
        let hash = self.bump()?;
        let name = self.expect_upper("`#` の後には定義名（大文字始まり）が必要です")?;
        let mut params = Vec::new();
        if !matches!(self.peek(0)?.kind, TokenKind::Newline | TokenKind::Arrow) {
            loop {
                let field_name = self.expect_ident("フィールド名が必要です")?;
                self.expect(&TokenKind::Colon, "フィールドは `名前: 型` の形で書きます")?;
                let ty = self.parse_type()?;
                let span = Span::new(field_name.span.start, ty.span().end);
                params.push(Field {
                    name: field_name,
                    ty,
                    span,
                });
                if matches!(self.peek(0)?.kind, TokenKind::Comma) {
                    self.bump()?;
                } else {
                    break;
                }
            }
        }
        let mut ret = None;
        if matches!(self.peek(0)?.kind, TokenKind::Arrow) {
            self.bump()?;
            ret = Some(self.parse_type()?);
        }
        if params.is_empty() {
            return Err(self.err(
                "宣言には少なくとも1つのフィールドまたは引数が必要です",
                name.span,
            ));
        }
        self.expect(&TokenKind::Newline, "宣言の行末で予期しないトークンです")?;
        let end = match &ret {
            Some(ty) => ty.span().end,
            None => params.last().unwrap().span.end,
        };
        Ok(Decl {
            name,
            params,
            ret,
            span: Span::new(hash.span.start, end),
        })
    }

    fn parse_type(&mut self) -> Result<TypeExpr, Diagnostic> {
        let token = self.peek(0)?;
        let name = match token.kind {
            TokenKind::UpperIdent(text) => Name::new(text, token.span),
            TokenKind::Ident(text) => Name::new(text, token.span),
            _ => return Err(self.err("型名が必要です", token.span)),
        };
        self.bump()?;
        if matches!(self.peek(0)?.kind, TokenKind::Lt) {
            self.finish_generic_type(name)
        } else {
            Ok(TypeExpr::Name {
                span: name.span,
                name,
            })
        }
    }

    fn finish_generic_type(&mut self, name: Name) -> Result<TypeExpr, Diagnostic> {
        self.expect(&TokenKind::Lt, "`<` が必要です")?;
        let arg = self.parse_type()?;
        let gt = self.expect(&TokenKind::Gt, "型引数を閉じる `>` が必要です")?;
        Ok(TypeExpr::Generic {
            span: Span::new(name.span.start, gt.span.end),
            name,
            arg: Box::new(arg),
        })
    }

    fn parse_import(&mut self) -> Result<Import, Diagnostic> {
        let at = self.bump()?;
        let path_token = self.peek(0)?;
        let path = match path_token.kind {
            TokenKind::Path(text) => {
                self.bump()?;
                Name::new(text, path_token.span)
            }
            _ => return Err(self.err("import するパスが必要です", path_token.span)),
        };
        self.expect(&TokenKind::Newline, "import パスのあとは改行してください")?;
        let indent = self.peek(0)?;
        if !matches!(indent.kind, TokenKind::Indent) {
            return Err(self.err(
                "import する名前をインデントして列挙してください",
                indent.span,
            ));
        }
        self.bump()?;
        let mut names = Vec::new();
        loop {
            let token = self.peek(0)?;
            match token.kind {
                TokenKind::Ident(text) => {
                    self.bump()?;
                    names.push(Name::new(text, token.span));
                }
                TokenKind::UpperIdent(text) => {
                    self.bump()?;
                    names.push(Name::new(text, token.span));
                }
                TokenKind::Dedent => {
                    self.bump()?;
                    break;
                }
                _ => return Err(self.err("import する名前（識別子）が必要です", token.span)),
            }
            self.expect(&TokenKind::Newline, "import する名前は1行に1つ書きます")?;
        }
        if names.is_empty() {
            return Err(self.err("import する名前が1つもありません", at.span));
        }
        let end = names.last().unwrap().span.end;
        Ok(Import {
            path,
            names,
            span: Span::new(at.span.start, end),
        })
    }

    fn parse_pub_binding(&mut self) -> Result<Binding, Diagnostic> {
        let pub_token = self.bump()?;
        self.expect(&TokenKind::Newline, "`:pub` は単独の行に書きます")?;
        let next = self.peek(0)?;
        if !matches!(next.kind, TokenKind::Ident(_)) {
            return Err(self.err("`:pub` の直後には束縛が必要です", next.span));
        }
        let mut binding = self.parse_binding(true)?;
        if let BindingKind::Value { ty: None, name, .. } = &binding.kind {
            return Err(self.err("`:pub` を付ける値束縛には型注釈が必要です", name.span));
        }
        binding.span = Span::new(pub_token.span.start, binding.span.end);
        Ok(binding)
    }

    fn parse_binding(&mut self, is_pub: bool) -> Result<Binding, Diagnostic> {
        let name = self.expect_ident("束縛名が必要です")?;
        let start = name.span.start;
        let token = self.peek(0)?;
        match token.kind {
            TokenKind::Eq => {
                self.bump()?;
                let value = self.parse_expr_value()?;
                let span = Span::new(start, value.span().end);
                Ok(Binding {
                    is_pub,
                    kind: BindingKind::Value {
                        name,
                        ty: None,
                        value,
                    },
                    span,
                })
            }
            TokenKind::Colon => {
                self.bump()?;
                self.parse_binding_after_colon(is_pub, name, start)
            }
            _ => Err(self.err("束縛には `=` か `:` が必要です", token.span)),
        }
    }

    fn parse_binding_after_colon(
        &mut self,
        is_pub: bool,
        name: Name,
        start: usize,
    ) -> Result<Binding, Diagnostic> {
        let head = self.peek(0)?;
        let head_name = match head.kind {
            TokenKind::UpperIdent(text) => Name::new(text, head.span),
            TokenKind::Ident(text) => Name::new(text, head.span),
            _ => return Err(self.err("型名または定義名が必要です", head.span)),
        };
        self.bump()?;
        let after = self.peek(0)?;
        match after.kind {
            TokenKind::Lt => {
                let ty = self.finish_generic_type(head_name)?;
                self.expect(&TokenKind::Eq, "型注釈のあとには `=` が必要です")?;
                let value = self.parse_expr_value()?;
                let span = Span::new(start, value.span().end);
                Ok(Binding {
                    is_pub,
                    kind: BindingKind::Value {
                        name,
                        ty: Some(ty),
                        value,
                    },
                    span,
                })
            }
            TokenKind::Eq => {
                self.bump()?;
                let ty = TypeExpr::Name {
                    span: head_name.span,
                    name: head_name,
                };
                let value = self.parse_expr_value()?;
                let span = Span::new(start, value.span().end);
                Ok(Binding {
                    is_pub,
                    kind: BindingKind::Value {
                        name,
                        ty: Some(ty),
                        value,
                    },
                    span,
                })
            }
            TokenKind::Ident(_) | TokenKind::Arrow => {
                let mut params = Vec::new();
                if matches!(self.peek(0)?.kind, TokenKind::Ident(_)) {
                    loop {
                        let param = self.expect_ident("引数名が必要です")?;
                        params.push(param);
                        if matches!(self.peek(0)?.kind, TokenKind::Comma) {
                            self.bump()?;
                        } else {
                            break;
                        }
                    }
                }
                self.expect(&TokenKind::Arrow, "実装束縛には `->` が必要です")?;
                let body = self.parse_arrow_body()?;
                let span = Span::new(start, body.span().end);
                Ok(Binding {
                    is_pub,
                    kind: BindingKind::Impl {
                        name,
                        def: head_name,
                        params,
                        body,
                    },
                    span,
                })
            }
            _ => Err(self.err(
                "`:` のあとの束縛の形が不正です（型注釈なら `= 式`、実装なら `引数... -> 本体`）",
                after.span,
            )),
        }
    }

    fn parse_expr_value(&mut self) -> Result<Expr, Diagnostic> {
        let token = self.peek(0)?;
        match &token.kind {
            TokenKind::Ident(text)
                if text == "h" && matches!(self.peek(1)?.kind, TokenKind::DotIdent(_)) =>
            {
                Ok(Expr::Element(self.parse_element()?))
            }
            TokenKind::UpperIdent(_) => self.parse_record_construction(),
            TokenKind::Newline => {
                self.bump()?;
                let indent = self.peek(0)?;
                if !matches!(indent.kind, TokenKind::Indent) {
                    return Err(self.err("`=` の右辺が空です（値かリストが必要です）", indent.span));
                }
                self.bump()?;
                let first = self.peek(0)?;
                if !matches!(first.kind, TokenKind::ListMarker) {
                    return Err(self.err(
                        "`=` の後に改行した場合はリスト（`- ...`）を書きます",
                        first.span,
                    ));
                }
                let (elems, span) = self.parse_vec_elems()?;
                self.expect(&TokenKind::Dedent, "リストの終わりが不正です")?;
                Ok(Expr::Vec { elems, span })
            }
            _ => {
                let (expr, end) = self.parse_line_expr()?;
                if end == LineEnd::Open {
                    self.expect(&TokenKind::Newline, "式の行末で予期しないトークンです")?;
                }
                self.parse_multiline_pipe(expr)
            }
        }
    }

    fn parse_field_value(&mut self) -> Result<Expr, Diagnostic> {
        let token = self.peek(0)?;
        match &token.kind {
            TokenKind::Ident(text)
                if text == "h" && matches!(self.peek(1)?.kind, TokenKind::DotIdent(_)) =>
            {
                Ok(Expr::Element(self.parse_element()?))
            }
            TokenKind::UpperIdent(_) => self.parse_record_construction(),
            _ => {
                let (expr, end) = self.parse_line_expr()?;
                if end == LineEnd::Open {
                    self.expect(
                        &TokenKind::Newline,
                        "フィールドの値の行末で予期しないトークンです",
                    )?;
                }
                Ok(expr)
            }
        }
    }

    fn parse_multiline_pipe(&mut self, mut expr: Expr) -> Result<Expr, Diagnostic> {
        if !matches!(self.peek(0)?.kind, TokenKind::Indent) {
            return Ok(expr);
        }
        self.bump()?;
        loop {
            let pipe = self.peek(0)?;
            if !matches!(pipe.kind, TokenKind::PipeGt) {
                return Err(self.err("継続行は `|>` で始まる必要があります", pipe.span));
            }
            self.bump()?;
            let (stage, _) = self.parse_pipe_stage()?;
            let span = Span::new(expr.span().start, stage.span().end);
            expr = Expr::Pipe {
                lhs: Box::new(expr),
                rhs: Box::new(stage),
                span,
            };
            if matches!(self.peek(0)?.kind, TokenKind::Newline) {
                self.bump()?;
            }
            match self.peek(0)?.kind {
                TokenKind::Dedent => {
                    self.bump()?;
                    break;
                }
                TokenKind::PipeGt => continue,
                _ => {
                    let token = self.peek(0)?;
                    return Err(self.err("継続行は `|>` で始まる必要があります", token.span));
                }
            }
        }
        Ok(expr)
    }

    fn parse_line_expr(&mut self) -> Result<(Expr, LineEnd), Diagnostic> {
        if matches!(self.peek(0)?.kind, TokenKind::Ident(_))
            && matches!(self.peek(1)?.kind, TokenKind::Arrow)
        {
            return self.parse_lambda();
        }
        let (mut lhs, mut end) = self.parse_app()?;
        while matches!(self.peek(0)?.kind, TokenKind::PipeGt) {
            self.bump()?;
            let (stage, stage_end) = self.parse_pipe_stage()?;
            end = stage_end;
            let span = Span::new(lhs.span().start, stage.span().end);
            lhs = Expr::Pipe {
                lhs: Box::new(lhs),
                rhs: Box::new(stage),
                span,
            };
        }
        Ok((lhs, end))
    }

    fn parse_lambda(&mut self) -> Result<(Expr, LineEnd), Diagnostic> {
        let param = self.expect_ident("ラムダの引数名が必要です")?;
        self.expect(&TokenKind::Arrow, "ラムダには `->` が必要です")?;
        let (body, end) = self.parse_lambda_body()?;
        let span = Span::new(param.span.start, body.span().end);
        Ok((
            Expr::Lambda {
                param,
                body: Box::new(body),
                span,
            },
            end,
        ))
    }

    fn parse_lambda_body(&mut self) -> Result<(Expr, LineEnd), Diagnostic> {
        if matches!(self.peek(0)?.kind, TokenKind::Newline) {
            self.bump()?;
            self.expect(&TokenKind::Indent, "ラムダの本体をインデントしてください")?;
            let body = self.parse_block_body()?;
            Ok((body, LineEnd::Closed))
        } else {
            self.parse_line_expr()
        }
    }

    fn parse_arrow_body(&mut self) -> Result<Expr, Diagnostic> {
        if matches!(self.peek(0)?.kind, TokenKind::Newline) {
            self.bump()?;
            self.expect(&TokenKind::Indent, "本体をインデントしてください")?;
            self.parse_block_body()
        } else {
            self.parse_expr_value()
        }
    }

    fn parse_block_body(&mut self) -> Result<Expr, Diagnostic> {
        let start = self.peek(0)?.span.start;
        let mut bindings = Vec::new();
        let result;
        loop {
            let is_binding = matches!(self.peek(0)?.kind, TokenKind::Ident(_))
                && matches!(self.peek(1)?.kind, TokenKind::Eq | TokenKind::Colon);
            if is_binding {
                bindings.push(self.parse_binding(false)?);
            } else {
                result = self.parse_expr_value()?;
                break;
            }
        }
        self.expect(
            &TokenKind::Dedent,
            "ブロックの最後の式のあとに行を続けられません",
        )?;
        let span = Span::new(start, result.span().end);
        if bindings.is_empty() {
            Ok(result)
        } else {
            Ok(Expr::Block {
                bindings,
                result: Box::new(result),
                span,
            })
        }
    }

    fn parse_pipe_stage(&mut self) -> Result<(Expr, LineEnd), Diagnostic> {
        if matches!(self.peek(0)?.kind, TokenKind::Bang) {
            let bang = self.bump()?;
            Ok((Expr::Bang { span: bang.span }, LineEnd::Open))
        } else {
            self.parse_app()
        }
    }

    fn parse_app(&mut self) -> Result<(Expr, LineEnd), Diagnostic> {
        let head = self.parse_primary()?;
        if self.starts_arg()? {
            let (first, mut end) = self.parse_arg()?;
            let mut args = vec![first];
            while matches!(self.peek(0)?.kind, TokenKind::Comma) {
                self.bump()?;
                let (arg, arg_end) = self.parse_arg()?;
                end = arg_end;
                args.push(arg);
            }
            let span = Span::new(head.span().start, args.last().unwrap().span().end);
            Ok((
                Expr::App {
                    func: Box::new(head),
                    args,
                    span,
                },
                end,
            ))
        } else {
            Ok((head, LineEnd::Open))
        }
    }

    fn starts_arg(&mut self) -> Result<bool, Diagnostic> {
        Ok(matches!(
            self.peek(0)?.kind,
            TokenKind::Int(_) | TokenKind::Float(_) | TokenKind::Str(_) | TokenKind::Ident(_)
        ))
    }

    fn parse_arg(&mut self) -> Result<(Expr, LineEnd), Diagnostic> {
        if matches!(self.peek(0)?.kind, TokenKind::Ident(_))
            && matches!(self.peek(1)?.kind, TokenKind::Arrow)
        {
            return self.parse_lambda();
        }
        let expr = self.parse_simple()?;
        Ok((expr, LineEnd::Open))
    }

    fn parse_primary(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_simple()
    }

    fn parse_simple(&mut self) -> Result<Expr, Diagnostic> {
        let token = self.peek(0)?;
        let mut expr = match token.kind {
            TokenKind::Int(value) => {
                self.bump()?;
                Expr::Int {
                    value,
                    span: token.span,
                }
            }
            TokenKind::Float(value) => {
                self.bump()?;
                Expr::Float {
                    value,
                    span: token.span,
                }
            }
            TokenKind::Str(ref lit) => {
                let str_expr = self.build_str(lit, token.span)?;
                self.bump()?;
                str_expr
            }
            TokenKind::Ident(text) => {
                self.bump()?;
                Expr::Var {
                    name: Name::new(text, token.span),
                    span: token.span,
                }
            }
            TokenKind::Bang => {
                return Err(self.err("`!` はパイプ `|>` の右側でのみ使えます", token.span));
            }
            _ => return Err(self.err("式が必要です", token.span)),
        };
        while matches!(self.peek(0)?.kind, TokenKind::Dot) {
            self.bump()?;
            let field = self.expect_ident("フィールド名（小文字の識別子）が必要です")?;
            let span = Span::new(expr.span().start, field.span.end);
            expr = Expr::Field {
                base: Box::new(expr),
                field,
                span,
            };
        }
        Ok(expr)
    }

    fn parse_record_construction(&mut self) -> Result<Expr, Diagnostic> {
        let name_token = self.bump()?;
        let name = match name_token.kind {
            TokenKind::UpperIdent(text) => Name::new(text, name_token.span),
            _ => return Err(self.err("レコード名（大文字始まり）が必要です", name_token.span)),
        };
        let after = self.peek(0)?;
        if !matches!(after.kind, TokenKind::Newline) {
            return Err(self.err(
                "レコードはフィールドを次の行からインデントして書きます（1行形式はありません）",
                after.span,
            ));
        }
        self.bump()?;
        self.expect(
            &TokenKind::Indent,
            "レコードのフィールドをインデントしてください",
        )?;
        let mut fields = Vec::new();
        loop {
            let token = self.peek(0)?;
            match token.kind {
                TokenKind::PipeGt | TokenKind::Dedent => break,
                TokenKind::Ident(_) => fields.push(self.parse_record_field()?),
                _ => return Err(self.err("フィールド名または `|>` が必要です", token.span)),
            }
        }
        let fields_end = fields.last().map(|f| f.span.end).unwrap_or(name.span.end);
        let mut result = Expr::Record {
            name,
            fields,
            span: Span::new(name_token.span.start, fields_end),
        };
        while matches!(self.peek(0)?.kind, TokenKind::PipeGt) {
            self.bump()?;
            let (stage, _) = self.parse_pipe_stage()?;
            let span = Span::new(result.span().start, stage.span().end);
            result = Expr::Pipe {
                lhs: Box::new(result),
                rhs: Box::new(stage),
                span,
            };
            if matches!(self.peek(0)?.kind, TokenKind::Newline) {
                self.bump()?;
            }
        }
        self.expect(&TokenKind::Dedent, "レコードの終わりが不正です")?;
        Ok(result)
    }

    fn parse_record_field(&mut self) -> Result<RecordField, Diagnostic> {
        let name = self.expect_ident("フィールド名が必要です")?;
        self.expect(&TokenKind::Colon, "フィールドは `名前: 値` の形で書きます")?;
        if matches!(self.peek(0)?.kind, TokenKind::Newline) {
            self.bump()?;
            self.expect(&TokenKind::Indent, "フィールドの値をインデントしてください")?;
            let first = self.peek(0)?;
            if !matches!(first.kind, TokenKind::ListMarker) {
                return Err(self.err(
                    "改行したフィールドの値はリスト（`- ...`）である必要があります",
                    first.span,
                ));
            }
            let (elems, elems_span) = self.parse_vec_elems()?;
            self.expect(&TokenKind::Dedent, "リストの終わりが不正です")?;
            let span = Span::new(name.span.start, elems_span.end);
            Ok(RecordField {
                name,
                value: Expr::Vec {
                    elems,
                    span: elems_span,
                },
                span,
            })
        } else {
            let value = self.parse_field_value()?;
            let span = Span::new(name.span.start, value.span().end);
            Ok(RecordField { name, value, span })
        }
    }

    fn parse_vec_elems(&mut self) -> Result<(Vec<VecElem>, Span), Diagnostic> {
        let start = self.peek(0)?.span.start;
        let mut end = start;
        let mut elems = Vec::new();
        loop {
            if !matches!(self.peek(0)?.kind, TokenKind::ListMarker) {
                break;
            }
            let marker = self.bump()?;
            let is_record = matches!(self.peek(0)?.kind, TokenKind::Ident(_))
                && matches!(self.peek(1)?.kind, TokenKind::Colon);
            if is_record {
                let (fields, span) = self.parse_inline_record_fields(marker.span.start)?;
                end = span.end;
                elems.push(VecElem::Record { fields, span });
            } else {
                let (expr, line_end) = self.parse_line_expr()?;
                end = expr.span().end;
                if line_end != LineEnd::Open {
                    return Err(self.err(
                        "リスト要素に複数行の式は書けません。先に束縛してから名前で参照してください",
                        expr.span(),
                    ));
                }
                self.expect(
                    &TokenKind::Newline,
                    "リスト要素の行末で予期しないトークンです",
                )?;
                if matches!(self.peek(0)?.kind, TokenKind::Indent) {
                    let token = self.peek(0)?;
                    return Err(self.err(
                        "リスト要素に複数行の式は書けません。先に束縛してから名前で参照してください",
                        token.span,
                    ));
                }
                elems.push(VecElem::Expr(expr));
            }
        }
        Ok((elems, Span::new(start, end)))
    }

    fn parse_inline_record_fields(
        &mut self,
        start: usize,
    ) -> Result<(Vec<RecordField>, Span), Diagnostic> {
        let mut fields = Vec::new();
        let first = self.parse_record_field()?;
        let mut end = first.span.end;
        fields.push(first);
        if matches!(self.peek(0)?.kind, TokenKind::Indent) {
            self.bump()?;
            loop {
                let is_field = matches!(self.peek(0)?.kind, TokenKind::Ident(_))
                    && matches!(self.peek(1)?.kind, TokenKind::Colon);
                if is_field {
                    let field = self.parse_record_field()?;
                    end = field.span.end;
                    fields.push(field);
                } else {
                    break;
                }
            }
            self.expect(
                &TokenKind::Dedent,
                "リスト要素のフィールドの終わりが不正です",
            )?;
        }
        Ok((fields, Span::new(start, end)))
    }

    fn parse_element(&mut self) -> Result<Element, Diagnostic> {
        let h_token = self.bump()?;
        let tag_token = self.peek(0)?;
        let tag = match tag_token.kind {
            TokenKind::DotIdent(text) => {
                self.bump()?;
                Name::new(text, tag_token.span)
            }
            _ => {
                return Err(self
                    .err(
                        "`h` の直後にはタグ（`.div` など）が必要です",
                        tag_token.span,
                    )
                    .with_label("`h` で始まるテキストは {'...'} で埋め込みます"));
            }
        };
        let attrs = self.parse_attrs()?;
        self.expect(&TokenKind::Newline, "要素の行末で予期しないトークンです")?;
        let children = self.parse_child_block()?;
        let end = children_end(&children)
            .or_else(|| attrs.last().map(|a| a.span.end))
            .unwrap_or(tag.span.end);
        Ok(Element {
            tag,
            attrs,
            children,
            span: Span::new(h_token.span.start, end),
        })
    }

    fn parse_attrs(&mut self) -> Result<Vec<Attr>, Diagnostic> {
        let mut attrs = Vec::new();
        loop {
            let token = self.peek(0)?;
            match token.kind {
                TokenKind::Newline | TokenKind::Eof => break,
                TokenKind::Ident(text) => {
                    self.bump()?;
                    let name = Name::new(text, token.span);
                    let next = self.peek(0)?;
                    let (value, end) = match next.kind {
                        TokenKind::Comma | TokenKind::Newline | TokenKind::Eof => {
                            (None, name.span.end)
                        }
                        _ => {
                            let value = self.parse_simple()?;
                            let end = value.span().end;
                            (Some(value), end)
                        }
                    };
                    attrs.push(Attr {
                        span: Span::new(name.span.start, end),
                        name,
                        value,
                    });
                    if matches!(self.peek(0)?.kind, TokenKind::Comma) {
                        self.bump()?;
                    } else {
                        break;
                    }
                }
                _ => return Err(self.err("属性名が必要です", token.span)),
            }
        }
        Ok(attrs)
    }

    fn parse_child_block(&mut self) -> Result<Vec<Child>, Diagnostic> {
        match self.lexer.peek_structural()? {
            Some(TokenKind::Indent) => {
                self.lexer.next()?;
            }
            _ => return Ok(Vec::new()),
        }
        let mut children = Vec::new();
        loop {
            match self.lexer.peek_structural()? {
                Some(TokenKind::Dedent) => {
                    self.lexer.next()?;
                    break;
                }
                Some(TokenKind::Eof) => break,
                Some(_) => {
                    let token = self.lexer.next()?;
                    return Err(self.err("子ブロックの構造が不正です", token.span));
                }
                None => children.push(self.parse_child()?),
            }
        }
        Ok(children)
    }

    fn parse_child(&mut self) -> Result<Child, Diagnostic> {
        match self.lexer.classify_line_head()? {
            LineHead::Ident => {
                let token = self.lexer.peek()?;
                if matches!(&token.kind, TokenKind::Ident(name) if name == "h") {
                    Ok(Child::Element(self.parse_element()?))
                } else {
                    self.take_text_child()
                }
            }
            LineHead::UpperIdent => Ok(Child::Component(self.parse_component()?)),
            LineHead::LBrace | LineHead::PipeGt | LineHead::Text => self.take_text_child(),
        }
    }

    fn take_text_child(&mut self) -> Result<Child, Diagnostic> {
        let (line, span) = self.lexer.take_text_line()?;
        let parts = self.build_parts(&line)?;
        Ok(Child::Text { parts, span })
    }

    fn parse_component(&mut self) -> Result<Component, Diagnostic> {
        let name_token = self.bump()?;
        let name = match name_token.kind {
            TokenKind::UpperIdent(text) => Name::new(text, name_token.span),
            _ => {
                return Err(self.err(
                    "コンポーネント名（大文字始まり）が必要です",
                    name_token.span,
                ));
            }
        };
        let mut args = Vec::new();
        if self.starts_arg()? {
            args.push(self.parse_simple()?);
            while matches!(self.peek(0)?.kind, TokenKind::Comma) {
                self.bump()?;
                args.push(self.parse_simple()?);
            }
        }
        let newline = self.peek(0)?;
        if !matches!(newline.kind, TokenKind::Newline) {
            return Err(self
                .err("コンポーネントの行末で予期しないトークンです", newline.span)
                .with_label("大文字で始まるテキストは {'...'} で埋め込みます"));
        }
        self.bump()?;
        let children = self.parse_child_block()?;
        let end = children_end(&children)
            .or_else(|| args.last().map(|a| a.span().end))
            .unwrap_or(name.span.end);
        Ok(Component {
            span: Span::new(name_token.span.start, end),
            name,
            args,
            children,
        })
    }

    fn build_str(&self, lit: &StrLiteral, span: Span) -> Result<Expr, Diagnostic> {
        Ok(Expr::Str {
            parts: self.build_parts(lit)?,
            span,
        })
    }

    fn build_parts(&self, lit: &StrLiteral) -> Result<Vec<StrPart>, Diagnostic> {
        let mut parts = Vec::new();
        for segment in &lit.segments {
            match segment {
                StrSegment::Text(span) => {
                    let raw = &self.source[span.start..span.end];
                    parts.push(StrPart::Text {
                        text: decode_braces(raw),
                        span: *span,
                    });
                }
                StrSegment::Interp(tokens, span) => {
                    let expr = self.parse_interp(tokens, *span)?;
                    parts.push(StrPart::Interp {
                        expr: Box::new(expr),
                        span: *span,
                    });
                }
            }
        }
        Ok(parts)
    }

    fn parse_interp(&self, tokens: &[Token], span: Span) -> Result<Expr, Diagnostic> {
        if tokens.is_empty() {
            return Err(self.err("補間 `{}` の中身が空です", span));
        }
        let mut pos = 0;
        let head = self.parse_interp_simple(tokens, &mut pos, span)?;
        let expr = if pos < tokens.len() {
            let head_start = head.span().start;
            let mut args = vec![self.parse_interp_simple(tokens, &mut pos, span)?];
            while pos < tokens.len() && matches!(tokens[pos].kind, TokenKind::Comma) {
                pos += 1;
                args.push(self.parse_interp_simple(tokens, &mut pos, span)?);
            }
            let end = args.last().unwrap().span().end;
            Expr::App {
                func: Box::new(head),
                args,
                span: Span::new(head_start, end),
            }
        } else {
            head
        };
        if pos != tokens.len() {
            return Err(self.err("補間式に余分なトークンがあります", tokens[pos].span));
        }
        Ok(expr)
    }

    fn parse_interp_simple(
        &self,
        tokens: &[Token],
        pos: &mut usize,
        span: Span,
    ) -> Result<Expr, Diagnostic> {
        let token = tokens
            .get(*pos)
            .ok_or_else(|| self.err("補間式が途中で終わっています", span))?;
        *pos += 1;
        let mut expr = match &token.kind {
            TokenKind::Int(value) => Expr::Int {
                value: *value,
                span: token.span,
            },
            TokenKind::Float(value) => Expr::Float {
                value: *value,
                span: token.span,
            },
            TokenKind::Str(lit) => self.build_str(lit, token.span)?,
            TokenKind::Ident(name) => Expr::Var {
                name: Name::new(name.clone(), token.span),
                span: token.span,
            },
            _ => return Err(self.err("補間式には識別子・リテラルのみ書けます", token.span)),
        };
        while *pos < tokens.len() && matches!(tokens[*pos].kind, TokenKind::Dot) {
            *pos += 1;
            let field_token = tokens
                .get(*pos)
                .ok_or_else(|| self.err("`.` の後にフィールド名が必要です", span))?;
            *pos += 1;
            let field = match &field_token.kind {
                TokenKind::Ident(name) => Name::new(name.clone(), field_token.span),
                _ => {
                    return Err(
                        self.err("`.` の後は小文字のフィールド名が必要です", field_token.span)
                    );
                }
            };
            let field_span = Span::new(expr.span().start, field_token.span.end);
            expr = Expr::Field {
                base: Box::new(expr),
                field,
                span: field_span,
            };
        }
        Ok(expr)
    }
}

fn children_end(children: &[Child]) -> Option<usize> {
    children.last().map(|child| match child {
        Child::Element(element) => element.span.end,
        Child::Component(component) => component.span.end,
        Child::Text { span, .. } => span.end,
    })
}

fn decode_braces(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if bytes[i] == b'{' && i + 1 < raw.len() && bytes[i + 1] == b'{' {
            out.push('{');
            i += 2;
        } else if bytes[i] == b'}' && i + 1 < raw.len() && bytes[i + 1] == b'}' {
            out.push('}');
            i += 2;
        } else {
            let ch = raw[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
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

    fn render_for(src: &str, diagnostic: &Diagnostic) -> String {
        let mut sources = SourceMap::new();
        sources.add_file(PathBuf::from("test.clum"), src.to_string());
        diagnostic.render(&sources)
    }

    fn parse_ok(src: &str) -> Module {
        parse(src, file_id())
            .unwrap_or_else(|e| panic!("パースに失敗しました: {}", render_for(src, &e)))
    }

    fn parse_error(src: &str) -> String {
        let err = parse(src, file_id()).expect_err("エラーを期待しました");
        render_for(src, &err)
    }

    fn only_binding(module: &Module) -> &Binding {
        assert_eq!(module.items.len(), 1);
        match &module.items[0] {
            Item::Binding(binding) => binding,
            other => panic!("Binding を期待しましたが {other:?} でした"),
        }
    }

    fn value_of(binding: &Binding) -> &Expr {
        match &binding.kind {
            BindingKind::Value { value, .. } => value,
            other => panic!("Value 束縛を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn decl_product_type() {
        let module = parse_ok("# User name: String, age: i32\n");
        assert_eq!(module.items.len(), 1);
        match &module.items[0] {
            Item::Decl(decl) => {
                assert_eq!(decl.name.text, "User");
                assert_eq!(decl.params.len(), 2);
                assert_eq!(decl.params[0].name.text, "name");
                assert!(decl.ret.is_none());
            }
            other => panic!("Decl を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn decl_function_signature() {
        let module = parse_ok("# SaveItem api: Api, item: Item -> Void\n");
        match &module.items[0] {
            Item::Decl(decl) => {
                assert_eq!(decl.params.len(), 2);
                match decl.ret.as_ref().unwrap() {
                    TypeExpr::Name { name, .. } => assert_eq!(name.text, "Void"),
                    other => panic!("Name 型を期待しましたが {other:?} でした"),
                }
            }
            other => panic!("Decl を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn decl_generic_type() {
        let module = parse_ok("# Recipe documents: Vec<Document>\n");
        match &module.items[0] {
            Item::Decl(decl) => match &decl.params[0].ty {
                TypeExpr::Generic { name, arg, .. } => {
                    assert_eq!(name.text, "Vec");
                    match arg.as_ref() {
                        TypeExpr::Name { name, .. } => assert_eq!(name.text, "Document"),
                        other => panic!("Name 型を期待しましたが {other:?} でした"),
                    }
                }
                other => panic!("Generic 型を期待しましたが {other:?} でした"),
            },
            other => panic!("Decl を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn import_lists_names() {
        let module = parse_ok("@./index\n  index\n");
        match &module.items[0] {
            Item::Import(import) => {
                assert_eq!(import.path.text, "./index");
                assert_eq!(import.names.len(), 1);
                assert_eq!(import.names[0].text, "index");
            }
            other => panic!("Import を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn value_binding_with_annotation() {
        let module = parse_ok("x: i32 = 42\n");
        let binding = only_binding(&module);
        match &binding.kind {
            BindingKind::Value { name, ty, value } => {
                assert_eq!(name.text, "x");
                assert!(ty.is_some());
                assert!(matches!(value, Expr::Int { value: 42, .. }));
            }
            other => panic!("Value を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn impl_binding_with_params() {
        let module = parse_ok("greet: Greet name -> 'hello'\n");
        let binding = only_binding(&module);
        match &binding.kind {
            BindingKind::Impl {
                name, def, params, ..
            } => {
                assert_eq!(name.text, "greet");
                assert_eq!(def.text, "Greet");
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].text, "name");
            }
            other => panic!("Impl を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn impl_binding_zero_params() {
        let module = parse_ok("thing: Thing -> 'x'\n");
        let binding = only_binding(&module);
        match &binding.kind {
            BindingKind::Impl { params, .. } => assert_eq!(params.len(), 0),
            other => panic!("Impl を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn string_interpolation_with_field() {
        let module = parse_ok("greet: Greet name -> 'hello, {user.name}!'\n");
        let binding = only_binding(&module);
        match &binding.kind {
            BindingKind::Impl { body, .. } => match body {
                Expr::Str { parts, .. } => {
                    assert_eq!(parts.len(), 3);
                    match &parts[1] {
                        StrPart::Interp { expr, .. } => {
                            assert!(matches!(expr.as_ref(), Expr::Field { .. }));
                        }
                        other => panic!("Interp を期待しましたが {other:?} でした"),
                    }
                }
                other => panic!("Str を期待しましたが {other:?} でした"),
            },
            other => panic!("Impl を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn string_double_brace_is_decoded() {
        let module = parse_ok("x = 'a {{b}} c'\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Str { parts, .. } => match &parts[0] {
                StrPart::Text { text, .. } => assert_eq!(text, "a {b} c"),
                other => panic!("Text を期待しましたが {other:?} でした"),
            },
            other => panic!("Str を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn field_access_chain() {
        let module = parse_ok("x = e.current-target.value\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Field { base, field, .. } => {
                assert_eq!(field.text, "value");
                assert!(matches!(base.as_ref(), Expr::Field { .. }));
            }
            other => panic!("Field を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn element_with_attributes() {
        let module = parse_ok("x: Html = h .a href '/about', class 'nav'\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Element(element) => {
                assert_eq!(element.tag.text, "a");
                assert_eq!(element.attrs.len(), 2);
                assert_eq!(element.attrs[0].name.text, "href");
                assert!(element.attrs[0].value.is_some());
            }
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn element_boolean_attribute() {
        let module = parse_ok("x: Html = h .input type 'text', disabled\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Element(element) => {
                assert_eq!(element.attrs.len(), 2);
                assert!(element.attrs[1].value.is_none());
            }
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn element_with_text_child() {
        let module = parse_ok("x: Html = h .div\n  こんにちは\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Element(element) => {
                assert_eq!(element.children.len(), 1);
                match &element.children[0] {
                    Child::Text { parts, .. } => match &parts[0] {
                        StrPart::Text { text, .. } => assert_eq!(text, "こんにちは"),
                        other => panic!("Text を期待しましたが {other:?} でした"),
                    },
                    other => panic!("Text 子を期待しましたが {other:?} でした"),
                }
            }
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn text_children_are_concatenated_as_separate_lines() {
        let module = parse_ok("x: Html = h .div\n  aa\n  bb\n  cc\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Element(element) => assert_eq!(element.children.len(), 3),
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn nested_elements() {
        let src = "x: Html = h .ul\n  h .li\n    one\n  h .li\n    two\n";
        let module = parse_ok(src);
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Element(element) => {
                assert_eq!(element.tag.text, "ul");
                assert_eq!(element.children.len(), 2);
                for child in &element.children {
                    match child {
                        Child::Element(li) => {
                            assert_eq!(li.tag.text, "li");
                            assert_eq!(li.children.len(), 1);
                        }
                        other => panic!("li 要素を期待しましたが {other:?} でした"),
                    }
                }
            }
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn component_call_with_child_block() {
        let src = "page: Html = h .body\n  Card 'お知らせ'\n    h .p\n      本日は晴天です\n";
        let module = parse_ok(src);
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Element(body) => match &body.children[0] {
                Child::Component(component) => {
                    assert_eq!(component.name.text, "Card");
                    assert_eq!(component.args.len(), 1);
                    assert_eq!(component.children.len(), 1);
                }
                other => panic!("Component を期待しましたが {other:?} でした"),
            },
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn interp_only_line_is_text_child() {
        let module = parse_ok("x: Html = h .ul\n  {items}\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Element(element) => match &element.children[0] {
                Child::Text { parts, .. } => {
                    assert_eq!(parts.len(), 1);
                    assert!(matches!(&parts[0], StrPart::Interp { .. }));
                }
                other => panic!("Text 子を期待しましたが {other:?} でした"),
            },
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn text_child_mixing_interp_and_japanese() {
        let module = parse_ok("x: Html = h .li\n  {page.title} さん\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Element(element) => match &element.children[0] {
                Child::Text { parts, .. } => {
                    assert_eq!(parts.len(), 2);
                    assert!(matches!(&parts[0], StrPart::Interp { .. }));
                    assert!(matches!(&parts[1], StrPart::Text { .. }));
                }
                other => panic!("Text 子を期待しましたが {other:?} でした"),
            },
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn text_child_with_string_in_interp() {
        let module = parse_ok("x: Html = h .p\n  {'h で始まる文'}\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Element(element) => match &element.children[0] {
                Child::Text { parts, .. } => match &parts[0] {
                    StrPart::Interp { expr, .. } => {
                        assert!(matches!(expr.as_ref(), Expr::Str { .. }));
                    }
                    other => panic!("Interp を期待しましたが {other:?} でした"),
                },
                other => panic!("Text 子を期待しましたが {other:?} でした"),
            },
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn pipe_head_line_is_text_child() {
        let module = parse_ok("x: Html = h .div\n  |> これは本文\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Element(element) => match &element.children[0] {
                Child::Text { parts, .. } => match &parts[0] {
                    StrPart::Text { text, .. } => assert_eq!(text, "|> これは本文"),
                    other => panic!("Text を期待しましたが {other:?} でした"),
                },
                other => panic!("Text 子を期待しましたが {other:?} でした"),
            },
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn lowercase_ident_line_is_text_child() {
        let module = parse_ok("x: Html = h .div\n  pages\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Element(element) => match &element.children[0] {
                Child::Text { parts, .. } => match &parts[0] {
                    StrPart::Text { text, .. } => assert_eq!(text, "pages"),
                    other => panic!("Text を期待しましたが {other:?} でした"),
                },
                other => panic!("Text 子を期待しましたが {other:?} でした"),
            },
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn list_marker_line_is_text_child() {
        let module = parse_ok("x: Html = h .div\n  - これは本文\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Element(element) => match &element.children[0] {
                Child::Text { parts, .. } => match &parts[0] {
                    StrPart::Text { text, .. } => assert_eq!(text, "- これは本文"),
                    other => panic!("Text を期待しましたが {other:?} でした"),
                },
                other => panic!("Text 子を期待しましたが {other:?} でした"),
            },
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn vec_literal_scalar_elements() {
        let module = parse_ok("xs =\n  - 1\n  - 2\n  - 3\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Vec { elems, .. } => {
                assert_eq!(elems.len(), 3);
                assert!(matches!(
                    &elems[0],
                    VecElem::Expr(Expr::Int { value: 1, .. })
                ));
            }
            other => panic!("Vec を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn vec_literal_record_elements() {
        let src = "docs =\n  - path: './a'\n    element: a\n  - path: './b'\n    element: b\n";
        let module = parse_ok(src);
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Vec { elems, .. } => {
                assert_eq!(elems.len(), 2);
                match &elems[0] {
                    VecElem::Record { fields, .. } => {
                        assert_eq!(fields.len(), 2);
                        assert_eq!(fields[0].name.text, "path");
                        assert_eq!(fields[1].name.text, "element");
                    }
                    other => panic!("Record 要素を期待しましたが {other:?} でした"),
                }
            }
            other => panic!("Vec を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn lambda_inline_body_absorbs_commas() {
        let module = parse_ok("result = map page -> f page, xs\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::App { func, args, .. } => {
                assert!(matches!(func.as_ref(), Expr::Var { .. }));
                assert_eq!(args.len(), 1);
                match &args[0] {
                    Expr::Lambda { body, .. } => match body.as_ref() {
                        Expr::App {
                            args: inner_args, ..
                        } => assert_eq!(inner_args.len(), 2),
                        other => panic!("App 本体を期待しましたが {other:?} でした"),
                    },
                    other => panic!("Lambda 引数を期待しましたが {other:?} でした"),
                }
            }
            other => panic!("App を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn lambda_block_body_with_local_binding() {
        let src = "f = make x ->\n  y = x\n  y\n";
        let module = parse_ok(src);
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::App { args, .. } => match &args[0] {
                Expr::Lambda { body, .. } => match body.as_ref() {
                    Expr::Block { bindings, .. } => assert_eq!(bindings.len(), 1),
                    other => panic!("Block 本体を期待しましたが {other:?} でした"),
                },
                other => panic!("Lambda を期待しましたが {other:?} でした"),
            },
            other => panic!("App を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn multiline_pipe_continuation() {
        let src = "items = pages\n  |> map page ->\n    h .li\n      {page.title}\n";
        let module = parse_ok(src);
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Pipe { lhs, rhs, .. } => {
                assert!(matches!(lhs.as_ref(), Expr::Var { .. }));
                assert!(matches!(rhs.as_ref(), Expr::App { .. }));
            }
            other => panic!("Pipe を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn inline_pipe_chain() {
        let module = parse_ok("x = a |> f b |> g c\n");
        let binding = only_binding(&module);
        match value_of(binding) {
            Expr::Pipe { lhs, .. } => assert!(matches!(lhs.as_ref(), Expr::Pipe { .. })),
            other => panic!("Pipe を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn pub_binding_sets_flag() {
        let module = parse_ok(":pub\nindex: Html = h .div\n");
        let binding = only_binding(&module);
        assert!(binding.is_pub);
    }

    #[test]
    fn record_construction_with_pipe_and_bang() {
        let src = "@./index\n  index\n\nRecipe\n  documents:\n    - path: './dist/index.html'\n      element: index\n  |> build\n  |> !\n";
        let module = parse_ok(src);
        assert_eq!(module.items.len(), 2);
        match &module.items[1] {
            Item::Expr(Expr::Pipe { lhs, rhs, .. }) => {
                assert!(matches!(rhs.as_ref(), Expr::Bang { .. }));
                match lhs.as_ref() {
                    Expr::Pipe { lhs, rhs, .. } => {
                        assert!(matches!(lhs.as_ref(), Expr::Record { .. }));
                        assert!(matches!(rhs.as_ref(), Expr::App { .. } | Expr::Var { .. }));
                    }
                    other => panic!("内側の Pipe を期待しましたが {other:?} でした"),
                }
            }
            other => panic!("Pipe 式文を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn full_page_source_parses() {
        let src = concat!(
            "items = pages\n",
            "  |> map page ->\n",
            "    h .li\n",
            "      h .a href page.path\n",
            "        {page.title}\n",
            "\n",
            ":pub\n",
            "index: Html = h .html\n",
            "  h .head\n",
            "    h .meta charset 'utf-8'\n",
            "    h .title\n",
            "      clum\n",
            "  h .body\n",
            "    h .ul\n",
            "      {items}\n",
        );
        let module = parse_ok(src);
        assert_eq!(module.items.len(), 2);
        match &module.items[1] {
            Item::Binding(binding) => {
                assert!(binding.is_pub);
                match value_of(binding) {
                    Expr::Element(html) => {
                        assert_eq!(html.tag.text, "html");
                        assert_eq!(html.children.len(), 2);
                    }
                    other => panic!("Element を期待しましたが {other:?} でした"),
                }
            }
            other => panic!("Binding を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn err_element_without_tag() {
        let message = parse_error("x: Html = h .div\n  h\n    child\n");
        assert!(message.contains("タグ"));
        assert!(message.contains(":2:"));
    }

    #[test]
    fn err_binding_without_eq_or_colon() {
        let message = parse_error("foo bar\n");
        assert!(message.contains("`=` か `:`"));
    }

    #[test]
    fn err_record_inline_form() {
        let message = parse_error("x: Html = h .div\n  Card 'x' extra 'y'\n");
        assert!(message.contains("コンポーネントの行末"));
    }

    #[test]
    fn err_record_construction_inline() {
        let message = parse_error("Recipe documents\n");
        assert!(message.contains("1行形式はありません"));
    }

    #[test]
    fn err_bang_outside_pipe() {
        let message = parse_error("x = !\n");
        assert!(message.contains("`!`"));
        assert!(message.contains("パイプ"));
    }

    #[test]
    fn err_import_without_names() {
        let message = parse_error("@./index\nx = 1\n");
        assert!(message.contains("インデントして列挙"));
    }

    #[test]
    fn err_pub_followed_by_non_binding() {
        let message = parse_error(":pub\n# Foo x: i32\n");
        assert!(message.contains("`:pub` の直後には束縛"));
    }

    #[test]
    fn err_pub_value_without_annotation() {
        let message = parse_error(":pub\nindex = h .div\n");
        assert!(message.contains("型注釈が必要"));
    }

    #[test]
    fn err_list_element_multiline() {
        let message = parse_error("xs =\n  - a\n    deeper\n");
        assert!(message.contains("複数行の式は書けません"));
    }

    #[test]
    fn err_interp_extra_tokens() {
        let message = parse_error("x = 'oops {a b c d}'\n");
        assert!(message.contains("余分なトークン") || message.contains("補間"));
    }

    #[test]
    fn err_dot_followed_by_uppercase() {
        let message = parse_error("x = page.Title\n");
        assert!(message.contains("フィールド名"));
    }

    #[test]
    fn err_text_lone_closing_brace() {
        let message = parse_error("x: Html = h .div\n  a}b\n");
        assert!(message.contains("`}}`"));
    }

    #[test]
    fn err_expression_followed_by_comma() {
        let message = parse_error("x = a, b\n");
        assert!(message.contains("式の行末で予期しないトークンです"));
        assert!(message.contains(":1:6"));
    }

    #[test]
    fn err_component_trailing_comma_needs_arg() {
        let message = parse_error("x: Html = h .div\n  Card 'a',\n");
        assert!(message.contains("式が必要"));
    }
}
