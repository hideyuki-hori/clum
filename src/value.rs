use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::Expr;
use crate::source::FileId;

#[derive(Debug, Clone)]
pub enum Value {
    I32(i32),
    F64(f64),
    Str(Rc<str>),
    Bool(bool),
    Void,
    Vec(Rc<Vec<Value>>),
    Record(Rc<RecordValue>),
    Html(Rc<HtmlNode>),
    Closure(Rc<Closure>),
    Eff(Rc<Effect>),
}

#[derive(Debug, Clone)]
pub struct RecordValue {
    pub fields: Vec<(String, Value)>,
}

impl RecordValue {
    pub fn field(&self, name: &str) -> Option<&Value> {
        self.fields
            .iter()
            .find(|(fname, _)| fname == name)
            .map(|(_, value)| value)
    }
}

#[derive(Debug, Clone)]
pub enum HtmlNode {
    Element {
        tag: String,
        attrs: Vec<(String, Option<String>)>,
        children: Vec<HtmlNode>,
    },
    Text(String),
}

#[derive(Debug, Clone)]
pub struct Origin {
    pub file: FileId,
    pub vec_tail: Rc<HashMap<String, bool>>,
}

#[derive(Debug, Clone)]
pub struct UserFn {
    pub params: Vec<String>,
    pub body: Rc<Expr>,
    pub env: Env,
    pub origin: Origin,
}

#[derive(Debug, Clone)]
pub enum Closure {
    User {
        func: Rc<UserFn>,
        bound: Vec<Value>,
    },
    Lambda {
        param: String,
        body: Rc<Expr>,
        env: Env,
        origin: Origin,
    },
    Build,
}

impl Closure {
    pub fn arity_remaining(&self) -> usize {
        match self {
            Closure::User { func, bound } => func.params.len() - bound.len(),
            Closure::Lambda { .. } => 1,
            Closure::Build => 1,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Effect {
    Build(Value),
}

#[derive(Debug, Clone)]
struct EnvNode {
    vars: HashMap<String, Value>,
    parent: Option<Env>,
}

#[derive(Debug, Clone)]
pub struct Env(Rc<EnvNode>);

impl Env {
    pub fn empty() -> Self {
        Env(Rc::new(EnvNode {
            vars: HashMap::new(),
            parent: None,
        }))
    }

    pub fn child(&self, vars: HashMap<String, Value>) -> Self {
        Env(Rc::new(EnvNode {
            vars,
            parent: Some(self.clone()),
        }))
    }

    pub fn lookup(&self, name: &str) -> Option<Value> {
        if let Some(value) = self.0.vars.get(name) {
            return Some(value.clone());
        }
        self.0
            .parent
            .as_ref()
            .and_then(|parent| parent.lookup(name))
    }
}
