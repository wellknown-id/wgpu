use html5ever::tendril::TendrilSink;
use html5ever::{parse_document, ParseOpts};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use crate::types::DomNode;

pub fn parse_html(html: &str) -> DomNode {
    let dom = parse_document(RcDom::default(), ParseOpts::default())
        .from_utf8()
        .read_from(&mut html.as_bytes())
        .expect("failed to parse HTML");

    let body = find_body(&dom.document).unwrap_or_else(|| dom.document.clone());
    convert_node(&body)
}

pub fn extract_styles(html: &str) -> Vec<String> {
    let mut styles = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<style>") {
        let after = &rest[start + 7..];
        if let Some(end) = after.find("</style>") {
            styles.push(after[..end].to_string());
            rest = &after[end + 8..];
        } else {
            break;
        }
    }
    styles
}

pub fn extract_script(html: &str) -> Option<String> {
    let marker = "<script>";
    let start = html.find(marker)?;
    let after = &html[start + marker.len()..];
    let end = after.find("</script>")?;
    Some(after[..end].trim().to_string())
}

fn find_body(handle: &Handle) -> Option<Handle> {
    let node = handle;
    if let NodeData::Element { ref name, .. } = node.data {
        if name.local.as_ref() == "body" {
            return Some(handle.clone());
        }
    }
    for child in node.children.borrow().iter() {
        if let Some(found) = find_body(child) {
            return Some(found);
        }
    }
    None
}

fn convert_node(handle: &Handle) -> DomNode {
    match handle.data {
        NodeData::Element {
            ref name,
            ref attrs,
            ..
        } => {
            let tag = name.local.as_ref().to_string();
            let attrs = attrs.borrow();
            let id = attrs
                .iter()
                .find(|a| a.name.local.as_ref() == "id")
                .map(|a| a.value.to_string());
            let classes = attrs
                .iter()
                .find(|a| a.name.local.as_ref() == "class")
                .map(|a| {
                    a.value
                        .split_whitespace()
                        .map(String::from)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let inline_style = attrs
                .iter()
                .find(|a| a.name.local.as_ref() == "style")
                .map(|a| a.value.to_string())
                .unwrap_or_default();

            let children: Vec<DomNode> =
                handle.children.borrow().iter().map(convert_node).collect();

            DomNode {
                tag,
                id,
                classes,
                inline_style,
                text: String::new(),
                children,
            }
        }
        NodeData::Text { ref contents } => {
            let text = contents.borrow().to_string();
            let text = text.trim().to_string();
            DomNode {
                tag: "#text".to_string(),
                id: None,
                classes: Vec::new(),
                inline_style: String::new(),
                text,
                children: Vec::new(),
            }
        }
        _ => {
            let children: Vec<DomNode> =
                handle.children.borrow().iter().map(convert_node).collect();
            if children.len() == 1 {
                return children.into_iter().next().unwrap();
            }
            DomNode {
                tag: "#fragment".to_string(),
                id: None,
                classes: Vec::new(),
                inline_style: String::new(),
                text: String::new(),
                children,
            }
        }
    }
}
