use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::rc::Rc;

use crate::ast::{
    Binding, BindingKind, Child, Component, Decl, Element, Expr, Item, Module, Name, RecordField,
    StrPart, TypeExpr, VecElem,
};
use crate::diag::Diagnostic;
use crate::resolve::{ImportedKind, ImportedName, Program, ResolvedModule, kebab_of};
use crate::source::{FileId, SourceMap};
use crate::span::Span;
use crate::value::{Closure, Effect, Env, HtmlNode, Origin, RecordValue, UserFn, Value};

struct Ctx<'a> {
    base_dir: &'a Path,
    file: FileId,
    vec_tail: Rc<HashMap<String, bool>>,
}

impl<'a> Ctx<'a> {
    fn with_origin(&self, origin: &Origin) -> Self {
        Ctx {
            base_dir: self.base_dir,
            file: origin.file,
            vec_tail: origin.vec_tail.clone(),
        }
    }

    fn origin(&self) -> Origin {
        Origin {
            file: self.file,
            vec_tail: self.vec_tail.clone(),
        }
    }

    fn err(&self, message: impl Into<String>, span: Span) -> Diagnostic {
        Diagnostic::error(message).at(self.file, span)
    }
}

pub fn eval_program(sources: &SourceMap, program: &Program) -> Result<(), Diagnostic> {
    let base_dir = sources
        .get(program.entry)
        .path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let modules_by_file: HashMap<FileId, &Module> = program
        .modules
        .iter()
        .map(|resolved| (resolved.file, &resolved.module))
        .collect();
    let mut module_values: HashMap<FileId, HashMap<String, Value>> = HashMap::new();
    for resolved in &program.modules {
        let own = eval_module(resolved, &module_values, &modules_by_file, &base_dir)?;
        module_values.insert(resolved.file, own);
    }
    Ok(())
}

fn eval_module(
    resolved: &ResolvedModule,
    module_values: &HashMap<FileId, HashMap<String, Value>>,
    modules_by_file: &HashMap<FileId, &Module>,
    base_dir: &Path,
) -> Result<HashMap<String, Value>, Diagnostic> {
    let vec_tail = Rc::new(build_vec_tail_map(resolved, modules_by_file));
    let ctx = Ctx {
        base_dir,
        file: resolved.file,
        vec_tail,
    };

    let mut import_vars = HashMap::new();
    for import in &resolved.imports {
        for imported in &import.names {
            inject_imported(&mut import_vars, imported, module_values, &ctx)?;
        }
    }
    let mut env = prelude_env().child(import_vars);

    let mut own = HashMap::new();
    for item in &resolved.module.items {
        match item {
            Item::Decl(_) | Item::Import(_) => {}
            Item::Binding(binding) => {
                env = eval_binding(binding, &env, &ctx)?;
                let name = binding_name(binding);
                if let Some(value) = env.lookup(&name.text) {
                    own.insert(name.text.clone(), value);
                }
                if let BindingKind::Impl { def, .. } = &binding.kind
                    && let Some(value) = env.lookup(&def.text)
                {
                    own.insert(def.text.clone(), value);
                }
            }
            Item::Expr(expr) => {
                if resolved.is_entry {
                    eval_expr(expr, &env, &ctx)?;
                }
            }
        }
    }
    Ok(own)
}

fn inject_imported(
    import_vars: &mut HashMap<String, Value>,
    imported: &ImportedName,
    module_values: &HashMap<FileId, HashMap<String, Value>>,
    ctx: &Ctx,
) -> Result<(), Diagnostic> {
    let origin_values = module_values.get(&imported.origin);
    match &imported.kind {
        ImportedKind::Value { source } => {
            let value = origin_values
                .and_then(|values| values.get(source))
                .cloned()
                .ok_or_else(|| {
                    ctx.err(
                        format!("`{}` の値が求まっていません", imported.name.text),
                        imported.name.span,
                    )
                })?;
            import_vars.insert(imported.name.text.clone(), value);
        }
        ImportedKind::Decl { source_impl, .. } => {
            if let Some(source_impl) = source_impl {
                let value = origin_values
                    .and_then(|values| values.get(source_impl))
                    .cloned()
                    .ok_or_else(|| {
                        ctx.err(
                            format!("`{}` の実装の値が求まっていません", imported.name.text),
                            imported.name.span,
                        )
                    })?;
                import_vars.insert(imported.name.text.clone(), value.clone());
                import_vars.insert(kebab_of(&imported.name.text), value);
            }
        }
    }
    Ok(())
}

