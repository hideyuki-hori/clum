use crate::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub items: Vec<Item>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Decl(Decl),
    Import(Import),
    Binding(Binding),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

impl Name {
    pub fn new(text: impl Into<String>, span: Span) -> Self {
        Self {
            text: text.into(),
            span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decl {
    pub name: Name,
    pub params: Vec<Field>,
    pub ret: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: Name,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    Name {
        name: Name,
        span: Span,
    },
    Generic {
        name: Name,
        arg: Box<TypeExpr>,
        span: Span,
    },
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Name { span, .. } => *span,
            TypeExpr::Generic { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Import {
    pub path: Name,
    pub names: Vec<Name>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub is_pub: bool,
    pub kind: BindingKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BindingKind {
    Value {
        name: Name,
        ty: Option<TypeExpr>,
        value: Expr,
    },
    Impl {
        name: Name,
        def: Name,
        params: Vec<Name>,
        body: Expr,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int {
        value: i64,
        span: Span,
    },
    Float {
        value: f64,
        span: Span,
    },
    Str {
        parts: Vec<StrPart>,
        span: Span,
    },
    Var {
        name: Name,
        span: Span,
    },
    Field {
        base: Box<Expr>,
        field: Name,
        span: Span,
    },
    App {
        func: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    Lambda {
        param: Name,
        body: Box<Expr>,
        span: Span,
    },
    Pipe {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    Bang {
        span: Span,
    },
    Record {
        name: Name,
        fields: Vec<RecordField>,
        span: Span,
    },
    Vec {
        elems: Vec<VecElem>,
        span: Span,
    },
    Block {
        bindings: Vec<Binding>,
        result: Box<Expr>,
        span: Span,
    },
    Element(Element),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Int { span, .. } => *span,
            Expr::Float { span, .. } => *span,
            Expr::Str { span, .. } => *span,
            Expr::Var { span, .. } => *span,
            Expr::Field { span, .. } => *span,
            Expr::App { span, .. } => *span,
            Expr::Lambda { span, .. } => *span,
            Expr::Pipe { span, .. } => *span,
            Expr::Bang { span } => *span,
            Expr::Record { span, .. } => *span,
            Expr::Vec { span, .. } => *span,
            Expr::Block { span, .. } => *span,
            Expr::Element(element) => element.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    Text { text: String, span: Span },
    Interp { expr: Box<Expr>, span: Span },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordField {
    pub name: Name,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VecElem {
    Expr(Expr),
    Record {
        fields: Vec<RecordField>,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub tag: Name,
    pub attrs: Vec<Attr>,
    pub children: Vec<Child>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attr {
    pub name: Name,
    pub value: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Child {
    Element(Element),
    Component(Component),
    Text { parts: Vec<StrPart>, span: Span },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    pub name: Name,
    pub args: Vec<Expr>,
    pub children: Vec<Child>,
    pub span: Span,
}
