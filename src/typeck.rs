use std::collections::HashMap;

use crate::ast::{
    Attr, Binding, BindingKind, Child, Component, Element, Expr, Item, Module, Name, RecordField,
    StrPart, TypeExpr, VecElem,
};
use crate::diag::Diagnostic;
use crate::prelude::{self, AttrKind};
use crate::resolve::{Program, ResolvedModule};
use crate::source::FileId;
use crate::span::Span;
use crate::ty::Ty;

pub fn check_program(program: &Program) -> Result<Vec<Diagnostic>, Diagnostic> {
    let mut warnings = Vec::new();
    let mut export_tys: HashMap<FileId, Ty> = HashMap::new();
    for module in &program.modules {
        let mut checker = Checker::new(module.file, &export_tys);
        let export = checker.run(module)?;
        warnings.append(&mut checker.warnings);
        if let Some(ty) = export {
            export_tys.insert(module.file, ty);
        }
    }
    Ok(warnings)
}

#[derive(Debug, Clone)]
enum DeclInfo {
    Record(Vec<(String, Ty)>),
    Func(Vec<Ty>, Ty),
}

struct Checker<'a> {
    file: FileId,
    export_tys: &'a HashMap<FileId, Ty>,
    decl_kinds: HashMap<String, bool>,
    decls: HashMap<String, DeclInfo>,
    scopes: Vec<HashMap<String, Ty>>,
    export_ty: Option<Ty>,
    warnings: Vec<Diagnostic>,
}

impl<'a> Checker<'a> {
    fn new(file: FileId, export_tys: &'a HashMap<FileId, Ty>) -> Self {
        Self {
            file,
            export_tys,
            decl_kinds: HashMap::new(),
            decls: HashMap::new(),
            scopes: vec![HashMap::new()],
            export_ty: None,
            warnings: Vec::new(),
        }
    }

    fn run(&mut self, module: &ResolvedModule) -> Result<Option<Ty>, Diagnostic> {
        self.build_decls(&module.module)?;
        self.seed_globals(module)?;
        self.check_items(&module.module)?;
        Ok(self.export_ty.clone())
    }

    fn build_decls(&mut self, module: &Module) -> Result<(), Diagnostic> {
        self.decl_kinds.insert("Recipe".to_string(), true);
        self.decl_kinds.insert("Document".to_string(), true);
        for item in &module.items {
            if let Item::Decl(decl) = item {
                self.decl_kinds
                    .insert(decl.name.text.clone(), decl.ret.is_none());
            }
        }
        self.decls.insert(
            "Recipe".to_string(),
            DeclInfo::Record(vec![(
                "documents".to_string(),
                Ty::Vec(Box::new(Ty::Record("Document".to_string()))),
            )]),
        );
        self.decls.insert(
            "Document".to_string(),
            DeclInfo::Record(vec![
                ("path".to_string(), Ty::Str),
                ("element".to_string(), Ty::Html),
            ]),
        );
        for item in &module.items {
            if let Item::Decl(decl) = item {
                let info = match &decl.ret {
                    Some(ret) => {
                        let mut params = Vec::new();
                        for field in &decl.params {
                            params.push(self.ty_of(&field.ty)?);
                        }
                        DeclInfo::Func(params, self.ty_of(ret)?)
                    }
                    None => {
                        let mut fields = Vec::new();
                        for field in &decl.params {
                            fields.push((field.name.text.clone(), self.ty_of(&field.ty)?));
                        }
                        DeclInfo::Record(fields)
                    }
                };
                self.decls.insert(decl.name.text.clone(), info);
            }
        }
        Ok(())
    }

    fn seed_globals(&mut self, module: &ResolvedModule) -> Result<(), Diagnostic> {
        self.define(
            "build",
            Ty::Fn(
                vec![Ty::Record("Recipe".to_string())],
                Box::new(Ty::Eff(Box::new(Ty::Void))),
            ),
        );
        self.define("true", Ty::Bool);
        self.define("false", Ty::Bool);
        for import in &module.imports {
            for name in &import.names {
                match self.export_tys.get(&import.file) {
                    Some(ty) => self.define(&name.text, ty.clone()),
                    None => {
                        return Err(self.error(
                            format!("import した `{}` の型が求まっていません", name.text),
                            name.span,
                        ));
                    }
                }
            }
        }
        for item in &module.module.items {
            if let Item::Binding(binding) = item {
                match &binding.kind {
                    BindingKind::Impl { name, def, .. } => {
                        if let Some(DeclInfo::Func(params, ret)) = self.decls.get(&def.text) {
                            let ty = Ty::Fn(params.clone(), Box::new(ret.clone()));
                            self.define(&name.text, ty.clone());
                            if binding.is_pub {
                                self.export_ty = Some(ty);
                            }
                        }
                    }
                    BindingKind::Value {
                        name, ty: Some(te), ..
                    } => {
                        let ty = self.ty_of(te)?;
                        self.define(&name.text, ty.clone());
                        if binding.is_pub {
                            self.export_ty = Some(ty);
                        }
                    }
                    BindingKind::Value { ty: None, .. } => {}
                }
            }
        }
        Ok(())
    }