fn prelude_env() -> Env {
    let mut vars = HashMap::new();
    vars.insert("true".to_string(), Value::Bool(true));
    vars.insert("false".to_string(), Value::Bool(false));
    vars.insert("build".to_string(), Value::Closure(Rc::new(Closure::Build)));
    Env::empty().child(vars)
}

fn binding_name(binding: &Binding) -> &Name {
    match &binding.kind {
        BindingKind::Value { name, .. } => name,
        BindingKind::Impl { name, .. } => name,
    }
}

fn is_vec_html_type(ty: &TypeExpr) -> bool {
    matches!(
        ty,
        TypeExpr::Generic { name, arg, .. }
            if name.text == "Vec"
                && matches!(arg.as_ref(), TypeExpr::Name { name, .. } if name.text == "Html")
    )
}

fn last_param_is_vec_html(decl: &Decl) -> bool {
    decl.params
        .last()
        .map(|field| is_vec_html_type(&field.ty))
        .unwrap_or(false)
}

fn build_vec_tail_map(
    resolved: &ResolvedModule,
    modules_by_file: &HashMap<FileId, &Module>,
) -> HashMap<String, bool> {
    let mut map: HashMap<String, bool> = resolved
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Decl(decl) if decl.ret.is_some() => {
                Some((decl.name.text.clone(), last_param_is_vec_html(decl)))
            }
            _ => None,
        })
        .collect();
    let imported_names = resolved
        .imports
        .iter()
        .flat_map(|import| import.names.iter());
    for imported in imported_names {
        let ImportedKind::Decl { source_decl, .. } = &imported.kind else {
            continue;
        };
        let Some(module) = modules_by_file.get(&imported.origin) else {
            continue;
        };
        for item in &module.items {
            if let Item::Decl(decl) = item
                && decl.name.text == *source_decl
                && decl.ret.is_some()
            {
                map.insert(imported.name.text.clone(), last_param_is_vec_html(decl));
            }
        }
    }
    map
}

fn eval_binding(binding: &Binding, env: &Env, ctx: &Ctx) -> Result<Env, Diagnostic> {
    match &binding.kind {
        BindingKind::Value { name, value, .. } => {
            let evaluated = eval_expr(value, env, ctx)?;
            let mut vars = HashMap::new();
            vars.insert(name.text.clone(), evaluated);
            Ok(env.child(vars))
        }
        BindingKind::Impl {
            name, def, params, ..
        } => {
            let body = binding_body(binding);
            let func = Rc::new(UserFn {
                params: params.iter().map(|param| param.text.clone()).collect(),
                body: Rc::new(body.clone()),
                env: env.clone(),
                origin: ctx.origin(),
            });
            let closure = Value::Closure(Rc::new(Closure::User {
                func,
                bound: Vec::new(),
            }));
            let mut vars = HashMap::new();
            vars.insert(name.text.clone(), closure.clone());
            vars.insert(def.text.clone(), closure);
            Ok(env.child(vars))
        }
    }
}

fn binding_body(binding: &Binding) -> &Expr {
    match &binding.kind {
        BindingKind::Impl { body, .. } => body,
        BindingKind::Value { value, .. } => value,
    }
}

fn eval_expr(expr: &Expr, env: &Env, ctx: &Ctx) -> Result<Value, Diagnostic> {
    match expr {
        Expr::Int { value, .. } => Ok(Value::I32(*value as i32)),
        Expr::Float { value, .. } => Ok(Value::F64(*value)),
        Expr::Str { parts, .. } => {
            let text = eval_text_parts(parts, env, ctx)?;
            Ok(Value::Str(Rc::from(text.as_str())))
        }
        Expr::Var { name, span } => env
            .lookup(&name.text)
            .ok_or_else(|| ctx.err(format!("名前 `{}` の値が求まりません", name.text), *span)),
        Expr::Field { base, field, span } => {
            let base_value = eval_expr(base, env, ctx)?;
            eval_field(&base_value, &field.text, *span, ctx)
        }
        Expr::App { func, args, span } => eval_app(func, args, env, ctx, *span),
        Expr::Lambda { param, body, .. } => Ok(Value::Closure(Rc::new(Closure::Lambda {
            param: param.text.clone(),
            body: Rc::new((**body).clone()),
            env: env.clone(),
            origin: ctx.origin(),
        }))),
        Expr::Pipe { lhs, rhs, span } => eval_pipe(lhs, rhs, env, ctx, *span),
        Expr::Bang { span } => Err(ctx.err(
            "`!` はパイプ `|>` の右側でのみ評価できます".to_string(),
            *span,
        )),
        Expr::Record { fields, .. } => eval_record(fields, env, ctx),
        Expr::Vec { elems, .. } => eval_vec(elems, env, ctx),
        Expr::Block {
            bindings, result, ..
        } => eval_block(bindings, result, env, ctx),
        Expr::Element(element) => {
            let node = eval_element(element, env, ctx)?;
            Ok(Value::Html(Rc::new(node)))
        }
    }
}

