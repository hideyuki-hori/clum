use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    I32,
    I64,
    F32,
    F64,
    Str,
    Bool,
    Void,
    Html,
    Tag,
    Vec(Box<Ty>),
    Eff(Box<Ty>),
    Record(String),
    Fn(Vec<Ty>, Box<Ty>),
}

impl Ty {
    pub fn is_numeric(&self) -> bool {
        matches!(self, Ty::I32 | Ty::I64 | Ty::F32 | Ty::F64)
    }

    pub fn is_html(&self) -> bool {
        matches!(self, Ty::Html)
    }

    pub fn is_vec_of_html(&self) -> bool {
        matches!(self, Ty::Vec(inner) if inner.is_html())
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::I32 => f.write_str("i32"),
            Ty::I64 => f.write_str("i64"),
            Ty::F32 => f.write_str("f32"),
            Ty::F64 => f.write_str("f64"),
            Ty::Str => f.write_str("String"),
            Ty::Bool => f.write_str("Bool"),
            Ty::Void => f.write_str("Void"),
            Ty::Html => f.write_str("Html"),
            Ty::Tag => f.write_str("Tag"),
            Ty::Vec(inner) => write!(f, "Vec<{inner}>"),
            Ty::Eff(inner) => write!(f, "Eff<{inner}>"),
            Ty::Record(name) => f.write_str(name),
            Ty::Fn(params, ret) => {
                f.write_str("(")?;
                for (index, param) in params.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{param}")?;
                }
                write!(f, " -> {ret})")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_simple_types() {
        assert_eq!(Ty::I32.to_string(), "i32");
        assert_eq!(Ty::Str.to_string(), "String");
        assert_eq!(Ty::Html.to_string(), "Html");
        assert_eq!(Ty::Void.to_string(), "Void");
    }

    #[test]
    fn display_generic_types() {
        assert_eq!(Ty::Vec(Box::new(Ty::Html)).to_string(), "Vec<Html>");
        assert_eq!(Ty::Eff(Box::new(Ty::Void)).to_string(), "Eff<Void>");
        assert_eq!(
            Ty::Vec(Box::new(Ty::Record("Document".to_string()))).to_string(),
            "Vec<Document>"
        );
    }

    #[test]
    fn display_function_type() {
        let ty = Ty::Fn(vec![Ty::Str, Ty::Html], Box::new(Ty::Html));
        assert_eq!(ty.to_string(), "(String, Html -> Html)");
    }

    #[test]
    fn display_record_type() {
        assert_eq!(Ty::Record("Recipe".to_string()).to_string(), "Recipe");
    }

    #[test]
    fn numeric_predicate() {
        assert!(Ty::I32.is_numeric());
        assert!(Ty::F64.is_numeric());
        assert!(!Ty::Str.is_numeric());
        assert!(!Ty::Html.is_numeric());
    }

    #[test]
    fn html_predicates() {
        assert!(Ty::Html.is_html());
        assert!(Ty::Vec(Box::new(Ty::Html)).is_vec_of_html());
        assert!(!Ty::Vec(Box::new(Ty::Str)).is_vec_of_html());
    }
}
