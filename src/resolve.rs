use std::collections::{HashMap, HashSet};
use std::fs;
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

pub const WINDOW_FILE: &str = "_.clum";

#[derive(Debug)]
pub struct Program {
    pub entry: FileId,
    pub modules: Vec<ResolvedModule>,
}

#[derive(Debug)]
pub struct ResolvedModule {
    pub file: FileId,
    pub module: Module,
    pub is_window: bool,
    pub is_entry: bool,
    pub imports: Vec<ModuleImport>,
}

#[derive(Debug, Clone)]
pub struct ModuleImport {
    pub path_text: String,
    pub span: Span,
    pub names: Vec<ImportedName>,
}

#[derive(Debug, Clone)]
pub struct ImportedName {
    pub name: Name,
    pub origin: FileId,
    pub kind: ImportedKind,
}

#[derive(Debug, Clone)]
pub enum ImportedKind {
    Value {
        source: String,
    },
    Decl {
        source_decl: String,
        source_impl: Option<String>,
    },
}

pub fn kebab_of(upper: &str) -> String {
    let mut out = String::new();
    for (index, c) in upper.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if index > 0 {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[derive(Debug, Clone)]
struct NsEntry {
    origin: FileId,
    kind: ImportedKind,
}

pub fn resolve_program(sources: &mut SourceMap, entry_path: &Path) -> Result<Program, Diagnostic> {
    let canon = fs::canonicalize(entry_path).map_err(|err| {
        Diagnostic::error(format!(
            "エントリファイル `{}` を開けません: {err}",
            entry_path.display()
        ))
    })?;
    if canon.is_dir() {
        return Err(Diagnostic::error(format!(
            "`{}` はディレクトリです。ディレクトリ指定は廃止されました",
            entry_path.display()
        ))
        .with_label(
            "`clum build <エントリファイル>` の形で指定します（例: `clum build ./dev.clum`）",
        ));
    }
    let file_name = canon
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !file_name.ends_with(".clum") {
        return Err(Diagnostic::error(format!(
            "`{}` は `.clum` ファイルではありません",
            entry_path.display()
        ))
        .with_label("エントリには `.clum` ファイルを指定します"));
    }
    if file_name == WINDOW_FILE {
        return Err(Diagnostic::error(format!(
            "窓口 `{WINDOW_FILE}` はエントリに指定できません"
        ))
        .with_label("窓口は公開面専用です。実行するプログラムは自由な名前のエントリファイルに書きます"));
    }
    let Some(dir_canon) = canon.parent().map(Path::to_path_buf) else {
        return Err(Diagnostic::error(format!(
            "エントリファイル `{}` の親ディレクトリが取れません",
            entry_path.display()
        )));
    };
    let dir_display = entry_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(clean_display)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut loader = Loader {
        sources,
        dir_ids: HashMap::new(),
        dirs: Vec::new(),
        files: Vec::new(),
        by_file: HashMap::new(),
        pending: Vec::new(),
    };
    let entry_dir = loader.ensure_dir(dir_canon, dir_display, None)?;
    let entry_stem = file_name.trim_end_matches(".clum");
    let entry = loader
        .files
        .iter()
        .find(|entry| entry.dir == entry_dir && entry.stem == entry_stem)
        .map(|entry| entry.file)
        .expect("エントリファイルはディレクトリの走査に含まれるはずです");
    while let Some(index) = loader.pending.pop() {
        loader.resolve_imports(index)?;
    }
    for index in 0..loader.files.len() {
        loader.check_file(index)?;
    }
    let ordered = order_files(loader.sources, &loader.files, &loader.by_file)?;
    let mut entries: Vec<Option<FileEntry>> = loader.files.into_iter().map(Some).collect();
    let mut modules = Vec::with_capacity(ordered.len());
    for index in ordered {
        let entry_data = entries[index].take().expect("順序は一意のはずです");
        modules.push(ResolvedModule {
            file: entry_data.file,
            module: entry_data.module,
            is_window: entry_data.is_window,
            is_entry: entry_data.file == entry,
            imports: entry_data.imports,
        });
    }
    Ok(Program { entry, modules })
}

struct DirData {
    canon: PathBuf,
    display: PathBuf,
    window: Option<FileId>,
    face: HashMap<String, NsEntry>,
}

struct FileEntry {
    file: FileId,
    dir: usize,
    is_window: bool,
    has_exprs: bool,
    stem: String,
    module: Module,
    exports: HashMap<String, ImportedKind>,
    imports: Vec<ModuleImport>,
}

struct Loader<'a> {
    sources: &'a mut SourceMap,
    dir_ids: HashMap<PathBuf, usize>,
    dirs: Vec<DirData>,
    files: Vec<FileEntry>,
    by_file: HashMap<FileId, usize>,
    pending: Vec<usize>,
}

enum SiblingTarget {
    File(usize),
    Subdir(usize),
}

impl Loader<'_> {
    fn ensure_dir(
        &mut self,
        canon: PathBuf,
        display: PathBuf,
        blame: Option<(FileId, Span)>,
    ) -> Result<usize, Diagnostic> {
        if let Some(&id) = self.dir_ids.get(&canon) {
            return Ok(id);
        }
        let id = self.dirs.len();
        self.dir_ids.insert(canon.clone(), id);
        self.dirs.push(DirData {
            canon: canon.clone(),
            display: display.clone(),
            window: None,
            face: HashMap::new(),
        });

        let mut names = Vec::new();
        let entries = fs::read_dir(&canon).map_err(|err| {
            io_error(
                format!("ディレクトリ `{}` を読めません: {err}", display.display()),
                blame,
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|err| {
                io_error(
                    format!("ディレクトリ `{}` を読めません: {err}", display.display()),
                    blame,
                )
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".clum") && entry.path().is_file() {
                names.push(name);
            }
        }
        names.sort();

        let mut file_indexes = Vec::new();
        for name in names {
            let path = canon.join(&name);
            let content = fs::read_to_string(&path).map_err(|err| {
                io_error(
                    format!(
                        "ファイル `{}` を読めません: {err}",
                        display.join(&name).display()
                    ),
                    blame,
                )
            })?;
            let file = self.sources.add_file(display.join(&name), content.clone());
            let module = parse(&content, file)?;
            let is_window = name == WINDOW_FILE;
            if is_window {
                check_window_contents(file, &module)?;
                self.dirs[id].window = Some(file);
            }
            let has_exprs = module
                .items
                .iter()
                .any(|item| matches!(item, Item::Expr(_)));
            let stem = name.trim_end_matches(".clum").to_string();
            let index = self.files.len();
            self.by_file.insert(file, index);
            self.files.push(FileEntry {
                file,
                dir: id,
                is_window,
                has_exprs,
                stem,
                module,
                exports: HashMap::new(),
                imports: Vec::new(),
            });
            file_indexes.push(index);
            self.pending.push(index);
        }

        for &index in &file_indexes {
            self.collect_file_exports(index)?;
        }
        self.build_face(id)?;
        Ok(id)
    }

    fn collect_file_exports(&mut self, index: usize) -> Result<(), Diagnostic> {
        let file = self.files[index].file;
        let mut exports = HashMap::new();
        {
            let module = &self.files[index].module;
            for item in &module.items {
                match item {
                    Item::Binding(binding) if binding.exposed => match &binding.kind {
                        BindingKind::Impl { name, def, .. } => {
                            return Err(Diagnostic::error(format!(
                                "実装 `{}` に `^` は付けません",
                                name.text
                            ))
                            .at(file, name.span)
                            .with_label(format!(
                                "`^# {}` と宣言に前置すれば、実装は双方向随伴で付いてきます",
                                def.text
                            )));
                        }
                        BindingKind::Value { name, ty, .. } => {
                            if ty.is_none() {
                                return Err(Diagnostic::error(format!(
                                    "公開する `{}` には型注釈が必要です",
                                    name.text
                                ))
                                .at(file, name.span)
                                .with_label(
                                    "`^` された定義は境界です。`^名前: 型 = 式` の形で束縛してください",
                                ));
                            }
                            exports.insert(
                                name.text.clone(),
                                ImportedKind::Value {
                                    source: name.text.clone(),
                                },
                            );
                        }
                    },
                    Item::Decl(decl) if decl.exposed => {
                        let impl_name = module.items.iter().find_map(|item| match item {
                            Item::Binding(Binding {
                                kind:
                                    BindingKind::Impl {
                                        name: impl_name,
                                        def,
                                        ..
                                    },
                                ..
                            }) if def.text == decl.name.text => Some(impl_name.text.clone()),
                            _ => None,
                        });
                        exports.insert(
                            decl.name.text.clone(),
                            ImportedKind::Decl {
                                source_decl: decl.name.text.clone(),
                                source_impl: impl_name,
                            },
                        );
                    }
                    _ => {}
                }
            }
        }
        self.files[index].exports = exports;
        Ok(())
    }

    fn build_face(&mut self, dir: usize) -> Result<(), Diagnostic> {
        let Some(window) = self.dirs[dir].window else {
            return Ok(());
        };
        let window_index = self.by_file[&window];
        let reexports: Vec<Import> = self.files[window_index]
            .module
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Import(import) if import.reexport => Some(import.clone()),
                _ => None,
            })
            .collect();
        let mut sources_seen: HashSet<(FileId, String)> = HashSet::new();
        for import in reexports {
            let target = self.sibling_target(window, dir, &import, true)?;
            for entry in &import.names {
                let ns_entry = match &target {
                    SiblingTarget::File(file_index) => {
                        if self.files[*file_index].has_exprs {
                            return Err(Diagnostic::error(format!(
                                "`{}` はプログラムファイルです（トップレベルに式があります）。差し出せません",
                                import.path.text
                            ))
                            .at(window, import.path.span)
                            .with_label("プログラムファイルは `clum build` で実行する対象です"));
                        }
                        let origin = self.files[*file_index].file;
                        let Some(kind) = self.files[*file_index].exports.get(&entry.source.text)
                        else {
                            return Err(Diagnostic::error(format!(
                                "`{}` は `{}` の公開名ではありません",
                                entry.source.text, import.path.text
                            ))
                            .at(window, entry.source.span)
                            .with_label("そのファイルが `^` を前置した定義だけを差し出せます"));
                        };
                        NsEntry {
                            origin,
                            kind: kind.clone(),
                        }
                    }
                    SiblingTarget::Subdir(sub) => {
                        let Some(ns_entry) = self.dirs[*sub].face.get(&entry.source.text) else {
                            return Err(Diagnostic::error(format!(
                                "`{}` は `{}` の公開名ではありません",
                                entry.source.text, import.path.text
                            ))
                            .at(window, entry.source.span)
                            .with_label(
                                "そのサブディレクトリの窓口が `^@` で差し出した名前だけを中継できます",
                            ));
                        };
                        ns_entry.clone()
                    }
                };
                let source_key = match &ns_entry.kind {
                    ImportedKind::Value { source } => source.clone(),
                    ImportedKind::Decl { source_decl, .. } => source_decl.clone(),
                };
                if !sources_seen.insert((ns_entry.origin, source_key)) {
                    return Err(Diagnostic::error(format!(
                        "`{}` はすでに別の名前で差し出されています",
                        entry.source.text
                    ))
                    .at(window, entry.source.span)
                    .with_label("同じ元を2つの名前では差し出せません（差し出し名は一意です）"));
                }
                if self.dirs[dir].face.contains_key(&entry.name.text) {
                    return Err(Diagnostic::error(format!(
                        "`{}` を重複して差し出しています",
                        entry.name.text
                    ))
                    .at(window, entry.name.span)
                    .with_label("同じ名前は一度しか差し出せません"));
                }
                self.dirs[dir]
                    .face
                    .insert(entry.name.text.clone(), ns_entry);
            }
        }
        Ok(())
    }

    fn sibling_target(
        &mut self,
        file: FileId,
        dir: usize,
        import: &Import,
        reexport: bool,
    ) -> Result<SiblingTarget, Diagnostic> {
        let text = import.path.text.as_str();
        let (symbol, forms) = if reexport {
            ("^@", "`^@./ファイル名` か `^@./サブディレクトリ`")
        } else {
            ("@", "`@./ファイル名` か `@./サブディレクトリ`")
        };
        let Some(name) = text.strip_prefix("./") else {
            return Err(
                Diagnostic::error(format!("`{symbol}` のパス `{text}` が不正です"))
                    .at(file, import.path.span)
                    .with_label(format!("出どころを明示します（{forms}）")),
            );
        };
        if !reexport
            && name.contains('/')
            && name
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
        {
            return Err(
                Diagnostic::error(format!("複数段の下り import（`{text}`）は未決定です"))
                    .at(file, import.path.span)
                    .with_label("コア仕様の要決定31です。1段ずつ import してください"),
            );
        }
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return Err(
                Diagnostic::error(format!("`{symbol}` のパス `{text}` が不正です"))
                    .at(file, import.path.span)
                    .with_label(format!("出どころを明示します（{forms}）")),
            );
        }
        if name == "_" {
            return Err(Diagnostic::error("窓口 `_.clum` は import で指せません")
                .at(file, import.path.span)
                .with_label(
                    "窓口はディレクトリを代弁するファイルであり、定義の置き場所ではありません",
                ));
        }
        let canon = self.dirs[dir].canon.clone();
        let display = self.dirs[dir].display.clone();
        let file_index = self
            .files
            .iter()
            .position(|entry| entry.dir == dir && entry.stem == name && !entry.is_window);
        let subdir_exists = canon.join(name).is_dir();
        match (file_index, subdir_exists) {
            (Some(_), true) => Err(Diagnostic::error(format!(
                "`{text}` はファイル `{name}.clum` とサブディレクトリ `{name}/` の両方に一致します"
            ))
            .at(file, import.path.span)
            .with_label("どちらかを改名して曖昧さを解消してください")),
            (Some(index), false) => Ok(SiblingTarget::File(index)),
            (None, true) => {
                let sub_canon = fs::canonicalize(canon.join(name)).map_err(|err| {
                    Diagnostic::error(format!("ディレクトリ `{text}` を開けません: {err}"))
                        .at(file, import.path.span)
                })?;
                let sub = self.ensure_dir(
                    sub_canon,
                    display.join(name),
                    Some((file, import.path.span)),
                )?;
                if self.dirs[sub].window.is_none() {
                    return Err(Diagnostic::error(format!(
                        "`{text}` は窓口 `{WINDOW_FILE}` を持たないため公開名がありません"
                    ))
                    .at(file, import.path.span)
                    .with_label(
                        "ディレクトリの外へ差し出す名前は、窓口の `^@` ブロックに列挙します",
                    ));
                }
                Ok(SiblingTarget::Subdir(sub))
            }
            (None, false) => Err(Diagnostic::error(format!(
                "`{text}` に一致するファイル・サブディレクトリが見つかりません"
            ))
            .at(file, import.path.span)),
        }
    }

    fn resolve_imports(&mut self, index: usize) -> Result<(), Diagnostic> {
        let file = self.files[index].file;
        let dir = self.files[index].dir;
        let is_window = self.files[index].is_window;
        let imports: Vec<Import> = self.files[index]
            .module
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Import(import) => Some(import.clone()),
                _ => None,
            })
            .collect();
        let mut resolved = Vec::new();
        for import in imports {
            if import.reexport && !is_window {
                return Err(Diagnostic::error("`^@` を書けるのは窓口 `_.clum` だけです")
                    .at(file, import.span)
                    .with_label(
                        "親へ差し出すのは窓口の仕事です。ファイルの定義は `^` の前置で公開します",
                    ));
            }
            if import.path.text == "." {
                return Err(Diagnostic::error("`@.` は廃止されました")
                    .at(file, import.path.span)
                    .with_label(
                        "兄弟ファイルは `@./ファイル名` で、出どころを明示して import します",
                    ));
            }
            let names = if import.path.text.starts_with("./") {
                let target = self.sibling_target(file, dir, &import, import.reexport)?;
                match target {
                    SiblingTarget::File(file_index) => {
                        self.bind_file_names(file, file_index, &import)?
                    }
                    SiblingTarget::Subdir(sub) => self.bind_face_names(file, sub, &import)?,
                }
            } else {
                let target = parse_import_path(&import.path.text).map_err(|(message, label)| {
                    Diagnostic::error(message)
                        .at(file, import.path.span)
                        .with_label(label)
                })?;
                let ImportTarget::Walk { ups, down } = target;
                let target_dir = self.walk_to_dir(file, dir, &import, ups, down.as_deref())?;
                self.bind_face_names(file, target_dir, &import)?
            };
            resolved.push(ModuleImport {
                path_text: import.path.text.clone(),
                span: import.span,
                names,
            });
        }
        self.files[index].imports = resolved;
        Ok(())
    }

    fn walk_to_dir(
        &mut self,
        file: FileId,
        dir: usize,
        import: &Import,
        ups: usize,
        down: Option<&str>,
    ) -> Result<usize, Diagnostic> {
        let mut canon = self.dirs[dir].canon.clone();
        let mut display = self.dirs[dir].display.clone();
        for _ in 0..ups {
            let Some(parent) = canon.parent() else {
                return Err(Diagnostic::error("これ以上、上のディレクトリはありません")
                    .at(file, import.path.span));
            };
            canon = parent.to_path_buf();
            display = display_parent(&display);
        }
        if let Some(down) = down {
            let target = canon.join(down);
            if !target.is_dir() {
                if canon.join(format!("{down}.clum")).is_file() {
                    return Err(Diagnostic::error(
                        "フォルダの外のファイルは import できません",
                    )
                    .at(file, import.path.span)
                    .with_label(
                        "ファイルの住所は同じフォルダの中でだけ通用します。よそのディレクトリからは窓口の公開名を import します",
                    ));
                }
                return Err(Diagnostic::error(format!(
                    "ディレクトリ `{}` が見つかりません",
                    import.path.text
                ))
                .at(file, import.path.span));
            }
            canon = target;
            display = display.join(down);
        }
        let canon = fs::canonicalize(&canon).map_err(|err| {
            Diagnostic::error(format!(
                "ディレクトリ `{}` を開けません: {err}",
                import.path.text
            ))
            .at(file, import.path.span)
        })?;
        let id = self.ensure_dir(canon, display, Some((file, import.path.span)))?;
        if self.dirs[id].window.is_none() {
            return Err(Diagnostic::error(format!(
                "`{}` は窓口 `{WINDOW_FILE}` を持たないため import できません",
                import.path.text
            ))
            .at(file, import.path.span)
            .with_label("ディレクトリの外へ差し出す名前は、窓口の `^@` ブロックに列挙します"));
        }
        Ok(id)
    }

    fn bind_file_names(
        &self,
        file: FileId,
        target_index: usize,
        import: &Import,
    ) -> Result<Vec<ImportedName>, Diagnostic> {
        if self.files[target_index].has_exprs {
            return Err(Diagnostic::error(format!(
                "`{}` はプログラムファイルです（トップレベルに式があります）。import できません",
                import.path.text
            ))
            .at(file, import.path.span)
            .with_label("プログラムファイルは `clum build` で実行する対象です"));
        }
        let origin = self.files[target_index].file;
        let mut names = Vec::new();
        for entry in &import.names {
            let Some(kind) = self.files[target_index].exports.get(&entry.source.text) else {
                return Err(Diagnostic::error(format!(
                    "`{}` は `{}` の公開名ではありません",
                    entry.source.text, import.path.text
                ))
                .at(file, entry.source.span)
                .with_label("そのファイルが `^` を前置した定義だけを import できます"));
            };
            names.push(ImportedName {
                name: entry.name.clone(),
                origin,
                kind: kind.clone(),
            });
        }
        Ok(names)
    }

    fn bind_face_names(
        &self,
        file: FileId,
        dir: usize,
        import: &Import,
    ) -> Result<Vec<ImportedName>, Diagnostic> {
        let mut names = Vec::new();
        for entry in &import.names {
            let Some(ns_entry) = self.dirs[dir].face.get(&entry.source.text) else {
                return Err(Diagnostic::error(format!(
                    "`{}` は `{}` の公開名ではありません",
                    entry.source.text, import.path.text
                ))
                .at(file, entry.source.span)
                .with_label(
                    "import できるのは、そのディレクトリの窓口が `^@` で差し出した名前だけです",
                ));
            };
            names.push(ImportedName {
                name: entry.name.clone(),
                origin: ns_entry.origin,
                kind: ns_entry.kind.clone(),
            });
        }
        Ok(names)
    }

    fn check_file(&mut self, index: usize) -> Result<(), Diagnostic> {
        let entry = &self.files[index];
        let mut checker = Checker::new(self.sources, entry.file);
        checker.check(&entry.module, &entry.imports)
    }
}

