use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::ast::{
    Binding, BindingKind, Child, Component as AstComponent, Element, Expr, Import, Item, Module,
    Name, StrPart, VecElem,
};
use crate::diag::Diagnostic;
use crate::parser::parse;
use crate::source::{FileId, SourceMap};
use crate::span::Span;

const PRELUDE_VALUES: &[&str] = &["map", "build", "true", "false"];
const PRELUDE_DECLS: &[&str] = &["Recipe", "Document"];

#[derive(Debug)]
pub struct Program {
    pub entry: FileId,
    pub modules: Vec<ResolvedModule>,
}

#[derive(Debug)]
pub struct ResolvedModule {
    pub file: FileId,
    pub module: Module,
    pub export: Option<Name>,
    pub imports: Vec<ModuleImport>,
}

#[derive(Debug, Clone)]
pub struct ModuleImport {
    pub file: FileId,
    pub names: Vec<Name>,
    pub path_text: String,
    pub span: Span,
}

pub fn resolve_program(sources: &mut SourceMap, entry: FileId) -> Result<Program, Diagnostic> {
    let entry_path = sources.get(entry).path().to_path_buf();
    let entry_canon = fs::canonicalize(&entry_path).unwrap_or(entry_path);
    let modules = {
        let mut loader = Loader {
            sources,
            by_path: HashMap::new(),
            on_stack: Vec::new(),
            modules: Vec::new(),
        };
        loader.load(entry, entry_canon)?;
        loader.modules
    };
    check_modules(sources, &modules)?;
    Ok(Program { entry, modules })
}

struct Loader<'a> {
    sources: &'a mut SourceMap,
    by_path: HashMap<PathBuf, FileId>,
    on_stack: Vec<(PathBuf, FileId)>,
    modules: Vec<ResolvedModule>,
}

impl Loader<'_> {
    fn load(&mut self, file: FileId, canon: PathBuf) -> Result<(), Diagnostic> {
        self.on_stack.push((canon.clone(), file));
        let content = self.sources.get(file).content().to_string();
        let module = parse(&content, file)?;
        let export = find_export(&module);
        let mut imports = Vec::new();
        for item in &module.items {
            if let Item::Import(import) = item {
                let target = self.resolve_import(file, import)?;
                imports.push(ModuleImport {
                    file: target,
                    names: import.names.clone(),
                    path_text: import.path.text.clone(),
                    span: import.span,
                });
            }
        }
        self.on_stack.pop();
        self.by_path.insert(canon, file);
        self.modules.push(ResolvedModule {
            file,
            module,
            export,
            imports,
        });
        Ok(())
    }

    fn resolve_import(&mut self, importer: FileId, import: &Import) -> Result<FileId, Diagnostic> {
        let base = self
            .sources
            .get(importer)
            .path()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let joined = clean_path(&base.join(format!("{}.clum", import.path.text)));
        let canon = match fs::canonicalize(&joined) {
            Ok(canon) => canon,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(Diagnostic::error(format!(
                    "import 先のファイル `{}` が見つかりません",
                    joined.display()
                ))
                .at(importer, import.path.span));
            }
            Err(err) => {
                return Err(Diagnostic::error(format!(
                    "import 先のファイル `{}` を読み込めません: {err}",
                    joined.display()
                ))
                .at(importer, import.path.span));
            }
        };
        if let Some(pos) = self.on_stack.iter().position(|(path, _)| path == &canon) {
            let mut chain: Vec<String> = self.on_stack[pos..]
                .iter()
                .map(|(_, id)| self.sources.get(*id).path().display().to_string())
                .collect();
            chain.push(
                self.sources
                    .get(self.on_stack[pos].1)
                    .path()
                    .display()
                    .to_string(),
            );
            return Err(Diagnostic::error(format!(
                "循環 import を検出しました: {}",
                chain.join(" -> ")
            ))
            .at(importer, import.path.span));
        }
        if let Some(&existing) = self.by_path.get(&canon) {
            return Ok(existing);
        }
        let content = match fs::read_to_string(&canon) {
            Ok(content) => content,
            Err(err) => {
                return Err(Diagnostic::error(format!(
                    "import 先のファイル `{}` を読み込めません: {err}",
                    joined.display()
                ))
                .at(importer, import.path.span));
            }
        };
        let target = self.sources.add_file(joined, content);
        self.load(target, canon)?;
        Ok(target)
    }
}

