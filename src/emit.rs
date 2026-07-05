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

fn render_node(node: &HtmlNode, out: &mut String) {
    match node {
        HtmlNode::Text(text) => out.push_str(text),
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
                    out.push_str(value);
                    out.push('"');
                }
            }
            out.push('>');
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

    #[test]
    fn renders_html_root_with_doctype() {
        let node = HtmlNode::Element {
            tag: "html".to_string(),
            attrs: Vec::new(),
            children: Vec::new(),
        };
        assert_eq!(render(&node), "<!DOCTYPE html><html></html>");
    }

    #[test]
    fn renders_fragment_without_doctype() {
        let node = HtmlNode::Element {
            tag: "div".to_string(),
            attrs: Vec::new(),
            children: vec![HtmlNode::Text("hi".to_string())],
        };
        assert_eq!(render(&node), "<div>hi</div>");
    }

    #[test]
    fn renders_attributes() {
        let node = HtmlNode::Element {
            tag: "a".to_string(),
            attrs: vec![
                ("href".to_string(), Some("/about".to_string())),
                ("disabled".to_string(), None),
            ],
            children: Vec::new(),
        };
        assert_eq!(render(&node), "<a href=\"/about\" disabled></a>");
    }
}
