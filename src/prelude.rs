#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrKind {
    Bool,
    Value,
}

pub const TAGS: &[&str] = &[
    "a",
    "abbr",
    "address",
    "area",
    "article",
    "aside",
    "audio",
    "b",
    "base",
    "bdi",
    "bdo",
    "blockquote",
    "body",
    "br",
    "button",
    "canvas",
    "caption",
    "cite",
    "code",
    "col",
    "colgroup",
    "data",
    "datalist",
    "dd",
    "del",
    "details",
    "dfn",
    "dialog",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hgroup",
    "hr",
    "html",
    "i",
    "iframe",
    "img",
    "input",
    "ins",
    "kbd",
    "label",
    "legend",
    "li",
    "link",
    "main",
    "map",
    "mark",
    "menu",
    "meta",
    "meter",
    "nav",
    "noscript",
    "object",
    "ol",
    "optgroup",
    "option",
    "output",
    "p",
    "picture",
    "pre",
    "progress",
    "q",
    "rp",
    "rt",
    "ruby",
    "s",
    "samp",
    "script",
    "section",
    "select",
    "slot",
    "small",
    "source",
    "span",
    "strong",
    "style",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "template",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "time",
    "title",
    "tr",
    "track",
    "u",
    "ul",
    "var",
    "video",
    "wbr",
];

const VOID_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

pub fn is_tag(tag: &str) -> bool {
    TAGS.contains(&tag)
}

pub fn is_void(tag: &str) -> bool {
    VOID_TAGS.contains(&tag)
}

pub fn global_attr(name: &str) -> Option<AttrKind> {
    match name {
        "hidden" | "autofocus" | "inert" | "itemscope" => Some(AttrKind::Bool),
        "id" | "class" | "title" | "lang" | "dir" | "style" | "tabindex" | "accesskey"
        | "contenteditable" | "draggable" | "spellcheck" | "translate" | "role" | "slot"
        | "part" | "nonce" | "enterkeyhint" | "inputmode" | "autocapitalize" | "popover" => {
            Some(AttrKind::Value)
        }
        _ => None,
    }
}

pub fn element_attr(tag: &str, name: &str) -> Option<AttrKind> {
    const V: AttrKind = AttrKind::Value;
    const B: AttrKind = AttrKind::Bool;
    let table: &[(&str, AttrKind)] = match tag {
        "a" => &[
            ("href", V),
            ("target", V),
            ("rel", V),
            ("download", V),
            ("hreflang", V),
            ("type", V),
            ("referrerpolicy", V),
            ("ping", V),
        ],
        "area" => &[
            ("href", V),
            ("alt", V),
            ("coords", V),
            ("shape", V),
            ("target", V),
            ("rel", V),
            ("download", V),
            ("referrerpolicy", V),
            ("ping", V),
        ],
        "audio" => &[
            ("src", V),
            ("preload", V),
            ("crossorigin", V),
            ("controls", B),
            ("autoplay", B),
            ("loop", B),
            ("muted", B),
        ],
        "base" => &[("href", V), ("target", V)],
        "blockquote" => &[("cite", V)],
        "button" => &[
            ("type", V),
            ("name", V),
            ("value", V),
            ("form", V),
            ("formaction", V),
            ("formmethod", V),
            ("disabled", B),
        ],
        "canvas" => &[("width", V), ("height", V)],
        "col" => &[("span", V)],
        "colgroup" => &[("span", V)],
        "data" => &[("value", V)],
        "del" => &[("cite", V), ("datetime", V)],
        "details" => &[("open", B)],
        "dialog" => &[("open", B)],
        "embed" => &[("src", V), ("type", V), ("width", V), ("height", V)],
        "fieldset" => &[("name", V), ("form", V), ("disabled", B)],
        "form" => &[
            ("action", V),
            ("method", V),
            ("enctype", V),
            ("target", V),
            ("name", V),
            ("autocomplete", V),
            ("novalidate", B),
        ],
        "iframe" => &[
            ("src", V),
            ("srcdoc", V),
            ("name", V),
            ("width", V),
            ("height", V),
            ("loading", V),
            ("allow", V),
            ("sandbox", V),
            ("referrerpolicy", V),
        ],
        "img" => &[
            ("src", V),
            ("alt", V),
            ("width", V),
            ("height", V),
            ("loading", V),
            ("decoding", V),
            ("srcset", V),
            ("sizes", V),
            ("crossorigin", V),
            ("referrerpolicy", V),
            ("usemap", V),
            ("ismap", B),
        ],
        "input" => &[
            ("type", V),
            ("value", V),
            ("name", V),
            ("placeholder", V),
            ("min", V),
            ("max", V),
            ("step", V),
            ("maxlength", V),
            ("minlength", V),
            ("pattern", V),
            ("autocomplete", V),
            ("size", V),
            ("list", V),
            ("accept", V),
            ("src", V),
            ("alt", V),
            ("form", V),
            ("disabled", B),
            ("required", B),
            ("checked", B),
            ("readonly", B),
            ("multiple", B),
        ],
        "ins" => &[("cite", V), ("datetime", V)],
        "label" => &[("for", V), ("form", V)],
        "li" => &[("value", V)],
        "link" => &[
            ("href", V),
            ("rel", V),
            ("type", V),
            ("media", V),
            ("sizes", V),
            ("as", V),
            ("crossorigin", V),
            ("hreflang", V),
            ("integrity", V),
            ("referrerpolicy", V),
            ("disabled", B),
        ],
        "map" => &[("name", V)],
        "meta" => &[
            ("charset", V),
            ("name", V),
            ("content", V),
            ("http-equiv", V),
            ("media", V),
        ],
        "meter" => &[
            ("value", V),
            ("min", V),
            ("max", V),
            ("low", V),
            ("high", V),
            ("optimum", V),
        ],
        "object" => &[
            ("data", V),
            ("type", V),
            ("name", V),
            ("width", V),
            ("height", V),
            ("form", V),
        ],
        "ol" => &[("start", V), ("type", V), ("reversed", B)],
        "optgroup" => &[("label", V), ("disabled", B)],
        "option" => &[("value", V), ("label", V), ("selected", B), ("disabled", B)],
        "output" => &[("for", V), ("name", V), ("form", V)],
        "progress" => &[("value", V), ("max", V)],
        "q" => &[("cite", V)],
        "script" => &[
            ("src", V),
            ("type", V),
            ("crossorigin", V),
            ("integrity", V),
            ("referrerpolicy", V),
            ("async", B),
            ("defer", B),
            ("nomodule", B),
        ],
        "select" => &[
            ("name", V),
            ("form", V),
            ("size", V),
            ("autocomplete", V),
            ("disabled", B),
            ("multiple", B),
            ("required", B),
        ],
        "source" => &[
            ("src", V),
            ("type", V),
            ("srcset", V),
            ("sizes", V),
            ("media", V),
        ],
        "style" => &[("media", V), ("type", V)],
        "td" => &[("colspan", V), ("rowspan", V), ("headers", V)],
        "textarea" => &[
            ("name", V),
            ("rows", V),
            ("cols", V),
            ("placeholder", V),
            ("maxlength", V),
            ("minlength", V),
            ("wrap", V),
            ("form", V),
            ("disabled", B),
            ("required", B),
            ("readonly", B),
        ],
        "th" => &[
            ("colspan", V),
            ("rowspan", V),
            ("headers", V),
            ("scope", V),
            ("abbr", V),
        ],
        "time" => &[("datetime", V)],
        "track" => &[
            ("src", V),
            ("kind", V),
            ("srclang", V),
            ("label", V),
            ("default", B),
        ],
        "video" => &[
            ("src", V),
            ("poster", V),
            ("preload", V),
            ("width", V),
            ("height", V),
            ("crossorigin", V),
            ("controls", B),
            ("autoplay", B),
            ("loop", B),
            ("muted", B),
            ("playsinline", B),
        ],
        _ => &[],
    };
    table
        .iter()
        .find(|(attr, _)| *attr == name)
        .map(|(_, kind)| *kind)
}