fn io_error(message: String, blame: Option<(FileId, Span)>) -> Diagnostic {
    let diagnostic = Diagnostic::error(message);
    match blame {
        Some((file, span)) => diagnostic.at(file, span),
        None => diagnostic,
    }
}

enum ImportTarget {
    Walk { ups: usize, down: Option<String> },
}

fn parse_import_path(text: &str) -> Result<ImportTarget, (String, String)> {
    let segments: Vec<&str> = text.split('/').collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err((
            format!("import パス `{text}` の形が不正です"),
            "`@./名前`・`@..`（連続可）・`@../名前` の形で書きます".to_string(),
        ));
    }
    let (ups, rest) = match segments[0] {
        ".." => {
            let count = segments.iter().take_while(|s| **s == "..").count();
            (count, &segments[count..])
        }
        _ => {
            return Err((
                format!("import パス `{text}` は `.` か `..` で始まる必要があります"),
                "import の対象は相対パスで指す兄弟ファイルかディレクトリだけです".to_string(),
            ));
        }
    };
    if rest.iter().any(|s| *s == "." || *s == "..") {
        return Err((
            format!("import パス `{text}` の形が不正です"),
            "上り（`..`）はパスの先頭にまとめて書きます".to_string(),
        ));
    }
    if rest.iter().any(|s| s.ends_with(".clum")) {
        return Err((
            "フォルダの外のファイルは import できません".to_string(),
            "ファイルの住所は同じフォルダの中でだけ通用します。よそのディレクトリからは窓口の公開名を import します".to_string(),
        ));
    }
    match rest.len() {
        0 => Ok(ImportTarget::Walk { ups, down: None }),
        1 => Ok(ImportTarget::Walk {
            ups,
            down: Some(rest[0].to_string()),
        }),
        _ => Err((
            format!("複数段の下り import（`{text}`）は未決定です"),
            "コア仕様の要決定31です。1段ずつ import してください".to_string(),
        )),
    }
}