    fn check_items(&mut self, module: &Module) -> Result<(), Diagnostic> {
        for item in &module.items {
            match item {
                Item::Decl(_) | Item::Import(_) => {}
                Item::Binding(binding) => match &binding.kind {
                    BindingKind::Impl {
                        def, params, body, ..
                    } => self.check_impl(def, params, body)?,
                    BindingKind::Value {
                        ty: Some(te),
                        value,
                        ..
                    } => {
                        let ty = self.ty_of(te)?;
                        self.check_expr(value, &ty)?;
                    }
                    BindingKind::Value {
                        name,
                        ty: None,
                        value,
                    } => {
                        let ty = self.infer_expr(value)?;
                        self.define(&name.text, ty);
                    }
                },
                Item::Expr(expr) => {
                    self.infer_expr(expr)?;
                }
            }
        }
        Ok(())
    }

    fn check_impl(&mut self, def: &Name, params: &[Name], body: &Expr) -> Result<(), Diagnostic> {
        let (param_tys, ret) = match self.decls.get(&def.text) {
            Some(DeclInfo::Func(param_tys, ret)) => (param_tys.clone(), ret.clone()),
            Some(DeclInfo::Record(_)) => {
                return Err(self.error(
                    format!("レコード型 `{}` には実装を書けません", def.text),
                    def.span,
                ));
            }
            None => {
                return Err(self.error(format!("定義 `{}` が見つかりません", def.text), def.span));
            }
        };
        if params.len() != param_tys.len() {
            return Err(self.error(
                format!(
                    "`{}` の実装は引数を{}個取りますが、{}個書かれています",
                    def.text,
                    param_tys.len(),
                    params.len()
                ),
                def.span,
            ));
        }
        self.push_scope();
        for (param, ty) in params.iter().zip(&param_tys) {
            self.define(&param.text, ty.clone());
        }
        let result = self.check_expr(body, &ret);
        self.pop_scope();
        result
    }

    fn ty_of(&self, te: &TypeExpr) -> Result<Ty, Diagnostic> {
        match te {
            TypeExpr::Name { name, span } => self.named_ty(&name.text, *span),
            TypeExpr::Generic { name, arg, span } => {
                let inner = self.ty_of(arg)?;
                match name.text.as_str() {
                    "Vec" => Ok(Ty::Vec(Box::new(inner))),
                    "Eff" => Ok(Ty::Eff(Box::new(inner))),
                    other => Err(self.error(
                        format!("`{other}` は型引数を取るジェネリック型ではありません"),
                        *span,
                    )),
                }
            }
        }
    }

    fn named_ty(&self, name: &str, span: Span) -> Result<Ty, Diagnostic> {
        let ty = match name {
            "i32" => Ty::I32,
            "i64" => Ty::I64,
            "f32" => Ty::F32,
            "f64" => Ty::F64,
            "String" => Ty::Str,
            "Bool" => Ty::Bool,
            "Void" => Ty::Void,
            "Html" => Ty::Html,
            "Tag" => Ty::Tag,
            "Recipe" => Ty::Record("Recipe".to_string()),
            "Document" => Ty::Record("Document".to_string()),
            "Vec" | "Eff" => {
                return Err(self.error(
                    format!("`{name}` には型引数が必要です（例: `{name}<T>`）"),
                    span,
                ));
            }
            other => match self.decl_kinds.get(other) {
                Some(true) => Ty::Record(other.to_string()),
                Some(false) => {
                    return Err(self.error(
                        format!("関数シグネチャ `{other}` は型注釈には使えません"),
                        span,
                    ));
                }
                None => {
                    return Err(self.error(format!("型 `{other}` は定義されていません"), span));
                }
            },
        };
        Ok(ty)
    }

    fn infer_expr(&mut self, expr: &Expr) -> Result<Ty, Diagnostic> {
        match expr {
            Expr::Int { .. } => Ok(Ty::I32),
            Expr::Float { .. } => Ok(Ty::F64),
            Expr::Str { parts, .. } => {
                self.check_interp_parts(parts)?;
                Ok(Ty::Str)
            }
            Expr::Var { name, span } => {
                if name.text == "map" {
                    return Err(
                        self.error("`map` は関数適用の位置でのみ使えます".to_string(), *span)
                    );
                }
                match self.lookup(&name.text) {
                    Some(ty) => Ok(ty),
                    None => {
                        Err(self.error(format!("`{}` の型が求まっていません", name.text), *span))
                    }
                }
            }
            Expr::Field { base, field, span } => self.infer_field(base, field, *span),
            Expr::App { func, args, span } => self.infer_app(func, args, *span),
            Expr::Lambda { span, .. } => Err(self.error(
                "ラムダの型を推論できません（`map` の引数としてのみ使えます）".to_string(),
                *span,
            )),
            Expr::Pipe { lhs, rhs, .. } => self.infer_pipe(lhs, rhs),
            Expr::Bang { span } => {
                Err(self.error("`!` はパイプ `|>` の右側でのみ使えます".to_string(), *span))
            }
            Expr::Record { name, fields, .. } => self.infer_record(name, fields),
            Expr::Vec { elems, span } => self.infer_vec(elems, None, *span),
            Expr::Block {
                bindings, result, ..
            } => self.infer_block(bindings, result),
            Expr::Element(element) => self.infer_element(element),
        }
    }