pub fn suggest_tag(unknown: &str) -> Option<&'static str> {
    let mut best: Option<(usize, &'static str)> = None;
    for &tag in TAGS {
        let distance = osa_distance(unknown, tag);
        match best {
            Some((best_distance, _)) if best_distance <= distance => {}
            _ => best = Some((distance, tag)),
        }
    }
    match best {
        Some((distance, tag)) if distance <= 2 => Some(tag),
        _ => None,
    }
}

fn osa_distance(a: &str, b: &str) -> usize {
    let source: Vec<char> = a.chars().collect();
    let target: Vec<char> = b.chars().collect();
    let rows = source.len() + 1;
    let cols = target.len() + 1;
    let mut dist = vec![vec![0usize; cols]; rows];
    for (i, row) in dist.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in dist[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..rows {
        for j in 1..cols {
            let cost = if source[i - 1] == target[j - 1] { 0 } else { 1 };
            let mut value = (dist[i - 1][j] + 1)
                .min(dist[i][j - 1] + 1)
                .min(dist[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && source[i - 1] == target[j - 2] && source[i - 2] == target[j - 1] {
                value = value.min(dist[i - 2][j - 2] + 1);
            }
            dist[i][j] = value;
        }
    }
    dist[source.len()][target.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_and_unknown_tags() {
        assert!(is_tag("div"));
        assert!(is_tag("html"));
        assert!(is_tag("input"));
        assert!(!is_tag("dvi"));
        assert!(!is_tag("foobar"));
    }

    #[test]
    fn void_tags() {
        assert!(is_void("br"));
        assert!(is_void("img"));
        assert!(is_void("input"));
        assert!(!is_void("div"));
        assert!(!is_void("span"));
    }

    #[test]
    fn suggests_nearest_tag() {
        assert_eq!(suggest_tag("dvi"), Some("div"));
        assert_eq!(suggest_tag("spam"), Some("span"));
        assert_eq!(suggest_tag("imgg"), Some("img"));
    }

    #[test]
    fn no_suggestion_when_far() {
        assert_eq!(suggest_tag("qwertyuiop"), None);
    }

    #[test]
    fn global_attributes() {
        assert_eq!(global_attr("id"), Some(AttrKind::Value));
        assert_eq!(global_attr("class"), Some(AttrKind::Value));
        assert_eq!(global_attr("hidden"), Some(AttrKind::Bool));
        assert_eq!(global_attr("href"), None);
    }

    #[test]
    fn element_attributes() {
        assert_eq!(element_attr("a", "href"), Some(AttrKind::Value));
        assert_eq!(element_attr("input", "disabled"), Some(AttrKind::Bool));
        assert_eq!(element_attr("input", "type"), Some(AttrKind::Value));
        assert_eq!(element_attr("meta", "charset"), Some(AttrKind::Value));
        assert_eq!(element_attr("img", "src"), Some(AttrKind::Value));
        assert_eq!(element_attr("div", "href"), None);
    }

    #[test]
    fn osa_counts_transposition_as_one() {
        assert_eq!(osa_distance("dvi", "div"), 1);
        assert_eq!(osa_distance("abc", "abc"), 0);
        assert_eq!(osa_distance("abc", "abd"), 1);
    }
}