fn clean_display(path: &Path) -> PathBuf {
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

fn display_parent(display: &Path) -> PathBuf {
    match display.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => display.join(".."),
    }
}

fn order_files(
    sources: &SourceMap,
    files: &[FileEntry],
    by_file: &HashMap<FileId, usize>,
) -> Result<Vec<usize>, Diagnostic> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Unvisited,
        OnStack,
        Done,
    }
    fn visit(
        index: usize,
        sources: &SourceMap,
        files: &[FileEntry],
        by_file: &HashMap<FileId, usize>,
        states: &mut Vec<State>,
        stack: &mut Vec<usize>,
        ordered: &mut Vec<usize>,
    ) -> Result<(), Diagnostic> {
        states[index] = State::OnStack;
        stack.push(index);
        for import in &files[index].imports {
            for imported in &import.names {
                let target = by_file[&imported.origin];
                match states[target] {
                    State::Done => {}
                    State::Unvisited => {
                        visit(target, sources, files, by_file, states, stack, ordered)?;
                    }
                    State::OnStack => {
                        let position = stack
                            .iter()
                            .position(|&i| i == target)
                            .expect("スタック上にあるはずです");
                        let mut chain: Vec<String> = stack[position..]
                            .iter()
                            .map(|&i| sources.get(files[i].file).path().display().to_string())
                            .collect();
                        chain.push(sources.get(files[target].file).path().display().to_string());
                        return Err(Diagnostic::error(format!(
                            "循環 import を検出しました: {}",
                            chain.join(" -> ")
                        ))
                        .at(files[index].file, imported.name.span));
                    }
                }
            }
        }
        stack.pop();
        states[index] = State::Done;
        ordered.push(index);
        Ok(())
    }

    let mut states = vec![State::Unvisited; files.len()];
    let mut ordered = Vec::with_capacity(files.len());
    let mut stack = Vec::new();
    for index in 0..files.len() {
        if states[index] == State::Unvisited {
            visit(
                index,
                sources,
                files,
                by_file,
                &mut states,
                &mut stack,
                &mut ordered,
            )?;
        }
    }
    Ok(ordered)
}