    fn check_expr(&mut self, expr: &Expr, want: &Ty) -> Result<(), Diagnostic> {
        match expr {
            Expr::Vec { elems, span } => {
                let want_elem = match want {
                    Ty::Vec(inner) => Some(inner.as_ref().clone()),
                    _ => None,
                };
                let got = self.infer_vec(elems, want_elem.as_ref(), *span)?;
                self.expect(&got, want, *span)
            }
            Expr::Lambda { param, body, span } => match want {
                Ty::Fn(params, ret) if params.len() == 1 => {
                    self.push_scope();
                    self.define(&param.text, params[0].clone());
                    let result = self.check_expr(body, ret);
                    self.pop_scope();
                    result
                }
                _ => Err(self.error(
                    format!("ここではラムダを書けません（`{want}` 型が必要です）"),
                    *span,
                )),
            },
            _ => {
                let got = self.infer_expr(expr)?;
                self.expect(&got, want, expr.span())
            }
        }
    }

    fn infer_field(&mut self, base: &Expr, field: &Name, span: Span) -> Result<Ty, Diagnostic> {
        let base_ty = self.infer_expr(base)?;
        match &base_ty {
            Ty::Record(name) => match self.decls.get(name) {
                Some(DeclInfo::Record(fields)) => {
                    match fields.iter().find(|(fname, _)| fname == &field.text) {
                        Some((_, ty)) => Ok(ty.clone()),
                        None => Err(self.error(
                            format!(
                                "レコード `{name}` にフィールド `{}` はありません",
                                field.text
                            ),
                            field.span,
                        )),
                    }
                }
                _ => Err(self.error(
                    format!("`{name}` はレコードではないためフィールドを持ちません"),
                    span,
                )),
            },
            other => Err(self.error(
                format!(
                    "`.{}` でアクセスしていますが、`{other}` 型はレコードではありません",
                    field.text
                ),
                base.span(),
            )),
        }
    }

    fn infer_app(&mut self, func: &Expr, args: &[Expr], span: Span) -> Result<Ty, Diagnostic> {
        if is_map(func) {
            if args.len() != 2 {
                return Err(self.error("`map` は関数と Vec の2引数が必要です".to_string(), span));
            }
            let vec_ty = self.infer_expr(&args[1])?;
            return self.infer_map(&args[0], &vec_ty, span);
        }
        let func_ty = self.infer_expr(func)?;
        let mut arg_tys = Vec::new();
        for arg in args {
            arg_tys.push((self.infer_expr(arg)?, arg.span()));
        }
        self.apply(func_ty, arg_tys)
    }

    fn infer_pipe(&mut self, lhs: &Expr, rhs: &Expr) -> Result<Ty, Diagnostic> {
        match rhs {
            Expr::Bang { span } => {
                let lhs_ty = self.infer_expr(lhs)?;
                match lhs_ty {
                    Ty::Eff(inner) => Ok(*inner),
                    other => Err(self.error(
                        format!("`!` は `Eff<...>` にのみ使えますが、`{other}` 型が来ました"),
                        *span,
                    )),
                }
            }
            Expr::App { func, args, span } if is_map(func) => {
                let lhs_ty = self.infer_expr(lhs)?;
                if args.len() != 1 {
                    return Err(self.error(
                        "パイプで `map` に渡すときは関数を1つ書きます".to_string(),
                        *span,
                    ));
                }
                self.infer_map(&args[0], &lhs_ty, *span)
            }
            Expr::App { func, args, .. } => {
                let lhs_ty = self.infer_expr(lhs)?;
                let func_ty = self.infer_expr(func)?;
                let mut arg_tys = Vec::new();
                for arg in args {
                    arg_tys.push((self.infer_expr(arg)?, arg.span()));
                }
                arg_tys.push((lhs_ty, lhs.span()));
                self.apply(func_ty, arg_tys)
            }
            _ => {
                if is_map(rhs) {
                    return Err(self.error(
                        "`map` には関数を渡してください（例: `xs |> map f`）".to_string(),
                        rhs.span(),
                    ));
                }
                let lhs_ty = self.infer_expr(lhs)?;
                let func_ty = self.infer_expr(rhs)?;
                self.apply(func_ty, vec![(lhs_ty, lhs.span())])
            }
        }
    }