fn eval_text_parts(parts: &[StrPart], env: &Env, ctx: &Ctx) -> Result<String, Diagnostic> {
    let mut out = String::new();
    for part in parts {
        match part {
            StrPart::Text { text, .. } => out.push_str(text),
            StrPart::Interp { expr, .. } => {
                let value = eval_expr(expr, env, ctx)?;
                out.push_str(&stringify_value(&value));
            }
        }
    }
    Ok(out)
}

fn stringify_value(value: &Value) -> String {
    match value {
        Value::Str(text) => text.to_string(),
        Value::I32(n) => n.to_string(),
        Value::F64(n) => n.to_string(),
        _ => String::new(),
    }
}

fn eval_field(base: &Value, field: &str, span: Span, ctx: &Ctx) -> Result<Value, Diagnostic> {
    match base {
        Value::Record(record) => record
            .field(field)
            .cloned()
            .ok_or_else(|| ctx.err(format!("フィールド `{field}` が見つかりません"), span)),
        _ => Err(ctx.err(
            "レコードではない値にフィールドアクセスしています".to_string(),
            span,
        )),
    }
}

fn is_map(expr: &Expr) -> bool {
    matches!(expr, Expr::Var { name, .. } if name.text == "map")
}

fn eval_app(
    func: &Expr,
    args: &[Expr],
    env: &Env,
    ctx: &Ctx,
    span: Span,
) -> Result<Value, Diagnostic> {
    if is_map(func) {
        let mapper = eval_expr(&args[0], env, ctx)?;
        let vec_value = eval_expr(&args[1], env, ctx)?;
        return eval_map(mapper, vec_value, ctx, span);
    }
    let func_value = eval_expr(func, env, ctx)?;
    let mut arg_values = Vec::with_capacity(args.len());
    for arg in args {
        arg_values.push(eval_expr(arg, env, ctx)?);
    }
    apply_value(func_value, arg_values, ctx, span)
}

fn eval_pipe(
    lhs: &Expr,
    rhs: &Expr,
    env: &Env,
    ctx: &Ctx,
    span: Span,
) -> Result<Value, Diagnostic> {
    match rhs {
        Expr::Bang { span: bang_span } => {
            let lhs_value = eval_expr(lhs, env, ctx)?;
            run_effect(lhs_value, ctx, *bang_span)
        }
        Expr::App { func, args, .. } if is_map(func) => {
            let lhs_value = eval_expr(lhs, env, ctx)?;
            let mapper = eval_expr(&args[0], env, ctx)?;
            eval_map(mapper, lhs_value, ctx, span)
        }
        Expr::App { func, args, .. } => {
            let lhs_value = eval_expr(lhs, env, ctx)?;
            let func_value = eval_expr(func, env, ctx)?;
            let mut arg_values = Vec::with_capacity(args.len() + 1);
            for arg in args {
                arg_values.push(eval_expr(arg, env, ctx)?);
            }
            arg_values.push(lhs_value);
            apply_value(func_value, arg_values, ctx, span)
        }
        _ => {
            let lhs_value = eval_expr(lhs, env, ctx)?;
            let func_value = eval_expr(rhs, env, ctx)?;
            apply_value(func_value, vec![lhs_value], ctx, span)
        }
    }
}