fn clean_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

fn find_export(module: &Module) -> Option<Name> {
    module.items.iter().find_map(|item| match item {
        Item::Binding(binding) if binding.is_pub => Some(binding_name(binding).clone()),
        _ => None,
    })
}

fn binding_name(binding: &Binding) -> &Name {
    match &binding.kind {
        BindingKind::Value { name, .. } => name,
        BindingKind::Impl { name, .. } => name,
    }
}

fn check_modules(sources: &SourceMap, modules: &[ResolvedModule]) -> Result<(), Diagnostic> {
    let export_of: HashMap<FileId, Option<Name>> = modules
        .iter()
        .map(|module| (module.file, module.export.clone()))
        .collect();
    for module in modules {
        let mut checker = Checker::new(sources, module.file);
        checker.check(module, &export_of)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Prelude,
    Import,
    TopLevel,
    Local,
}

#[derive(Debug, Clone, Copy)]
struct Binder {
    origin: Origin,
    boundary: usize,
    span: Span,
}

#[derive(Debug, Clone, Copy)]
enum DeclKind {
    Impl,
    Record,
    Component,
}

struct Checker<'a> {
    sources: &'a SourceMap,
    file: FileId,
    decls: HashMap<String, Option<Span>>,
    frames: Vec<HashMap<String, Binder>>,
    boundary: usize,
    def_impls: Vec<HashMap<String, Span>>,
    pub_seen: Option<Span>,
}

impl<'a> Checker<'a> {
    fn new(sources: &'a SourceMap, file: FileId) -> Self {
        let mut prelude = HashMap::new();
        for name in PRELUDE_VALUES {
            prelude.insert(
                (*name).to_string(),
                Binder {
                    origin: Origin::Prelude,
                    boundary: 0,
                    span: Span::new(0, 0),
                },
            );
        }
        let mut decls = HashMap::new();
        for name in PRELUDE_DECLS {
            decls.insert((*name).to_string(), None);
        }
        Self {
            sources,
            file,
            decls,
            frames: vec![prelude],
            boundary: 0,
            def_impls: vec![HashMap::new()],
            pub_seen: None,
        }
    }

    fn check(
        &mut self,
        module: &ResolvedModule,
        export_of: &HashMap<FileId, Option<Name>>,
    ) -> Result<(), Diagnostic> {
        self.collect_decls(&module.module)?;
        self.collect_top_level(module, export_of)?;
        self.resolve_bodies(&module.module)
    }

    fn collect_decls(&mut self, module: &Module) -> Result<(), Diagnostic> {
        for item in &module.items {
            if let Item::Decl(decl) = item {
                if let Some(existing) = self.decls.get(&decl.name.text) {
                    let message = match existing {
                        None => format!("型 `{}` は prelude で定義済みです", decl.name.text),
                        Some(span) => format!(
                            "型 `{}` はすでに{}行目で宣言されています",
                            decl.name.text,
                            self.line_of(*span)
                        ),
                    };
                    return Err(Diagnostic::error(message)
                        .at(self.file, decl.name.span)
                        .with_label("同じ名前の定義を2つ持つことはできません"));
                }
                self.decls
                    .insert(decl.name.text.clone(), Some(decl.name.span));
            }
        }
        Ok(())
    }