    fn infer_map(&mut self, f: &Expr, vec_ty: &Ty, span: Span) -> Result<Ty, Diagnostic> {
        let elem = match vec_ty {
            Ty::Vec(inner) => inner.as_ref().clone(),
            other => {
                return Err(self.error(
                    format!("`map` の対象は `Vec<T>` ですが、`{other}` 型が来ました"),
                    span,
                ));
            }
        };
        match f {
            Expr::Lambda { param, body, .. } => {
                self.push_scope();
                self.define(&param.text, elem);
                let body_ty = self.infer_expr(body);
                self.pop_scope();
                Ok(Ty::Vec(Box::new(body_ty?)))
            }
            _ => {
                let func_ty = self.infer_expr(f)?;
                match func_ty {
                    Ty::Fn(params, ret) if params.len() == 1 => {
                        self.expect(&elem, &params[0], f.span())?;
                        Ok(Ty::Vec(ret))
                    }
                    other => Err(self.error(
                        format!("`map` の第1引数は1引数の関数ですが、`{other}` 型が来ました"),
                        f.span(),
                    )),
                }
            }
        }
    }

    fn apply(&self, func_ty: Ty, args: Vec<(Ty, Span)>) -> Result<Ty, Diagnostic> {
        let mut current = func_ty;
        let mut index = 0;
        loop {
            match current {
                Ty::Fn(params, ret) => {
                    let available = args.len() - index;
                    if available < params.len() {
                        for (offset, param) in params.iter().take(available).enumerate() {
                            self.expect_arg(&args[index + offset], param)?;
                        }
                        let rest = params[available..].to_vec();
                        return Ok(Ty::Fn(rest, ret));
                    }
                    for (offset, param) in params.iter().enumerate() {
                        self.expect_arg(&args[index + offset], param)?;
                    }
                    index += params.len();
                    current = *ret;
                    if index == args.len() {
                        return Ok(current);
                    }
                }
                other => {
                    if index == args.len() {
                        return Ok(other);
                    }
                    return Err(self.error(
                        format!("`{other}` 型は関数ではないため、これ以上引数を適用できません"),
                        args[index].1,
                    ));
                }
            }
        }
    }

    fn expect_arg(&self, got: &(Ty, Span), want: &Ty) -> Result<(), Diagnostic> {
        if &got.0 == want {
            return Ok(());
        }
        Err(self.error(
            format!(
                "引数の型が一致しません: `{want}` 型を期待しましたが、`{}` 型が来ました",
                got.0
            ),
            got.1,
        ))
    }

    fn infer_record(&mut self, name: &Name, fields: &[RecordField]) -> Result<Ty, Diagnostic> {
        match self.decls.get(&name.text) {
            Some(DeclInfo::Record(_)) => {}
            Some(DeclInfo::Func(_, _)) => {
                return Err(self.error(
                    format!(
                        "`{}` は関数シグネチャでレコードとして構築できません",
                        name.text
                    ),
                    name.span,
                ));
            }
            None => {
                return Err(self.error(
                    format!("型 `{}` は定義されていません", name.text),
                    name.span,
                ));
            }
        }
        self.check_record_fields(&name.text, name.span, fields)?;
        Ok(Ty::Record(name.text.clone()))
    }

    fn check_record_fields(
        &mut self,
        type_name: &str,
        at_span: Span,
        fields: &[RecordField],
    ) -> Result<(), Diagnostic> {
        let decl_fields = match self.decls.get(type_name) {
            Some(DeclInfo::Record(decl_fields)) => decl_fields.clone(),
            _ => {
                return Err(
                    self.error(format!("`{type_name}` はレコード型ではありません"), at_span)
                );
            }
        };
        if fields.len() != decl_fields.len() {
            return Err(self.error(
                format!(
                    "`{type_name}` はフィールドを{}個持ちますが、{}個書かれています",
                    decl_fields.len(),
                    fields.len()
                ),
                at_span,
            ));
        }
        for (index, (field, (decl_name, decl_ty))) in fields.iter().zip(&decl_fields).enumerate() {
            if field.name.text != *decl_name {
                return Err(self.error(
                    format!(
                        "`{type_name}` の{}番目のフィールドは `{decl_name}` ですが、`{}` が書かれています（順序も宣言と揃えてください）",
                        index + 1,
                        field.name.text
                    ),
                    field.name.span,
                ));
            }
            self.check_expr(&field.value, decl_ty)?;
        }
        Ok(())
    }

    fn infer_vec(
        &mut self,
        elems: &[VecElem],
        want_elem: Option<&Ty>,
        span: Span,
    ) -> Result<Ty, Diagnostic> {
        let elem_ty = match want_elem {
            Some(ty) => ty.clone(),
            None => {
                if elems.is_empty() {
                    return Err(self.error(
                        "空のリストの要素型を決められません。型注釈を付けてください".to_string(),
                        span,
                    ));
                }
                match &elems[0] {
                    VecElem::Expr(expr) => self.infer_expr(expr)?,
                    VecElem::Record { span: rspan, .. } => {
                        return Err(self.error(
                            "リスト要素の型を決められません。束縛に型注釈（例: `Vec<T>`）を付けてください".to_string(),
                            *rspan,
                        ));
                    }
                }
            }
        };
        for elem in elems {
            match elem {
                VecElem::Expr(expr) => self.check_expr(expr, &elem_ty)?,
                VecElem::Record {
                    fields,
                    span: rspan,
                } => match &elem_ty {
                    Ty::Record(rname) => {
                        self.check_record_fields(&rname.clone(), *rspan, fields)?;
                    }
                    other => {
                        return Err(self.error(
                            format!("リスト要素はレコードですが、要素型は `{other}` です"),
                            *rspan,
                        ));
                    }
                },
            }
        }
        Ok(Ty::Vec(Box::new(elem_ty)))
    }