fn eval_map(mapper: Value, vec_value: Value, ctx: &Ctx, span: Span) -> Result<Value, Diagnostic> {
    let items = match vec_value {
        Value::Vec(items) => items,
        _ => {
            return Err(ctx.err("`map` の対象は Vec である必要があります".to_string(), span));
        }
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items.iter() {
        out.push(apply_value(mapper.clone(), vec![item.clone()], ctx, span)?);
    }
    Ok(Value::Vec(Rc::new(out)))
}

fn apply_value(func: Value, args: Vec<Value>, ctx: &Ctx, span: Span) -> Result<Value, Diagnostic> {
    let Value::Closure(closure) = func else {
        return Err(ctx.err("関数ではない値は適用できません".to_string(), span));
    };
    match closure.as_ref() {
        Closure::Build => {
            let mut args = args.into_iter();
            let recipe = args
                .next()
                .ok_or_else(|| ctx.err("`build` には Recipe が必要です".to_string(), span))?;
            Ok(Value::Eff(Rc::new(Effect::Build(recipe))))
        }
        Closure::Lambda {
            param,
            body,
            env,
            origin,
            ..
        } => {
            let mut args = args.into_iter();
            let arg = args
                .next()
                .ok_or_else(|| ctx.err("ラムダに渡す引数がありません".to_string(), span))?;
            let mut vars = HashMap::new();
            vars.insert(param.clone(), arg);
            let new_env = env.child(vars);
            let call_ctx = ctx.with_origin(origin);
            eval_expr(body, &new_env, &call_ctx)
        }
        Closure::User { func, bound } => {
            let mut all_bound = bound.clone();
            all_bound.extend(args);
            if all_bound.len() < func.params.len() {
                return Ok(Value::Closure(Rc::new(Closure::User {
                    func: func.clone(),
                    bound: all_bound,
                })));
            }
            let mut vars = HashMap::new();
            for (name, value) in func.params.iter().zip(all_bound) {
                vars.insert(name.clone(), value);
            }
            let new_env = func.env.child(vars);
            let call_ctx = ctx.with_origin(&func.origin);
            eval_expr(&func.body, &new_env, &call_ctx)
        }
    }
}

fn run_effect(value: Value, ctx: &Ctx, span: Span) -> Result<Value, Diagnostic> {
    let Value::Eff(effect) = value else {
        return Err(ctx.err("`!` は `Eff<...>` にのみ使えます".to_string(), span));
    };
    match effect.as_ref() {
        Effect::Build(recipe) => {
            run_build(recipe, ctx, span)?;
            Ok(Value::Void)
        }
    }
}

fn run_build(recipe: &Value, ctx: &Ctx, span: Span) -> Result<(), Diagnostic> {
    let Value::Record(record) = recipe else {
        return Err(ctx.err("`build` には Recipe レコードが必要です".to_string(), span));
    };
    let Some(Value::Vec(documents)) = record.field("documents") else {
        return Err(ctx.err(
            "Recipe に `documents` フィールドが見つかりません".to_string(),
            span,
        ));
    };
    for document in documents.iter() {
        let Value::Record(document) = document else {
            return Err(ctx.err("Document レコードが必要です".to_string(), span));
        };
        let Some(Value::Str(path_text)) = document.field("path") else {
            return Err(ctx.err(
                "Document の `path` は String である必要があります".to_string(),
                span,
            ));
        };
        let Some(Value::Html(html)) = document.field("element") else {
            return Err(ctx.err(
                "Document の `element` は Html である必要があります".to_string(),
                span,
            ));
        };
        let output = crate::emit::render(html);
        let out_path = ctx.base_dir.join(path_text.as_ref());
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                ctx.err(
                    format!(
                        "出力先ディレクトリ `{}` を作成できません: {err}",
                        parent.display()
                    ),
                    span,
                )
            })?;
        }
        fs::write(&out_path, output).map_err(|err| {
            ctx.err(
                format!("ファイル `{}` へ書き出せません: {err}", out_path.display()),
                span,
            )
        })?;
    }
    Ok(())
}

fn eval_record(fields: &[RecordField], env: &Env, ctx: &Ctx) -> Result<Value, Diagnostic> {
    let mut out = Vec::with_capacity(fields.len());
    for field in fields {
        let value = eval_expr(&field.value, env, ctx)?;
        out.push((field.name.text.clone(), value));
    }
    Ok(Value::Record(Rc::new(RecordValue { fields: out })))
}

fn eval_vec(elems: &[VecElem], env: &Env, ctx: &Ctx) -> Result<Value, Diagnostic> {
    let mut out = Vec::with_capacity(elems.len());
    for elem in elems {
        let value = match elem {
            VecElem::Expr(expr) => eval_expr(expr, env, ctx)?,
            VecElem::Record { fields, .. } => eval_record(fields, env, ctx)?,
        };
        out.push(value);
    }
    Ok(Value::Vec(Rc::new(out)))
}