    fn collect_top_level(
        &mut self,
        module: &ResolvedModule,
        export_of: &HashMap<FileId, Option<Name>>,
    ) -> Result<(), Diagnostic> {
        self.frames.push(HashMap::new());
        for import in &module.imports {
            let export = export_of.get(&import.file).cloned().flatten();
            for name in &import.names {
                match &export {
                    None => {
                        return Err(Diagnostic::error(format!(
                            "`{}` は何も export していません",
                            import.path_text
                        ))
                        .at(self.file, name.span)
                        .with_label("import できるのは相手ファイルの `:pub` 定義だけです"));
                    }
                    Some(export) if export.text != name.text => {
                        return Err(Diagnostic::error(format!(
                            "`{}` は `{}` の export ではありません",
                            name.text, import.path_text
                        ))
                        .at(self.file, name.span)
                        .with_label(format!(
                            "`{}` の export は `{}` です",
                            import.path_text, export.text
                        )));
                    }
                    Some(_) => {}
                }
                self.declare(name, Origin::Import)?;
            }
        }
        self.frames.push(HashMap::new());
        for item in &module.module.items {
            if let Item::Binding(binding) = item {
                if binding.is_pub {
                    if let Some(first) = self.pub_seen {
                        return Err(Diagnostic::error("`:pub` は1ファイルに1つまでです")
                            .at(self.file, binding_name(binding).span)
                            .with_label(format!(
                                "最初の公開定義は{}行目です",
                                self.line_of(first)
                            )));
                    }
                    self.pub_seen = Some(binding_name(binding).span);
                }
                let name = binding_name(binding);
                self.declare(name, Origin::TopLevel)?;
                if let BindingKind::Impl { def, .. } = &binding.kind {
                    self.resolve_decl_ref(def, DeclKind::Impl)?;
                    self.declare_impl(def)?;
                }
            }
        }
        Ok(())
    }

    fn resolve_bodies(&mut self, module: &Module) -> Result<(), Diagnostic> {
        for item in &module.items {
            match item {
                Item::Binding(binding) => match &binding.kind {
                    BindingKind::Value { value, .. } => self.resolve_expr(value)?,
                    BindingKind::Impl { params, body, .. } => self.resolve_fn(params, body)?,
                },
                Item::Expr(expr) => self.resolve_expr(expr)?,
                Item::Decl(_) | Item::Import(_) => {}
            }
        }
        Ok(())
    }

    fn resolve_fn(&mut self, params: &[Name], body: &Expr) -> Result<(), Diagnostic> {
        self.boundary += 1;
        self.frames.push(HashMap::new());
        let mut result = Ok(());
        for param in params {
            if let Err(err) = self.declare(param, Origin::Local) {
                result = Err(err);
                break;
            }
        }
        if result.is_ok() {
            result = self.resolve_expr(body);
        }
        self.frames.pop();
        self.boundary -= 1;
        result
    }

    fn resolve_lambda(&mut self, param: &Name, body: &Expr) -> Result<(), Diagnostic> {
        self.boundary += 1;
        self.frames.push(HashMap::new());
        let mut result = self.declare(param, Origin::Local);
        if result.is_ok() {
            result = self.resolve_expr(body);
        }
        self.frames.pop();
        self.boundary -= 1;
        result
    }

    fn resolve_block(&mut self, bindings: &[Binding], result: &Expr) -> Result<(), Diagnostic> {
        self.frames.push(HashMap::new());
        self.def_impls.push(HashMap::new());
        let outcome = self.resolve_block_inner(bindings, result);
        self.def_impls.pop();
        self.frames.pop();
        outcome
    }

    fn resolve_block_inner(
        &mut self,
        bindings: &[Binding],
        result: &Expr,
    ) -> Result<(), Diagnostic> {
        for binding in bindings {
            match &binding.kind {
                BindingKind::Value { name, value, .. } => {
                    self.resolve_expr(value)?;
                    self.declare(name, Origin::Local)?;
                }
                BindingKind::Impl {
                    name,
                    def,
                    params,
                    body,
                } => {
                    self.resolve_decl_ref(def, DeclKind::Impl)?;
                    self.declare_impl(def)?;
                    self.resolve_fn(params, body)?;
                    self.declare(name, Origin::Local)?;
                }
            }
        }
        self.resolve_expr(result)
    }