    fn infer_block(&mut self, bindings: &[Binding], result: &Expr) -> Result<Ty, Diagnostic> {
        self.push_scope();
        let outcome = self.infer_block_inner(bindings, result);
        self.pop_scope();
        outcome
    }

    fn infer_block_inner(&mut self, bindings: &[Binding], result: &Expr) -> Result<Ty, Diagnostic> {
        for binding in bindings {
            match &binding.kind {
                BindingKind::Value {
                    name,
                    ty: Some(te),
                    value,
                } => {
                    let ty = self.ty_of(te)?;
                    self.check_expr(value, &ty)?;
                    self.define(&name.text, ty);
                }
                BindingKind::Value {
                    name,
                    ty: None,
                    value,
                } => {
                    let ty = self.infer_expr(value)?;
                    self.define(&name.text, ty);
                }
                BindingKind::Impl {
                    name,
                    def,
                    params,
                    body,
                } => {
                    self.check_impl(def, params, body)?;
                    if let Some(DeclInfo::Func(param_tys, ret)) = self.decls.get(&def.text) {
                        let ty = Ty::Fn(param_tys.clone(), Box::new(ret.clone()));
                        self.define(&name.text, ty);
                    }
                }
            }
        }
        self.infer_expr(result)
    }

    fn infer_element(&mut self, element: &Element) -> Result<Ty, Diagnostic> {
        let tag = &element.tag.text;
        if !prelude::is_tag(tag) {
            let mut diagnostic =
                self.error(format!("`Tag` に `.{tag}` はありません"), element.tag.span);
            if let Some(suggestion) = prelude::suggest_tag(tag) {
                diagnostic = diagnostic.with_label(format!("もしかして `.{suggestion}` ですか？"));
            }
            return Err(diagnostic);
        }
        if prelude::is_void(tag)
            && let Some(child) = element.children.first()
        {
            return Err(self
                .error(
                    format!("void 要素 `.{tag}` は子を持てません"),
                    child_span(child),
                )
                .with_label("void 要素の子ブロックは書けません"));
        }
        for attr in &element.attrs {
            self.check_attr(tag, attr)?;
        }
        if tag == "img" && !element.attrs.iter().any(|attr| attr.name.text == "alt") {
            self.warnings.push(
                Diagnostic::warning("img 要素に alt 属性がありません".to_string())
                    .at(self.file, element.tag.span)
                    .with_label("アクセシビリティのため alt を付けてください"),
            );
        }
        for child in &element.children {
            self.check_child(child)?;
        }
        Ok(Ty::Html)
    }

    fn check_attr(&mut self, tag: &str, attr: &Attr) -> Result<(), Diagnostic> {
        let name = &attr.name.text;
        if name.starts_with("on-") {
            return Err(self
                .error(
                    format!("`on-*` 属性 `{name}` は html ターゲットでは使えません"),
                    attr.name.span,
                )
                .with_label("イベントは js ターゲットの責務です"));
        }
        if name.starts_with("data-") || name.starts_with("aria-") {
            return self.check_attr_value(name, attr);
        }
        let kind = prelude::element_attr(tag, name).or_else(|| prelude::global_attr(name));
        match kind {
            Some(AttrKind::Bool) => match &attr.value {
                Some(value) => Err(self.error(
                    format!("真偽属性 `{name}` に値は指定できません"),
                    value.span(),
                )),
                None => Ok(()),
            },
            Some(AttrKind::Value) => self.check_attr_value(name, attr),
            None => Err(self
                .error(
                    format!("`.{tag}` 要素に属性 `{name}` はありません"),
                    attr.name.span,
                )
                .with_label("data-* / aria-* は任意に使えます")),
        }
    }

    fn check_attr_value(&mut self, name: &str, attr: &Attr) -> Result<(), Diagnostic> {
        match &attr.value {
            None => Err(self.error(format!("属性 `{name}` には値が必要です"), attr.name.span)),
            Some(value) => {
                let ty = self.infer_expr(value)?;
                if ty == Ty::Str {
                    Ok(())
                } else {
                    Err(self.error(
                        format!("属性値は String 型である必要がありますが、`{ty}` 型が来ました"),
                        value.span(),
                    ))
                }
            }
        }
    }

    fn check_child(&mut self, child: &Child) -> Result<(), Diagnostic> {
        match child {
            Child::Element(element) => {
                self.infer_element(element)?;
                Ok(())
            }
            Child::Component(component) => self.check_component(component),
            Child::Text { parts, .. } => self.check_text_child(parts),
        }
    }