fn check_window_contents(file: FileId, module: &Module) -> Result<(), Diagnostic> {
    for item in &module.items {
        let span = match item {
            Item::Import(import) if import.reexport => continue,
            Item::Import(import) => import.span,
            Item::Decl(decl) => decl.span,
            Item::Binding(binding) => binding.span,
            Item::Expr(expr) => expr.span(),
        };
        return Err(Diagnostic::error(
            "窓口 `_.clum` に書けるのは `^@` ブロックだけです",
        )
        .at(file, span)
        .with_label(
            "窓口はディレクトリの公開面専用です。束縛・宣言・式はエントリファイルか通常のモジュールに書きます",
        ));
    }
    Ok(())
}

fn binding_name(binding: &Binding) -> &Name {
    match &binding.kind {
        BindingKind::Value { name, .. } => name,
        BindingKind::Impl { name, .. } => name,
    }
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
        }
    }

    fn check(&mut self, module: &Module, imports: &[ModuleImport]) -> Result<(), Diagnostic> {
        self.collect_decls(module)?;
        self.collect_top_level(module, imports)?;
        self.resolve_bodies(module)
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
        module: &Module,
        imports: &[ModuleImport],
    ) -> Result<(), Diagnostic> {
        self.frames.push(HashMap::new());
        for import in imports {
            for imported in &import.names {
                match &imported.kind {
                    ImportedKind::Value { .. } => {
                        self.declare(&imported.name, Origin::Import)?;
                    }
                    ImportedKind::Decl { source_impl, .. } => {
                        self.declare_imported_decl(&imported.name)?;
                        if source_impl.is_some() {
                            let synthesized =
                                Name::new(kebab_of(&imported.name.text), imported.name.span);
                            self.declare(&synthesized, Origin::Import)?;
                        }
                    }
                }
            }
        }
        self.frames.push(HashMap::new());
        for item in &module.items {
            if let Item::Binding(binding) = item {
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

    fn declare_imported_decl(&mut self, name: &Name) -> Result<(), Diagnostic> {
        if let Some(existing) = self.decls.get(&name.text) {
            let message = match existing {
                None => format!("型 `{}` は prelude で定義済みです", name.text),
                Some(span) => format!(
                    "import した `{}` は{}行目の宣言と重複しています",
                    name.text,
                    self.line_of(*span)
                ),
            };
            return Err(Diagnostic::error(message)
                .at(self.file, name.span)
                .with_label("同じ名前の定義を2つ持つことはできません"));
        }
        self.decls.insert(name.text.clone(), Some(name.span));
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

    fn resolve_var(&mut self, name: &Name) -> Result<(), Diagnostic> {
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

    fn resolve_decl_ref(&mut self, name: &Name, kind: DeclKind) -> Result<(), Diagnostic> {
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
        let path = env::temp_dir().join(format!(
            "clum-resolve-{tag}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        TmpDir { path }
    }

    fn resolve_files(dir: &Path, entry: &str, files: &[(&str, &str)]) -> Result<Program, String> {
        for (name, content) in files {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, content).unwrap();
        }
        let mut sources = SourceMap::new();
        match resolve_program(&mut sources, &dir.join(entry)) {
            Ok(program) => Ok(program),
            Err(diagnostic) => Err(diagnostic.render(&sources)),
        }
    }

    fn resolve_entry(src: &str) -> Result<Program, String> {
        let dir = temp_dir("single");
        resolve_files(&dir.path, "entry.clum", &[("entry.clum", src)])
    }

    fn error_of(src: &str) -> String {
        resolve_entry(src).expect_err("エラーを期待しました")
    }

    #[test]
    fn kebab_of_derives_lower_kebab() {
        assert_eq!(kebab_of("Card"), "card");
        assert_eq!(kebab_of("MyButton"), "my-button");
        assert_eq!(kebab_of("A"), "a");
    }

    #[test]
    fn top_level_reference_resolves() {
        assert!(resolve_entry("title = 'home'\nx = title\n").is_ok());
    }

    #[test]
    fn component_definition_and_use_resolves() {
        let src = concat!(
            "# Card title: String, body: Html -> Html\n",
            "card: Card title, body -> h .div\n",
            "  {title}\n",
            "page: Html = h .body\n",
            "  Card 'お知らせ'\n",
            "    h .p\n",
            "      本文\n",
        );
        assert!(resolve_entry(src).is_ok());
    }

    #[test]
    fn undefined_name_is_error() {
        let message = error_of("x = foo\n");
        assert!(message.contains("名前 `foo` は定義されていません"));
    }

    #[test]
    fn shadowing_prelude_is_error() {
        let message = error_of("map = 1\n");
        assert!(message.contains("prelude で定義済みの名前"));
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
    }

    #[test]
    fn sibling_file_import_resolves() {
        let dir = temp_dir("sibling-file");
        let result = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "@./index\n  index\n\nx = index\n"),
                ("index.clum", "^index: Html = h .div\n"),
            ],
        );
        assert!(result.is_ok(), "{:?}", result.err());
    }

    #[test]
    fn sibling_component_pair_is_importable() {
        let dir = temp_dir("sibling-pair");
        let result = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                (
                    "entry.clum",
                    "@./card\n  Card\n\npage: Html = h .body\n  Card 'x'\n    h .p\n      本文\nused = card\n",
                ),
                (
                    "card.clum",
                    "^# Card title: String, body: Html -> Html\ncard: Card title, body -> h .div\n  {title}\n",
                ),
            ],
        );
        assert!(result.is_ok(), "{:?}", result.err());
    }

    #[test]
    fn sibling_without_import_is_error() {
        let dir = temp_dir("sibling-implicit");
        let message = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "x = index\n"),
                ("index.clum", "^index: Html = h .div\n"),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("名前 `index` は定義されていません"));
    }

    #[test]
    fn unexposed_sibling_name_is_error() {
        let dir = temp_dir("sibling-private");
        let message = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "@./util\n  helper\n\nx = helper\n"),
                ("util.clum", "helper: i32 = 1\n"),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("`helper` は `./util` の公開名ではありません"));
    }

    #[test]
    fn same_caret_name_in_two_files_is_allowed() {
        let dir = temp_dir("same-name");
        let result = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                (
                    "entry.clum",
                    "@./a\n  index\n\n@./b\n  b-index = index\n\nx = index\ny = b-index\n",
                ),
                ("a.clum", "^index: Html = h .div\n"),
                ("b.clum", "^index: Html = h .span\n"),
            ],
        );
        assert!(result.is_ok(), "{:?}", result.err());
    }

    #[test]
    fn exposed_value_without_annotation_is_error() {
        let message = error_of("^index = h .div\n");
        assert!(message.contains("公開する `index` には型注釈が必要です"));
    }

    #[test]
    fn exposed_impl_is_error() {
        let message = error_of("# Card title: String -> Html\n^card: Card title -> h .div\n");
        assert!(message.contains("実装 `card` に `^` は付けません"));
        assert!(message.contains("^# Card"));
    }

    #[test]
    fn at_dot_is_abolished_error() {
        let message = error_of("@.\n  index\n\nx = 1\n");
        assert!(message.contains("`@.` は廃止されました"));
        assert!(message.contains("@./ファイル名"));
    }

    #[test]
    fn caret_at_dot_is_error() {
        let dir = temp_dir("caret-at-dot");
        let message = resolve_files(
            &dir.path,
            "entry.clum",
            &[("entry.clum", "x = 1\n"), ("_.clum", "^@.\n  index\n")],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("`^@` のパス `.` が不正です"));
        assert!(message.contains("出どころを明示します"));
    }

    #[test]
    fn reexport_outside_window_is_error() {
        let dir = temp_dir("reexport-outside");
        let message = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "x = 1\n"),
                ("page.clum", "^@./lib\n  x\n\n^index: Html = h .div\n"),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("`^@` を書けるのは窓口 `_.clum` だけです"));
    }

    #[test]
    fn window_content_is_error() {
        let dir = temp_dir("window-content");
        let message = resolve_files(
            &dir.path,
            "entry.clum",
            &[("entry.clum", "x = 1\n"), ("_.clum", "y = 2\n")],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("窓口 `_.clum` に書けるのは `^@` ブロックだけです"));
    }

    #[test]
    fn window_as_entry_is_error() {
        let dir = temp_dir("window-entry");
        let message = resolve_files(
            &dir.path,
            "_.clum",
            &[
                ("_.clum", "^@./page\n  index\n"),
                ("page.clum", "^index: Html = h .div\n"),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("窓口 `_.clum` はエントリに指定できません"));
    }

    #[test]
    fn program_file_import_is_error() {
        let dir = temp_dir("program-import");
        let message = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "@./other\n  x\n\ny = x\n"),
                (
                    "other.clum",
                    "^x: Html = h .div\n\nRecipe\n  documents:\n    - path: './a.html'\n      element: x\n  |> build\n  |> !\n",
                ),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("`./other` はプログラムファイルです"));
    }

    #[test]
    fn coexisting_program_files_are_allowed() {
        let dir = temp_dir("two-entries");
        let result = resolve_files(
            &dir.path,
            "dev.clum",
            &[
                (
                    "dev.clum",
                    "@./index\n  index\n\nRecipe\n  documents:\n    - path: './dev-dist/index.html'\n      element: index\n  |> build\n  |> !\n",
                ),
                (
                    "prd.clum",
                    "@./index\n  index\n\nRecipe\n  documents:\n    - path: './prd-dist/index.html'\n      element: index\n  |> build\n  |> !\n",
                ),
                ("index.clum", "^index: Html = h .div\n"),
            ],
        );
        assert!(result.is_ok(), "{:?}", result.err());
    }

    #[test]
    fn window_reexports_sibling_file() {
        let dir = temp_dir("face-file");
        let result = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "@./lib\n  page\n\nx = page\n"),
                ("lib/_.clum", "^@./page\n  page\n"),
                ("lib/page.clum", "^page: Html = h .div\n"),
            ],
        );
        assert!(result.is_ok(), "{:?}", result.err());
    }

    #[test]
    fn face_alias_renames_public_name() {
        let dir = temp_dir("face-alias");
        let result = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                (
                    "entry.clum",
                    "@./ui\n  Kado\n\npage: Html = h .body\n  Kado 'x'\n    h .p\n      本文\nused = kado\n",
                ),
                ("ui/_.clum", "^@./card\n  Kado = Card\n"),
                (
                    "ui/card.clum",
                    "^# Card title: String, body: Html -> Html\ncard: Card title, body -> h .div\n  {title}\n",
                ),
            ],
        );
        assert!(result.is_ok(), "{:?}", result.err());
    }

    #[test]
    fn import_alias_renames_local_name() {
        let dir = temp_dir("import-alias");
        let result = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "@./page\n  front = index\n\nx = front\n"),
                ("page.clum", "^index: Html = h .div\n"),
            ],
        );
        assert!(result.is_ok(), "{:?}", result.err());
    }

    #[test]
    fn subdir_relay_resolves() {
        let dir = temp_dir("relay");
        let result = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                (
                    "entry.clum",
                    "@./ui\n  Card\n\npage: Html = h .body\n  Card 'x'\n    h .p\n      本文\n",
                ),
                ("ui/_.clum", "^@./parts\n  Card\n"),
                ("ui/parts/_.clum", "^@./card\n  Card\n"),
                (
                    "ui/parts/card.clum",
                    "^# Card title: String, body: Html -> Html\ncard: Card title, body -> h .div\n  {title}\n",
                ),
            ],
        );
        assert!(result.is_ok(), "{:?}", result.err());
    }

    #[test]
    fn face_unknown_source_is_error() {
        let dir = temp_dir("face-unknown");
        let message = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "x = 1\n"),
                ("_.clum", "^@./page\n  missing\n"),
                ("page.clum", "^index: Html = h .div\n"),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("`missing` は `./page` の公開名ではありません"));
    }

    #[test]
    fn face_duplicate_public_name_is_error() {
        let dir = temp_dir("face-dup");
        let message = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "x = 1\n"),
                ("_.clum", "^@./pages\n  index\n  index = about\n"),
                (
                    "pages.clum",
                    "^index: Html = h .div\n^about: Html = h .span\n",
                ),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("`index` を重複して差し出しています"));
    }

    #[test]
    fn face_duplicate_source_is_error() {
        let dir = temp_dir("face-dup-source");
        let message = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "x = 1\n"),
                ("_.clum", "^@./pages\n  index\n  home = index\n"),
                ("pages.clum", "^index: Html = h .div\n"),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("すでに別の名前で差し出されています"));
    }

    #[test]
    fn import_of_unoffered_name_is_error() {
        let dir = temp_dir("unoffered");
        let message = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "@./lib\n  wrong\n\nx = wrong\n"),
                ("lib/_.clum", "^@./page\n  page\n"),
                ("lib/page.clum", "^page: Html = h .div\n"),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("`wrong` は `./lib` の公開名ではありません"));
    }

    #[test]
    fn import_of_missing_target_is_error() {
        let message = error_of("@./missing\n  x\n\ny = 1\n");
        assert!(
            message.contains("`./missing` に一致するファイル・サブディレクトリが見つかりません")
        );
    }

    #[test]
    fn import_of_windowless_dir_is_error() {
        let dir = temp_dir("windowless");
        let message = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "@./lib\n  x\n\ny = x\n"),
                ("lib/util.clum", "^x: Html = h .div\n"),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("`./lib` は窓口 `_.clum` を持たないため公開名がありません"));
    }

    #[test]
    fn multi_descent_import_is_error() {
        let message = error_of("@./a/b\n  x\n\ny = x\n");
        assert!(message.contains("複数段の下り import"));
        assert!(message.contains("要決定31"));
    }

    #[test]
    fn ambiguous_file_and_subdir_is_error() {
        let dir = temp_dir("ambiguous");
        let message = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "@./ui\n  x\n\ny = x\n"),
                ("ui.clum", "^x: Html = h .div\n"),
                ("ui/_.clum", "^@./inner\n  x\n"),
                ("ui/inner.clum", "^x: Html = h .div\n"),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("ファイル `ui.clum` とサブディレクトリ `ui/` の両方に一致します"));
    }

    #[test]
    fn window_file_is_not_importable() {
        let message = error_of("@./_\n  x\n\ny = 1\n");
        assert!(message.contains("窓口 `_.clum` は import で指せません"));
    }

    #[test]
    fn parent_file_import_is_error() {
        let dir = temp_dir("parent-file");
        let message = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "@./sub\n  x\n\ny = x\n"),
                ("page.clum", "^index: Html = h .div\n"),
                ("sub/_.clum", "^@./util\n  x\n"),
                (
                    "sub/util.clum",
                    "@../page\n  index\n\n^x: Html = h .div\n  {index}\n",
                ),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("フォルダの外のファイルは import できません"));
    }

    #[test]
    fn import_cycle_is_error() {
        let dir = temp_dir("cycle");
        let message = resolve_files(
            &dir.path,
            "entry.clum",
            &[
                ("entry.clum", "@./a\n  a\n\nx = a\n"),
                ("a.clum", "@./b\n  b\n\n^a: Html = h .div\n  {b}\n"),
                ("b.clum", "@./a\n  a\n\n^b: Html = h .div\n  {a}\n"),
            ],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("循環 import を検出しました"));
    }

    #[test]
    fn missing_entry_is_error() {
        let dir = temp_dir("missing-entry");
        let message = resolve_files(
            &dir.path,
            "nope.clum",
            &[("index.clum", "^index: Html = h .div\n")],
        )
        .expect_err("エラーを期待しました");
        assert!(message.contains("エントリファイル"));
        assert!(message.contains("開けません"));
    }
}