    fn resolve_expr(&mut self, expr: &Expr) -> Result<(), Diagnostic> {
        match expr {
            Expr::Int { .. } | Expr::Float { .. } | Expr::Bang { .. } => Ok(()),
            Expr::Str { parts, .. } => self.resolve_parts(parts),
            Expr::Var { name, .. } => self.resolve_var(name),
            Expr::Field { base, .. } => self.resolve_expr(base),
            Expr::App { func, args, .. } => {
                self.resolve_expr(func)?;
                for arg in args {
                    self.resolve_expr(arg)?;
                }
                Ok(())
            }
            Expr::Lambda { param, body, .. } => self.resolve_lambda(param, body),
            Expr::Pipe { lhs, rhs, .. } => {
                self.resolve_expr(lhs)?;
                self.resolve_expr(rhs)
            }
            Expr::Record { name, fields, .. } => {
                self.resolve_decl_ref(name, DeclKind::Record)?;
                for field in fields {
                    self.resolve_expr(&field.value)?;
                }
                Ok(())
            }
            Expr::Vec { elems, .. } => {
                for elem in elems {
                    match elem {
                        VecElem::Expr(expr) => self.resolve_expr(expr)?,
                        VecElem::Record { fields, .. } => {
                            for field in fields {
                                self.resolve_expr(&field.value)?;
                            }
                        }
                    }
                }
                Ok(())
            }
            Expr::Block {
                bindings, result, ..
            } => self.resolve_block(bindings, result),
            Expr::Element(element) => self.resolve_element(element),
        }
    }

    fn resolve_element(&mut self, element: &Element) -> Result<(), Diagnostic> {
        for attr in &element.attrs {
            if let Some(value) = &attr.value {
                self.resolve_expr(value)?;
            }
        }
        for child in &element.children {
            self.resolve_child(child)?;
        }
        Ok(())
    }

    fn resolve_child(&mut self, child: &Child) -> Result<(), Diagnostic> {
        match child {
            Child::Element(element) => self.resolve_element(element),
            Child::Component(component) => self.resolve_component(component),
            Child::Text { parts, .. } => self.resolve_parts(parts),
        }
    }

    fn resolve_component(&mut self, component: &AstComponent) -> Result<(), Diagnostic> {
        self.resolve_decl_ref(&component.name, DeclKind::Component)?;
        for arg in &component.args {
            self.resolve_expr(arg)?;
        }
        for child in &component.children {
            self.resolve_child(child)?;
        }
        Ok(())
    }

    fn resolve_parts(&mut self, parts: &[StrPart]) -> Result<(), Diagnostic> {
        for part in parts {
            if let StrPart::Interp { expr, .. } = part {
                self.resolve_expr(expr)?;
            }
        }
        Ok(())
    }

    fn resolve_var(&self, name: &Name) -> Result<(), Diagnostic> {
        for frame in self.frames.iter().rev() {
            if let Some(binder) = frame.get(&name.text) {
                if binder.origin == Origin::Local && binder.boundary != self.boundary {
                    return Err(Diagnostic::error(format!(
                        "`{}` は外側の関数のローカル値です（{}行目で束縛）。この関数の内側からは参照できません",
                        name.text,
                        self.line_of(binder.span)
                    ))
                    .at(self.file, name.span)
                    .with_label("引数として明示的に受け取ってください（暗黙のキャプチャは禁止です）"));
                }
                return Ok(());
            }
        }
        Err(
            Diagnostic::error(format!("名前 `{}` は定義されていません", name.text))
                .at(self.file, name.span)
                .with_label("対応する束縛・import・prelude が見つかりません"),
        )
    }