fn eval_block(
    bindings: &[Binding],
    result: &Expr,
    env: &Env,
    ctx: &Ctx,
) -> Result<Value, Diagnostic> {
    let mut current = env.clone();
    for binding in bindings {
        current = eval_binding(binding, &current, ctx)?;
    }
    eval_expr(result, &current, ctx)
}

fn eval_element(element: &Element, env: &Env, ctx: &Ctx) -> Result<HtmlNode, Diagnostic> {
    let mut attrs = Vec::with_capacity(element.attrs.len());
    for attr in &element.attrs {
        let value = match &attr.value {
            Some(expr) => {
                let evaluated = eval_expr(expr, env, ctx)?;
                Some(expect_string(&evaluated, expr.span(), ctx)?)
            }
            None => None,
        };
        attrs.push((attr.name.text.clone(), value));
    }
    let mut children = Vec::new();
    for child in &element.children {
        eval_child_into(child, env, ctx, &mut children)?;
    }
    Ok(HtmlNode::Element {
        tag: element.tag.text.clone(),
        attrs,
        children,
    })
}

fn expect_string(value: &Value, span: Span, ctx: &Ctx) -> Result<String, Diagnostic> {
    match value {
        Value::Str(text) => Ok(text.to_string()),
        _ => Err(ctx.err("属性値は String である必要があります".to_string(), span)),
    }
}

fn unwrap_html(value: Value, span: Span, ctx: &Ctx) -> Result<HtmlNode, Diagnostic> {
    match value {
        Value::Html(node) => Ok((*node).clone()),
        _ => Err(ctx.err("Html 型の値が必要です".to_string(), span)),
    }
}

fn eval_child_into(
    child: &Child,
    env: &Env,
    ctx: &Ctx,
    out: &mut Vec<HtmlNode>,
) -> Result<(), Diagnostic> {
    match child {
        Child::Element(element) => {
            out.push(eval_element(element, env, ctx)?);
            Ok(())
        }
        Child::Component(component) => {
            let value = eval_component(component, env, ctx)?;
            out.push(unwrap_html(value, component.span, ctx)?);
            Ok(())
        }
        Child::Text { parts, span } => {
            if let [StrPart::Interp { expr, .. }] = parts.as_slice() {
                let value = eval_expr(expr, env, ctx)?;
                match value {
                    Value::Html(node) => out.push((*node).clone()),
                    Value::Vec(items) => {
                        for item in items.iter() {
                            out.push(unwrap_html(item.clone(), *span, ctx)?);
                        }
                    }
                    Value::Str(text) => out.push(HtmlNode::Text(text.to_string())),
                    _ => {
                        return Err(ctx.err(
                            "子として埋め込めるのは Html / Vec<Html> / String だけです".to_string(),
                            *span,
                        ));
                    }
                }
                Ok(())
            } else {
                let text = eval_text_parts(parts, env, ctx)?;
                out.push(HtmlNode::Text(text));
                Ok(())
            }
        }
    }
}

