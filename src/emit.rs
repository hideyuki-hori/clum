use crate::prelude;
use crate::value::HtmlNode;

pub fn render(node: &HtmlNode) -> String {
    let mut out = String::new();
    if is_html_root(node) {
        out.push_str("<!DOCTYPE html>");
    }
    render_node(node, &mut out);
    out
}

fn is_html_root(node: &HtmlNode) -> bool {
    matches!(node, HtmlNode::Element { tag, .. } if tag == "html")
}

fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            other => out.push(other),
        }
    }
    out
}

fn render_node(node: &HtmlNode, out: &mut String) {
    match node {
        HtmlNode::Text(text) => out.push_str(&escape_text(text)),
        HtmlNode::Element {
            tag,
            attrs,
            children,
        } => {
            out.push('<');
            out.push_str(tag);
            for (name, value) in attrs {
                out.push(' ');
                out.push_str(name);
                if let Some(value) = value {
                    out.push_str("=\"");
                    out.push_str(&escape_attr(value));
                    out.push('"');
                }
            }
            out.push('>');
            if prelude::is_void(tag) {
                return;
            }
            for child in children {
                render_node(child, out);
            }
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(tag: &str, attrs: Vec<(&str, Option<&str>)>, children: Vec<HtmlNode>) -> HtmlNode {
        HtmlNode::Element {
            tag: tag.to_string(),
            attrs: attrs
                .into_iter()
                .map(|(name, value)| (name.to_string(), value.map(str::to_string)))
                .collect(),
            children,
        }
    }

    #[test]
    fn renders_html_root_with_doctype() {
        let node = element("html", Vec::new(), Vec::new());
        assert_eq!(render(&node), "<!DOCTYPE html><html></html>");
    }

    #[test]
    fn renders_fragment_without_doctype() {
        let node = element("div", Vec::new(), vec![HtmlNode::Text("hi".to_string())]);
        assert_eq!(render(&node), "<div>hi</div>");
    }

    #[test]
    fn renders_value_and_boolean_attributes() {
        let node = element(
            "a",
            vec![("href", Some("/about")), ("disabled", None)],
            Vec::new(),
        );
        assert_eq!(render(&node), "<a href=\"/about\" disabled></a>");
    }

    #[test]
    fn escapes_text_amp_lt_gt() {
        let node = element(
            "p",
            Vec::new(),
            vec![HtmlNode::Text("a & b < c > d".to_string())],
        );
        assert_eq!(render(&node), "<p>a &amp; b &lt; c &gt; d</p>");
    }

    #[test]
    fn escapes_attribute_amp_quot_lt_but_not_gt() {
        let node = element("a", vec![("href", Some("a&b\"c<d>e"))], Vec::new());
        assert_eq!(render(&node), "<a href=\"a&amp;b&quot;c&lt;d>e\"></a>");
    }

    #[test]
    fn void_element_has_no_closing_tag_even_with_attributes() {
        let node = element(
            "img",
            vec![("src", Some("/x.png")), ("alt", Some("x"))],
            Vec::new(),
        );
        assert_eq!(render(&node), "<img src=\"/x.png\" alt=\"x\">");
    }

    #[test]
    fn void_element_without_attributes() {
        let node = element("br", Vec::new(), Vec::new());
        assert_eq!(render(&node), "<br>");
    }

    #[test]
    fn consecutive_text_nodes_concatenate_without_insertion() {
        let node = element(
            "div",
            Vec::new(),
            vec![
                HtmlNode::Text("aa".to_string()),
                HtmlNode::Text("bb".to_string()),
                HtmlNode::Text("cc".to_string()),
            ],
        );
        assert_eq!(render(&node), "<div>aabbcc</div>");
    }

    #[test]
    fn nested_elements_produce_no_whitespace() {
        let node = element(
            "div",
            Vec::new(),
            vec![element(
                "p",
                Vec::new(),
                vec![element(
                    "span",
                    Vec::new(),
                    vec![HtmlNode::Text("x".to_string())],
                )],
            )],
        );
        assert_eq!(render(&node), "<div><p><span>x</span></p></div>");
    }
}