    fn resolve_decl_ref(&self, name: &Name, kind: DeclKind) -> Result<(), Diagnostic> {
        if self.decls.contains_key(&name.text) {
            return Ok(());
        }
        let (message, label) = match kind {
            DeclKind::Impl => (
                format!("定義 `{}` が見つかりません", name.text),
                "対応する `#` 宣言が必要です",
            ),
            DeclKind::Record => (
                format!("型 `{}` は定義されていません", name.text),
                "`#` 宣言または prelude の型（Recipe / Document）が必要です",
            ),
            DeclKind::Component => (
                format!("コンポーネント `{}` は定義されていません", name.text),
                "対応する `#` 宣言が見つかりません",
            ),
        };
        Err(Diagnostic::error(message)
            .at(self.file, name.span)
            .with_label(label))
    }

    fn declare_impl(&mut self, def: &Name) -> Result<(), Diagnostic> {
        for scope in self.def_impls.iter().rev() {
            if let Some(first) = scope.get(&def.text) {
                return Err(Diagnostic::error(format!(
                    "定義 `{}` の実装が複数あります（1定義=1実装）",
                    def.text
                ))
                .at(self.file, def.span)
                .with_label(format!("最初の実装は{}行目です", self.line_of(*first))));
            }
        }
        self.def_impls
            .last_mut()
            .unwrap()
            .insert(def.text.clone(), def.span);
        Ok(())
    }

    fn declare(&mut self, name: &Name, origin: Origin) -> Result<(), Diagnostic> {
        if let Some(existing) = self.frames.last().unwrap().get(&name.text) {
            let existing = *existing;
            return Err(self.same_scope_error(name, origin, existing.span));
        }
        for frame in self.frames.iter().rev().skip(1) {
            if let Some(existing) = frame.get(&name.text) {
                let existing = *existing;
                return Err(self.shadow_error(name, existing.origin, existing.span));
            }
        }
        self.frames.last_mut().unwrap().insert(
            name.text.clone(),
            Binder {
                origin,
                boundary: self.boundary,
                span: name.span,
            },
        );
        Ok(())
    }

    fn same_scope_error(&self, name: &Name, origin: Origin, existing: Span) -> Diagnostic {
        match origin {
            Origin::Import => {
                Diagnostic::error(format!("`{}` を重複して import しています", name.text))
                    .at(self.file, name.span)
                    .with_label("同じ名前を2回 import することはできません")
            }
            _ => Diagnostic::error(format!(
                "`{}` はすでに{}行目で束縛されています",
                name.text,
                self.line_of(existing)
            ))
            .at(self.file, name.span)
            .with_label("再代入はできません（clum の束縛は不変です）"),
        }
    }

    fn shadow_error(&self, name: &Name, origin: Origin, existing: Span) -> Diagnostic {
        let what = match origin {
            Origin::Prelude => "prelude で定義済みの名前".to_string(),
            Origin::Import => "import 済みの名前".to_string(),
            Origin::TopLevel => {
                format!(
                    "トップレベル（{}行目）で束縛済みの名前",
                    self.line_of(existing)
                )
            }
            Origin::Local => {
                format!(
                    "外側のスコープ（{}行目）で束縛済みの名前",
                    self.line_of(existing)
                )
            }
        };
        Diagnostic::error(format!("`{}` は{}です", name.text, what))
            .at(self.file, name.span)
            .with_label("シャドーイングはできません。別の名前を使ってください")
    }