fn eval_component(component: &Component, env: &Env, ctx: &Ctx) -> Result<Value, Diagnostic> {
    let func_value = env.lookup(&component.name.text).ok_or_else(|| {
        ctx.err(
            format!(
                "コンポーネント `{}` の値が見つかりません",
                component.name.text
            ),
            component.name.span,
        )
    })?;
    let mut args = Vec::with_capacity(component.args.len() + 1);
    for arg in &component.args {
        args.push(eval_expr(arg, env, ctx)?);
    }
    if !component.children.is_empty() {
        let mut child_htmls = Vec::new();
        for child in &component.children {
            eval_child_into(child, env, ctx, &mut child_htmls)?;
        }
        let is_vec_tail = ctx
            .vec_tail
            .get(&component.name.text)
            .copied()
            .unwrap_or(false);
        if is_vec_tail {
            let items: Vec<Value> = child_htmls
                .into_iter()
                .map(|node| Value::Html(Rc::new(node)))
                .collect();
            args.push(Value::Vec(Rc::new(items)));
        } else {
            let node = child_htmls
                .into_iter()
                .next()
                .ok_or_else(|| ctx.err("コンポーネントの子が空です".to_string(), component.span))?;
            args.push(Value::Html(Rc::new(node)));
        }
    }
    apply_value(func_value, args, ctx, component.span)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::resolve_program;
    use crate::typeck::check_program;
    use std::env;
    use std::fs;
    use std::path::PathBuf;

    struct TmpDir {
        path: PathBuf,
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn temp_dir(tag: &str) -> TmpDir {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = env::temp_dir().join(format!("clum-eval-{tag}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        TmpDir { path }
    }

    fn checked_program(entry: &Path) -> (SourceMap, crate::resolve::Program) {
        let mut sources = SourceMap::new();
        let program = resolve_program(&mut sources, entry).unwrap_or_else(|diagnostic| {
            panic!(
                "resolve に成功する前提です: {}",
                diagnostic.render(&sources)
            )
        });
        check_program(&program).unwrap_or_else(|diagnostic| {
            panic!("型検査に成功する前提です: {}", diagnostic.render(&sources))
        });
        (sources, program)
    }

    fn eval_value(src: &str, name: &str) -> Value {
        let dir = temp_dir("value");
        let entry = dir.path.join("entry.clum");
        fs::write(&entry, src).unwrap();
        let (sources, program) = checked_program(&entry);
        let modules_by_file: HashMap<FileId, &Module> = program
            .modules
            .iter()
            .map(|resolved| (resolved.file, &resolved.module))
            .collect();
        let mut module_values: HashMap<FileId, HashMap<String, Value>> = HashMap::new();
        for resolved in &program.modules {
            let own = eval_module(resolved, &module_values, &modules_by_file, &dir.path)
                .unwrap_or_else(|diagnostic| {
                    panic!("評価に成功する前提です: {}", diagnostic.render(&sources))
                });
            module_values.insert(resolved.file, own);
        }
        module_values[&program.entry][name].clone()
    }

    fn as_str(value: &Value) -> String {
        match value {
            Value::Str(s) => s.to_string(),
            other => panic!("String を期待しましたが {other:?} でした"),
        }
    }

    fn as_html(value: &Value) -> HtmlNode {
        match value {
            Value::Html(node) => (**node).clone(),
            other => panic!("Html を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn partial_application_and_full_application() {
        let src = concat!(
            "# Cat a: String, b: String -> String\n",
            "cat: Cat a, b -> a\n",
            "partial = cat 'first'\n",
            "x: String = partial 'second'\n",
        );
        assert_eq!(as_str(&eval_value(src, "x")), "first");
    }

    #[test]
    fn pipe_tail_insertion_applies_function() {
        let src = concat!(
            "# Cat a: String, b: String -> String\n",
            "cat: Cat a, b -> b\n",
            "x: String = 'tail' |> cat 'head'\n",
        );
        assert_eq!(as_str(&eval_value(src, "x")), "tail");
    }

    #[test]
    fn interpolation_evaluates_field_access() {
        let src = concat!(
            "# Page path: String, title: String\n",
            "page: Page = Page\n",
            "  path: './a'\n",
            "  title: 'A'\n",
            "x: String = '{page.title} です'\n",
        );
        assert_eq!(as_str(&eval_value(src, "x")), "A です");
    }

    #[test]
    fn map_over_vec_evaluates_each_element() {
        let src = concat!(
            "xs: Vec<i32> =\n",
            "  - 1\n",
            "  - 2\n",
            "  - 3\n",
            "ys: Vec<String> = xs |> map n -> '{n}!'\n",
        );
        match eval_value(src, "ys") {
            Value::Vec(items) => {
                let rendered: Vec<String> = items.iter().map(as_str).collect();
                assert_eq!(rendered, vec!["1!", "2!", "3!"]);
            }
            other => panic!("Vec を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn record_construction_builds_fields() {
        let src = concat!(
            "# Page path: String, title: String\n",
            "page: Page = Page\n",
            "  path: './a'\n",
            "  title: 'A'\n",
        );
        match eval_value(src, "page") {
            Value::Record(record) => {
                assert_eq!(as_str(record.field("path").unwrap()), "./a");
                assert_eq!(as_str(record.field("title").unwrap()), "A");
            }
            other => panic!("Record を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn vec_literal_of_records() {
        let src = concat!(
            "# Page path: String, title: String\n",
            "pages: Vec<Page> =\n",
            "  - path: './a'\n",
            "    title: 'A'\n",
            "  - path: './b'\n",
            "    title: 'B'\n",
        );
        match eval_value(src, "pages") {
            Value::Vec(items) => {
                assert_eq!(items.len(), 2);
                match &items[0] {
                    Value::Record(record) => {
                        assert_eq!(as_str(record.field("path").unwrap()), "./a")
                    }
                    other => panic!("Record を期待しましたが {other:?} でした"),
                }
            }
            other => panic!("Vec を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn html_tree_with_attributes_and_text() {
        let src = "x: Html = h .a href '/about'\n  about\n";
        match as_html(&eval_value(src, "x")) {
            HtmlNode::Element {
                tag,
                attrs,
                children,
            } => {
                assert_eq!(tag, "a");
                assert_eq!(
                    attrs,
                    vec![("href".to_string(), Some("/about".to_string()))]
                );
                assert_eq!(children.len(), 1);
                match &children[0] {
                    HtmlNode::Text(text) => assert_eq!(text, "about"),
                    other => panic!("Text を期待しましたが {other:?} でした"),
                }
            }
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn boolean_attribute_has_no_value() {
        let src = "x: Html = h .input type 'text', disabled\n";
        match as_html(&eval_value(src, "x")) {
            HtmlNode::Element { attrs, .. } => {
                assert_eq!(attrs[1], ("disabled".to_string(), None));
            }
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn vec_html_splice_via_map() {
        let src = concat!(
            "# Page path: String, title: String\n",
            "pages: Vec<Page> =\n",
            "  - path: './a'\n",
            "    title: 'A'\n",
            "  - path: './b'\n",
            "    title: 'B'\n",
            "items = pages\n",
            "  |> map page ->\n",
            "    h .li\n",
            "      {page.title}\n",
            "x: Html = h .ul\n",
            "  {items}\n",
        );
        match as_html(&eval_value(src, "x")) {
            HtmlNode::Element { tag, children, .. } => {
                assert_eq!(tag, "ul");
                assert_eq!(children.len(), 2);
                for child in &children {
                    match child {
                        HtmlNode::Element { tag, .. } => assert_eq!(tag, "li"),
                        other => panic!("li 要素を期待しましたが {other:?} でした"),
                    }
                }
            }
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn component_with_single_html_child() {
        let src = concat!(
            "# Card title: String, body: Html -> Html\n",
            "card: Card title, body -> h .div\n",
            "  {title}\n",
            "  {body}\n",
            "page: Html = h .body\n",
            "  Card 'お知らせ'\n",
            "    h .p\n",
            "      本文\n",
        );
        match as_html(&eval_value(src, "page")) {
            HtmlNode::Element { children, .. } => match &children[0] {
                HtmlNode::Element { children, .. } => {
                    assert_eq!(children.len(), 2);
                    match &children[0] {
                        HtmlNode::Text(text) => assert_eq!(text, "お知らせ"),
                        other => panic!("Text を期待しましたが {other:?} でした"),
                    }
                    match &children[1] {
                        HtmlNode::Element { tag, .. } => assert_eq!(tag, "p"),
                        other => panic!("p 要素を期待しましたが {other:?} でした"),
                    }
                }
                other => panic!("div 要素を期待しましたが {other:?} でした"),
            },
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn component_with_vec_html_children() {
        let src = concat!(
            "# Card title: String, body: Vec<Html> -> Html\n",
            "card: Card title, body -> h .div\n",
            "  {body}\n",
            "page: Html = h .body\n",
            "  Card 'お知らせ'\n",
            "    h .p\n",
            "      本文\n",
            "    h .p\n",
            "      追伸\n",
        );
        match as_html(&eval_value(src, "page")) {
            HtmlNode::Element { children, .. } => match &children[0] {
                HtmlNode::Element { children, .. } => {
                    assert_eq!(children.len(), 2);
                    for child in children {
                        match child {
                            HtmlNode::Element { tag, .. } => assert_eq!(tag, "p"),
                            other => panic!("p 要素を期待しましたが {other:?} でした"),
                        }
                    }
                }
                other => panic!("div 要素を期待しましたが {other:?} でした"),
            },
            other => panic!("Element を期待しましたが {other:?} でした"),
        }
    }

    #[test]
    fn top_level_bang_runs_in_order() {
        let dir = temp_dir("order");
        let src = concat!(
            "index: Html = h .html\n",
            "\n",
            "Recipe\n",
            "  documents:\n",
            "    - path: './dist/index.html'\n",
            "      element: index\n",
            "  |> build\n",
            "  |> !\n",
        );
        fs::write(dir.path.join("site.clum"), src).unwrap();

        let (sources, program) = checked_program(&dir.path.join("site.clum"));
        eval_program(&sources, &program).expect("評価に成功する前提です");

        let output = fs::read_to_string(dir.path.join("dist/index.html")).unwrap();
        assert!(output.starts_with("<!DOCTYPE html>"));
        assert!(output.contains("<html>"));
    }

    #[test]
    fn build_writes_multiple_documents() {
        let dir = temp_dir("multi");
        let src = concat!(
            "index: Html = h .div\n",
            "  一\n",
            "\n",
            "Recipe\n",
            "  documents:\n",
            "    - path: './dist/a.html'\n",
            "      element: index\n",
            "    - path: './dist/b.html'\n",
            "      element: index\n",
            "  |> build\n",
            "  |> !\n",
        );
        fs::write(dir.path.join("site.clum"), src).unwrap();

        let (sources, program) = checked_program(&dir.path.join("site.clum"));
        eval_program(&sources, &program).expect("評価に成功する前提です");

        assert!(dir.path.join("dist/a.html").is_file());
        assert!(dir.path.join("dist/b.html").is_file());
    }

    #[test]
    fn multi_file_import_evaluates_exported_value() {
        let dir = temp_dir("import");
        fs::write(
            dir.path.join("index.clum"),
            "^index: Html = h .div\n  中身\n",
        )
        .unwrap();
        fs::write(
            dir.path.join("site.clum"),
            concat!(
                "@./index\n",
                "  index\n",
                "\n",
                "Recipe\n",
                "  documents:\n",
                "    - path: './dist/index.html'\n",
                "      element: index\n",
                "  |> build\n",
                "  |> !\n",
            ),
        )
        .unwrap();

        let (sources, program) = checked_program(&dir.path.join("site.clum"));
        eval_program(&sources, &program).expect("評価に成功する前提です");

        let output = fs::read_to_string(dir.path.join("dist/index.html")).unwrap();
        assert!(output.contains("中身"));
    }

    #[test]
    fn imported_component_renders_across_files() {
        let dir = temp_dir("component-across");
        fs::write(
            dir.path.join("card.clum"),
            concat!(
                "^# Card title: String, body: Vec<Html> -> Html\n",
                "card: Card title, body -> h .section\n",
                "  {title}\n",
                "  {body}\n",
            ),
        )
        .unwrap();
        fs::write(
            dir.path.join("site.clum"),
            concat!(
                "@./card\n",
                "  Card\n",
                "\n",
                "index: Html = h .body\n",
                "  Card 'お知らせ'\n",
                "    h .p\n",
                "      本文\n",
                "\n",
                "Recipe\n",
                "  documents:\n",
                "    - path: './dist/index.html'\n",
                "      element: index\n",
                "  |> build\n",
                "  |> !\n",
            ),
        )
        .unwrap();

        let (sources, program) = checked_program(&dir.path.join("site.clum"));
        eval_program(&sources, &program).expect("評価に成功する前提です");

        let output = fs::read_to_string(dir.path.join("dist/index.html")).unwrap();
        assert!(output.contains("<section>お知らせ<p>本文</p></section>"));
    }

    #[test]
    fn program_file_exprs_do_not_run_unless_entry() {
        let dir = temp_dir("program-no-run");
        fs::create_dir_all(dir.path.join("lib")).unwrap();
        fs::write(
            dir.path.join("lib/page.clum"),
            "^page: Html = h .div\n  中身\n",
        )
        .unwrap();
        fs::write(
            dir.path.join("lib/recipe.clum"),
            concat!(
                "@./page\n",
                "  page\n",
                "\n",
                "Recipe\n",
                "  documents:\n",
                "    - path: './never.html'\n",
                "      element: page\n",
                "  |> build\n",
                "  |> !\n",
            ),
        )
        .unwrap();
        fs::write(dir.path.join("lib/_.clum"), "^@./page\n  page\n").unwrap();
        fs::write(
            dir.path.join("site.clum"),
            concat!(
                "@./lib\n",
                "  page\n",
                "\n",
                "Recipe\n",
                "  documents:\n",
                "    - path: './dist/index.html'\n",
                "      element: page\n",
                "  |> build\n",
                "  |> !\n",
            ),
        )
        .unwrap();

        let (sources, program) = checked_program(&dir.path.join("site.clum"));
        eval_program(&sources, &program).expect("評価に成功する前提です");

        assert!(dir.path.join("dist/index.html").is_file());
        assert!(!dir.path.join("lib/never.html").exists());
        assert!(!dir.path.join("never.html").exists());
    }
}