    fn check_text_child(&mut self, parts: &[StrPart]) -> Result<(), Diagnostic> {
        if parts.len() == 1
            && let StrPart::Interp { expr, .. } = &parts[0]
        {
            let ty = self.infer_expr(expr)?;
            if ty == Ty::Str || ty.is_html() || ty.is_vec_of_html() {
                return Ok(());
            }
            return Err(self.error(
                format!(
                    "子として埋め込めるのは Html / Vec<Html> / String だけですが、`{ty}` 型が来ました"
                ),
                expr.span(),
            ));
        }
        self.check_interp_parts(parts)
    }

    fn check_interp_parts(&mut self, parts: &[StrPart]) -> Result<(), Diagnostic> {
        for part in parts {
            if let StrPart::Interp { expr, .. } = part {
                self.check_stringify(expr)?;
            }
        }
        Ok(())
    }

    fn check_stringify(&mut self, expr: &Expr) -> Result<(), Diagnostic> {
        let ty = self.infer_expr(expr)?;
        if ty == Ty::Str || ty.is_numeric() {
            Ok(())
        } else {
            Err(self.error(
                format!("文字列に埋め込めるのは String と数値だけですが、`{ty}` 型が来ました"),
                expr.span(),
            ))
        }
    }

    fn check_component(&mut self, component: &Component) -> Result<(), Diagnostic> {
        let (params, ret) = match self.decls.get(&component.name.text) {
            Some(DeclInfo::Func(params, ret)) => (params.clone(), ret.clone()),
            Some(DeclInfo::Record(_)) => {
                return Err(self.error(
                    format!(
                        "`{}` はレコード型でコンポーネントとして使えません",
                        component.name.text
                    ),
                    component.name.span,
                ));
            }
            None => {
                return Err(self.error(
                    format!(
                        "コンポーネント `{}` の定義がありません",
                        component.name.text
                    ),
                    component.name.span,
                ));
            }
        };
        if ret != Ty::Html {
            return Err(self.error(
                format!(
                    "コンポーネント `{}` は Html を返しません（`{ret}` 型）",
                    component.name.text
                ),
                component.name.span,
            ));
        }
        if component.children.is_empty() {
            if component.args.len() != params.len() {
                return Err(self.component_arity_error(component, params.len()));
            }
            for (arg, param) in component.args.iter().zip(&params) {
                self.check_expr(arg, param)?;
            }
            return Ok(());
        }
        let Some((last, explicit)) = params.split_last() else {
            return Err(self.error(
                format!("`{}` は子ブロックを受け取りません", component.name.text),
                component.span,
            ));
        };
        if !(last.is_html() || last.is_vec_of_html()) {
            return Err(self.error(
                format!(
                    "コンポーネント `{}` の末尾引数は `{last}` 型で、子ブロックを渡せません（Html か Vec<Html> が必要です）",
                    component.name.text
                ),
                component.span,
            ));
        }
        if component.args.len() != explicit.len() {
            return Err(self.error(
                format!(
                    "コンポーネント `{}` は明示引数を{}個取りますが、{}個渡されています（残り1個は子ブロック）",
                    component.name.text,
                    explicit.len(),
                    component.args.len()
                ),
                component.name.span,
            ));
        }
        for (arg, param) in component.args.iter().zip(explicit) {
            self.check_expr(arg, param)?;
        }
        for child in &component.children {
            self.check_child(child)?;
        }
        Ok(())
    }

    fn component_arity_error(&self, component: &Component, expected: usize) -> Diagnostic {
        self.error(
            format!(
                "コンポーネント `{}` は引数を{}個取りますが、{}個渡されています",
                component.name.text,
                expected,
                component.args.len()
            ),
            component.name.span,
        )
    }

    fn expect(&self, got: &Ty, want: &Ty, span: Span) -> Result<(), Diagnostic> {
        if got == want {
            return Ok(());
        }
        Err(self.error(
            format!("型が一致しません: `{want}` 型を期待しましたが、`{got}` 型が来ました"),
            span,
        ))
    }

    fn lookup(&self, name: &str) -> Option<Ty> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty.clone());
            }
        }
        None
    }

    fn define(&mut self, name: &str, ty: Ty) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), ty);
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn error(&self, message: String, span: Span) -> Diagnostic {
        Diagnostic::error(message).at(self.file, span)
    }
}

fn is_map(expr: &Expr) -> bool {
    matches!(expr, Expr::Var { name, .. } if name.text == "map")
}