    fn line_of(&self, span: Span) -> usize {
        self.sources.get(self.file).line_col(span.start).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn resolve_single(src: &str) -> Result<(), Diagnostic> {
        let mut sources = SourceMap::new();
        let file = sources.add_file(PathBuf::from("main.clum"), src.to_string());
        let module = parse(src, file).expect("パースに成功する前提です");
        let export = find_export(&module);
        let resolved = ResolvedModule {
            file,
            module,
            export,
            imports: Vec::new(),
        };
        let export_of: HashMap<FileId, Option<Name>> =
            [(file, resolved.export.clone())].into_iter().collect();
        let mut checker = Checker::new(&sources, file);
        checker.check(&resolved, &export_of)
    }

    fn render(src: &str, diagnostic: &Diagnostic) -> String {
        let mut sources = SourceMap::new();
        sources.add_file(PathBuf::from("main.clum"), src.to_string());
        diagnostic.render(&sources)
    }

    fn error_of(src: &str) -> String {
        let err = resolve_single(src).expect_err("エラーを期待しました");
        render(src, &err)
    }

    struct TmpDir {
        path: PathBuf,
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn temp_dir(tag: &str) -> TmpDir {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            env::temp_dir().join(format!("clum-resolve-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        TmpDir { path }
    }

    fn resolve_files(dir: &Path, files: &[(&str, &str)]) -> Result<(), String> {
        for (name, content) in files {
            fs::write(dir.join(name), content).unwrap();
        }
        let mut sources = SourceMap::new();
        let entry_path = dir.join("main.clum");
        let content = fs::read_to_string(&entry_path).unwrap();
        let entry = sources.add_file(entry_path, content);
        match resolve_program(&mut sources, entry) {
            Ok(_) => Ok(()),
            Err(diagnostic) => Err(diagnostic.render(&sources)),
        }
    }

    #[test]
    fn top_level_reference_resolves() {
        assert!(resolve_single("title = 'home'\nx = title\n").is_ok());
    }

    #[test]
    fn lambda_reads_top_level_and_own_param() {
        let src = concat!(
            "pages =\n",
            "  - path: './a'\n",
            "    title: 'A'\n",
            "items = pages\n",
            "  |> map page ->\n",
            "    h .li\n",
            "      {page.title}\n",
        );
        assert!(resolve_single(src).is_ok());
    }

    #[test]
    fn component_definition_and_use_resolves() {
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
        assert!(resolve_single(src).is_ok());
    }

    #[test]
    fn undefined_name_is_error() {
        let message = error_of("x = foo\n");
        assert!(message.contains("名前 `foo` は定義されていません"));
        assert!(message.contains(":1:5"));
    }

    #[test]
    fn shadowing_prelude_is_error() {
        let message = error_of("map = 1\n");
        assert!(message.contains("prelude で定義済みの名前"));
        assert!(message.contains("シャドーイング"));
    }

    #[test]
    fn local_shadowing_top_level_is_error() {
        let src = concat!(
            "title = 'home'\n",
            "# Make x: String -> Html\n",
            "make: Make x ->\n",
            "  title = x\n",
            "  h .div\n",
            "    {title}\n",
        );
        let message = error_of(src);
        assert!(message.contains("トップレベル（1行目）で束縛済みの名前"));
        assert!(message.contains(":4:3"));
    }

    #[test]
    fn reassign_in_same_scope_is_error() {
        let message = error_of("x = 1\nx = 2\n");
        assert!(message.contains("`x` はすでに1行目で束縛されています"));
        assert!(message.contains(":2:1"));
    }

    #[test]
    fn implicit_capture_is_error() {
        let src = concat!(
            "# Make prefix: String, items: Vec<Html> -> Vec<Html>\n",
            "make: Make prefix, items ->\n",
            "  items\n",
            "    |> map item ->\n",
            "      h .li\n",
            "        {prefix}\n",
        );
        let message = error_of(src);
        assert!(message.contains("外側の関数のローカル値です（2行目で束縛）"));
        assert!(message.contains(":6:10"));
    }

    #[test]
    fn duplicate_impl_of_one_definition_is_error() {
        let src = concat!(
            "# Greet name: String -> String\n",
            "hi: Greet name -> name\n",
            "yo: Greet name -> name\n",
        );
        let message = error_of(src);
        assert!(message.contains("定義 `Greet` の実装が複数あります"));
        assert!(message.contains("最初の実装は2行目です"));
        assert!(message.contains(":3:5"));
    }

    #[test]
    fn block_impl_duplicating_top_level_impl_is_error() {
        let src = concat!(
            "# Greet name: String -> String\n",
            "hi: Greet name -> name\n",
            "xs =\n",
            "  - 1\n",
            "items = xs\n",
            "  |> map n ->\n",
            "    yo: Greet name -> name\n",
            "    h .li\n",
        );
        let message = error_of(src);
        assert!(message.contains("定義 `Greet` の実装が複数あります"));
        assert!(message.contains("最初の実装は2行目です"));
        assert!(message.contains(":7:9"));
    }

    #[test]
    fn duplicate_block_impls_in_same_block_is_error() {
        let src = concat!(
            "# Greet name: String -> String\n",
            "xs =\n",
            "  - 1\n",
            "items = xs\n",
            "  |> map n ->\n",
            "    hi: Greet name -> name\n",
            "    yo: Greet name -> name\n",
            "    h .li\n",
        );
        let message = error_of(src);
        assert!(message.contains("定義 `Greet` の実装が複数あります"));
        assert!(message.contains("最初の実装は6行目です"));
        assert!(message.contains(":7:9"));
    }

    #[test]
    fn sibling_block_impls_of_same_definition_resolve() {
        let src = concat!(
            "# Greet name: String -> String\n",
            "xs =\n",
            "  - 1\n",
            "a = xs\n",
            "  |> map n ->\n",
            "    hi: Greet name -> name\n",
            "    h .li\n",
            "b = xs\n",
            "  |> map n ->\n",
            "    yo: Greet name -> name\n",
            "    h .li\n",
        );
        assert!(resolve_single(src).is_ok());
    }

    #[test]
    fn multiple_pub_is_error() {
        let src = concat!(
            ":pub\n",
            "a: Html = h .div\n",
            ":pub\n",
            "b: Html = h .span\n",
        );
        let message = error_of(src);
        assert!(message.contains("`:pub` は1ファイルに1つまでです"));
        assert!(message.contains("最初の公開定義は2行目です"));
        assert!(message.contains(":4:1"));
    }

    #[test]
    fn unknown_component_is_error() {
        let src = concat!(":pub\n", "page: Html = h .body\n", "  Card 'x'\n",);
        let message = error_of(src);
        assert!(message.contains("コンポーネント `Card` は定義されていません"));
    }

    #[test]
    fn unknown_record_type_is_error() {
        let message = error_of("x = Foo\n  a: 1\n");
        assert!(message.contains("型 `Foo` は定義されていません"));
    }

    #[test]
    fn duplicate_decl_is_error() {
        let message = error_of("# Foo a: i32\n# Foo b: i32\n");
        assert!(message.contains("型 `Foo` はすでに1行目で宣言されています"));
    }

    #[test]
    fn import_resolves_ok() {
        let dir = temp_dir("import-ok");
        let result = resolve_files(
            &dir.path,
            &[
                (
                    "main.clum",
                    "@ ./index\n  index\n\nRecipe\n  documents:\n    - path: './dist/index.html'\n      element: index\n  |> build\n  |> !\n",
                ),
                ("index.clum", ":pub\nindex: Html = h .div\n"),
            ],
        );
        assert!(result.is_ok(), "{:?}", result.err());
    }

    #[test]
    fn import_of_non_export_is_error() {
        let dir = temp_dir("import-notexport");
        let message = resolve_files(
            &dir.path,
            &[
                ("main.clum", "@ ./index\n  wrong\n\nx = wrong\n"),
                ("index.clum", ":pub\nindex: Html = h .div\n"),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("`wrong` は `./index` の export ではありません"));
    }

    #[test]
    fn import_missing_file_is_error() {
        let dir = temp_dir("import-missing");
        let message = resolve_files(&dir.path, &[("main.clum", "@ ./missing\n  x\n\ny = 1\n")])
            .expect_err("エラーを期待しました");
        assert!(message.contains("が見つかりません"));
    }

    #[test]
    fn import_cycle_is_error() {
        let dir = temp_dir("import-cycle");
        let message = resolve_files(
            &dir.path,
            &[
                ("main.clum", "@ ./a\n  a\n\nx = a\n"),
                ("a.clum", "@ ./b\n  b\n\n:pub\na: Html = h .div\n  {b}\n"),
                ("b.clum", "@ ./a\n  a\n\n:pub\nb: Html = h .div\n  {a}\n"),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("循環 import を検出しました"));
    }
}