fn child_span(child: &Child) -> Span {
    match child {
        Child::Element(element) => element.span,
        Child::Component(component) => component.span,
        Child::Text { span, .. } => *span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::resolve_program;
    use crate::source::SourceMap;
    use std::path::PathBuf;

    fn typeck_of(src: &str) -> (SourceMap, Result<Vec<Diagnostic>, Diagnostic>) {
        let mut sources = SourceMap::new();
        let file = sources.add_file(PathBuf::from("main.clum"), src.to_string());
        let program = resolve_program(&mut sources, file).expect("resolve に成功する前提です");
        let result = check_program(&program);
        (sources, result)
    }

    fn expect_ok(src: &str) -> Vec<Diagnostic> {
        let (sources, result) = typeck_of(src);
        result.unwrap_or_else(|diagnostic| {
            panic!("型検査に成功する前提です: {}", diagnostic.render(&sources))
        })
    }

    fn error_of(src: &str) -> String {
        let (sources, result) = typeck_of(src);
        let diagnostic = result.expect_err("型エラーを期待しました");
        diagnostic.render(&sources)
    }

    fn warning_of(src: &str) -> String {
        let (sources, result) = typeck_of(src);
        let warnings = result.expect("型検査に成功する前提です");
        assert_eq!(warnings.len(), 1, "警告が1件である前提です");
        warnings[0].render(&sources)
    }

    #[test]
    fn literal_with_annotation_ok() {
        expect_ok("x: i32 = 42\n");
        expect_ok("y: String = 'hi'\n");
        expect_ok("z: f64 = 3.14\n");
    }

    #[test]
    fn function_application_ok() {
        let src = concat!(
            "# Greet name: String -> String\n",
            "greet: Greet name -> name\n",
            "x = greet 'clum'\n",
        );
        assert!(expect_ok(src).is_empty());
    }

    #[test]
    fn partial_then_full_application_ok() {
        let src = concat!(
            "# Cat a: String, b: String -> String\n",
            "cat: Cat a, b -> a\n",
            "partial = cat 'a'\n",
            "full = partial 'b'\n",
        );
        assert!(expect_ok(src).is_empty());
    }

    #[test]
    fn pipe_tail_insertion_ok() {
        let src = concat!(
            "# Cat a: String, b: String -> String\n",
            "cat: Cat a, b -> a\n",
            "x = 'b' |> cat 'a'\n",
        );
        assert!(expect_ok(src).is_empty());
    }

    #[test]
    fn map_over_record_vec_ok() {
        let src = concat!(
            "# Page path: String, title: String\n",
            "pages: Vec<Page> =\n",
            "  - path: './a'\n",
            "    title: 'A'\n",
            "titles = pages\n",
            "  |> map page -> page.title\n",
        );
        assert!(expect_ok(src).is_empty());
    }

    #[test]
    fn record_construction_and_build_and_bang_ok() {
        let src = concat!(
            ":pub\n",
            "index: Html = h .html\n",
            "\n",
            "Recipe\n",
            "  documents:\n",
            "    - path: './dist/index.html'\n",
            "      element: index\n",
            "  |> build\n",
            "  |> !\n",
        );
        assert!(expect_ok(src).is_empty());
    }

    #[test]
    fn vec_literal_scalars_ok() {
        expect_ok("xs: Vec<i32> =\n  - 1\n  - 2\n  - 3\n");
    }

    #[test]
    fn element_with_text_child_ok() {
        expect_ok("x: Html = h .div\n  こんにちは\n");
    }

    #[test]
    fn element_with_attributes_ok() {
        expect_ok("x: Html = h .a href '/about', class 'nav'\n");
        expect_ok("x: Html = h .input type 'text', disabled\n");
        expect_ok("x: Html = h .div data-role 'main', aria-hidden 'true'\n");
    }

    #[test]
    fn component_with_child_block_ok() {
        let src = concat!(
            "# Card title: String, body: Html -> Html\n",
            "card: Card title, body -> h .div\n",
            "  {title}\n",
            ":pub\n",
            "page: Html = h .body\n",
            "  Card 'お知らせ'\n",
            "    h .p\n",
            "      本文\n",
        );
        assert!(expect_ok(src).is_empty());
    }

    #[test]
    fn interpolation_of_string_field_ok() {
        let src = concat!(
            "# Page path: String, title: String\n",
            "pages: Vec<Page> =\n",
            "  - path: './a'\n",
            "    title: 'A'\n",
            "items = pages\n",
            "  |> map page ->\n",
            "    h .li\n",
            "      {page.title} さん\n",
        );
        assert!(expect_ok(src).is_empty());
    }

    #[test]
    fn annotation_mismatch_is_error() {
        let message = error_of("x: String = 42\n");
        assert!(message.contains("`String` 型を期待しましたが、`i32` 型が来ました"));
        assert!(message.contains(":1:13"));
    }

    #[test]
    fn argument_type_mismatch_is_error() {
        let src = concat!(
            "# Greet name: String -> String\n",
            "greet: Greet name -> name\n",
            "x = greet 42\n",
        );
        let message = error_of(src);
        assert!(message.contains("引数の型が一致しません"));
        assert!(message.contains("`String` 型を期待しましたが、`i32` 型が来ました"));
        assert!(message.contains(":3:11"));
    }

    #[test]
    fn over_application_is_error() {
        let src = concat!(
            "# Greet name: String -> String\n",
            "greet: Greet name -> name\n",
            "x = greet 'a', 'b'\n",
        );
        let message = error_of(src);
        assert!(message.contains("これ以上引数を適用できません"));
    }

    #[test]
    fn unknown_field_is_error() {
        let src = concat!(
            "# Page path: String, title: String\n",
            "p: Page = Page\n",
            "  path: './a'\n",
            "  title: 'A'\n",
            "x = p.missing\n",
        );
        let message = error_of(src);
        assert!(message.contains("レコード `Page` にフィールド `missing` はありません"));
    }

    #[test]
    fn record_field_count_mismatch_is_error() {
        let src = concat!(
            "# Page path: String, title: String\n",
            "p: Page = Page\n",
            "  path: './a'\n",
        );
        let message = error_of(src);
        assert!(message.contains("フィールドを2個持ちますが、1個書かれています"));
    }

    #[test]
    fn record_field_order_violation_is_error() {
        let src = concat!(
            "# Page path: String, title: String\n",
            "p: Page = Page\n",
            "  title: 'A'\n",
            "  path: './a'\n",
        );
        let message = error_of(src);
        assert!(message.contains("1番目のフィールドは `path` ですが、`title` が書かれています"));
    }

    #[test]
    fn vec_element_type_mismatch_is_error() {
        let src = "xs: Vec<i32> =\n  - 1\n  - 'a'\n";
        let message = error_of(src);
        assert!(message.contains("`i32` 型を期待しましたが、`String` 型が来ました"));
    }

    #[test]
    fn vec_record_without_expected_type_is_error() {
        let src = "data =\n  - path: './a'\n    title: 'A'\n";
        let message = error_of(src);
        assert!(message.contains("リスト要素の型を決められません"));
        assert!(message.contains("型注釈"));
    }

    #[test]
    fn bang_on_non_eff_is_error() {
        let message = error_of("x = 'a' |> !\n");
        assert!(message.contains("`!` は `Eff<...>` にのみ使えますが、`String` 型が来ました"));
        assert!(message.contains(":1:12"));
    }

    #[test]
    fn unknown_tag_is_error_with_suggestion() {
        let src = ":pub\nindex: Html = h .dvi\n";
        let message = error_of(src);
        assert!(message.contains("`Tag` に `.dvi` はありません"));
        assert!(message.contains("もしかして `.div` ですか？"));
    }

    #[test]
    fn unknown_attribute_is_error() {
        let src = ":pub\nindex: Html = h .div foo 'bar'\n";
        let message = error_of(src);
        assert!(message.contains("`.div` 要素に属性 `foo` はありません"));
    }

    #[test]
    fn on_attribute_is_error() {
        let src = ":pub\nindex: Html = h .button on-click 'x'\n";
        let message = error_of(src);
        assert!(message.contains("`on-*` 属性 `on-click` は html ターゲットでは使えません"));
        assert!(message.contains("js ターゲットの責務"));
    }

    #[test]
    fn void_element_child_is_error() {
        let src = ":pub\nindex: Html = h .br\n  child\n";
        let message = error_of(src);
        assert!(message.contains("void 要素 `.br` は子を持てません"));
        assert!(message.contains(":3:3"));
    }

    #[test]
    fn boolean_attribute_with_value_is_error() {
        let src = ":pub\nindex: Html = h .input disabled 'x'\n";
        let message = error_of(src);
        assert!(message.contains("真偽属性 `disabled` に値は指定できません"));
    }

    #[test]
    fn value_attribute_without_value_is_error() {
        let src = ":pub\nindex: Html = h .a href\n";
        let message = error_of(src);
        assert!(message.contains("属性 `href` には値が必要です"));
    }

    #[test]
    fn whole_line_embed_of_bool_is_error() {
        let src = "x: Html = h .div\n  {true}\n";
        let message = error_of(src);
        assert!(message.contains(
            "子として埋め込めるのは Html / Vec<Html> / String だけですが、`Bool` 型が来ました"
        ));
    }

    #[test]
    fn inline_embed_of_bool_is_error() {
        let src = "x: Html = h .div\n  値は {true} です\n";
        let message = error_of(src);
        assert!(
            message.contains("文字列に埋め込めるのは String と数値だけですが、`Bool` 型が来ました")
        );
    }

    #[test]
    fn missing_alt_is_warning() {
        let src = ":pub\nindex: Html = h .img src '/logo.png'\n";
        let message = warning_of(src);
        assert!(message.starts_with("warning:"));
        assert!(message.contains("img 要素に alt 属性がありません"));
    }

    #[test]
    fn img_with_alt_has_no_warning() {
        let src = ":pub\nindex: Html = h .img src '/l.png', alt 'ロゴ'\n";
        assert!(expect_ok(src).is_empty());
    }

    #[test]
    fn unknown_generic_type_annotation_is_error() {
        let message = error_of("x: Nope<i32> = 1\n");
        assert!(
            message.contains("型 `Nope` は定義されていません") || message.contains("ジェネリック")
        );
    }
}
