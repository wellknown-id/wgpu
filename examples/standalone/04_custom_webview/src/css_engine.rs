use crate::types::*;

struct CssRule {
    selector: Selector,
    properties: Vec<(String, String)>,
}

enum Selector {
    Tag(String),
    Class(String),
    Id(String),
    TagClass(String, String),
}

pub fn apply_styles(root: &DomNode, css_sources: &[String]) -> StyledNode {
    let rules = parse_all_css(css_sources);
    style_node(root, &rules, &ComputedStyle::default())
}

fn style_node(node: &DomNode, rules: &[CssRule], parent_style: &ComputedStyle) -> StyledNode {
    let mut style = ComputedStyle {
        color: parent_style.color,
        font_size: parent_style.font_size,
        ..ComputedStyle::default()
    };

    if node.tag == "#text" {
        style.display = Display::Inline;
    }

    apply_tag_defaults(&node.tag, &mut style);

    for rule in rules {
        if selector_matches(&rule.selector, node) {
            apply_properties(&rule.properties, &mut style);
        }
    }

    if !node.inline_style.is_empty() {
        let inline_props = parse_declarations(&node.inline_style);
        apply_properties(&inline_props, &mut style);
    }

    let children = node
        .children
        .iter()
        .filter(|c| !(c.tag == "#text" && c.text.is_empty()))
        .map(|c| style_node(c, rules, &style))
        .collect();

    StyledNode {
        dom_node: node.clone(),
        style,
        children,
    }
}

fn apply_tag_defaults(tag: &str, style: &mut ComputedStyle) {
    match tag {
        "h1" => {
            style.font_size = 32.0;
            style.margin = Edges {
                top: 12.0,
                bottom: 12.0,
                left: 0.0,
                right: 0.0,
            };
        }
        "h2" => {
            style.font_size = 24.0;
            style.margin = Edges {
                top: 8.0,
                bottom: 8.0,
                left: 0.0,
                right: 0.0,
            };
        }
        "h3" => {
            style.font_size = 20.0;
            style.margin = Edges {
                top: 6.0,
                bottom: 6.0,
                left: 0.0,
                right: 0.0,
            };
        }
        "p" => {
            style.margin = Edges {
                top: 8.0,
                bottom: 8.0,
                left: 0.0,
                right: 0.0,
            };
        }
        _ => {}
    }
}

fn selector_matches(selector: &Selector, node: &DomNode) -> bool {
    match selector {
        Selector::Tag(tag) => node.tag == *tag,
        Selector::Class(cls) => node.classes.contains(cls),
        Selector::Id(id) => node.id.as_deref() == Some(id.as_str()),
        Selector::TagClass(tag, cls) => node.tag == *tag && node.classes.contains(cls),
    }
}

fn apply_properties(props: &[(String, String)], style: &mut ComputedStyle) {
    for (name, val) in props {
        let val = val.trim();
        match name.as_str() {
            "display" => {
                style.display = match val {
                    "flex" => Display::Flex,
                    "block" => Display::Block,
                    "inline" => Display::Inline,
                    "none" => Display::None,
                    _ => style.display,
                };
            }
            "flex-direction" => {
                style.flex_direction = match val {
                    "column" => FlexDirection::Column,
                    _ => FlexDirection::Row,
                };
            }
            "justify-content" => {
                style.justify_content = match val {
                    "center" => JustifyContent::Center,
                    "flex-end" | "end" => JustifyContent::End,
                    "space-between" => JustifyContent::SpaceBetween,
                    "space-around" => JustifyContent::SpaceAround,
                    "space-evenly" => JustifyContent::SpaceEvenly,
                    _ => JustifyContent::Start,
                };
            }
            "align-items" => {
                style.align_items = match val {
                    "center" => AlignItems::Center,
                    "flex-start" | "start" => AlignItems::Start,
                    "flex-end" | "end" => AlignItems::End,
                    _ => AlignItems::Stretch,
                };
            }
            "width" => {
                if let Some(px) = parse_length(val) {
                    style.width = Dimension::Px(px);
                }
            }
            "height" => {
                if let Some(px) = parse_length(val) {
                    style.height = Dimension::Px(px);
                }
            }
            "position" => {
                style.position = match val {
                    "fixed" => Position::Fixed,
                    "absolute" => Position::Absolute,
                    _ => Position::Static,
                };
            }
            "left" => {
                style.left = parse_length(val);
            }
            "top" => {
                style.top = parse_length(val);
            }
            "right" => {
                style.right = parse_length(val);
            }
            "bottom" => {
                style.bottom = parse_length(val);
            }
            "padding" => {
                let parts: Vec<&str> = val.split_whitespace().collect();
                match parts.len() {
                    1 => {
                        if let Some(px) = parse_length(parts[0]) {
                            style.padding = Edges::uniform(px);
                        }
                    }
                    2 => {
                        if let (Some(tb), Some(lr)) =
                            (parse_length(parts[0]), parse_length(parts[1]))
                        {
                            style.padding.top = tb;
                            style.padding.bottom = tb;
                            style.padding.left = lr;
                            style.padding.right = lr;
                        }
                    }
                    4 => {
                        if let (Some(t), Some(r), Some(b), Some(l)) = (
                            parse_length(parts[0]),
                            parse_length(parts[1]),
                            parse_length(parts[2]),
                            parse_length(parts[3]),
                        ) {
                            style.padding.top = t;
                            style.padding.right = r;
                            style.padding.bottom = b;
                            style.padding.left = l;
                        }
                    }
                    _ => {}
                }
            }
            "padding-top" => {
                if let Some(v) = parse_length(val) {
                    style.padding.top = v;
                }
            }
            "padding-right" => {
                if let Some(v) = parse_length(val) {
                    style.padding.right = v;
                }
            }
            "padding-bottom" => {
                if let Some(v) = parse_length(val) {
                    style.padding.bottom = v;
                }
            }
            "padding-left" => {
                if let Some(v) = parse_length(val) {
                    style.padding.left = v;
                }
            }
            "margin" => {
                let parts: Vec<&str> = val.split_whitespace().collect();
                match parts.len() {
                    1 => {
                        if let Some(px) = parse_length(parts[0]) {
                            style.margin = Edges::uniform(px);
                        }
                    }
                    2 => {
                        if let (Some(tb), Some(lr)) =
                            (parse_length(parts[0]), parse_length(parts[1]))
                        {
                            style.margin.top = tb;
                            style.margin.bottom = tb;
                            style.margin.left = lr;
                            style.margin.right = lr;
                        }
                    }
                    4 => {
                        if let (Some(t), Some(r), Some(b), Some(l)) = (
                            parse_length(parts[0]),
                            parse_length(parts[1]),
                            parse_length(parts[2]),
                            parse_length(parts[3]),
                        ) {
                            style.margin.top = t;
                            style.margin.right = r;
                            style.margin.bottom = b;
                            style.margin.left = l;
                        }
                    }
                    _ => {}
                }
            }
            "margin-top" => {
                if let Some(v) = parse_length(val) {
                    style.margin.top = v;
                }
            }
            "margin-right" => {
                if let Some(v) = parse_length(val) {
                    style.margin.right = v;
                }
            }
            "margin-bottom" => {
                if let Some(v) = parse_length(val) {
                    style.margin.bottom = v;
                }
            }
            "margin-left" => {
                if let Some(v) = parse_length(val) {
                    style.margin.left = v;
                }
            }
            "gap" => {
                if let Some(v) = parse_length(val) {
                    style.gap = v;
                }
            }
            "flex" => {
                if let Ok(v) = val.parse::<f32>() {
                    style.flex_grow = v;
                    style.flex_shrink = 1.0;
                    style.flex_basis = Dimension::Px(0.0);
                }
            }
            "flex-grow" => {
                if let Ok(v) = val.parse::<f32>() {
                    style.flex_grow = v;
                }
            }
            "flex-shrink" => {
                if let Ok(v) = val.parse::<f32>() {
                    style.flex_shrink = v;
                }
            }
            "background" | "background-color" => {
                if let Some(c) = parse_color(val) {
                    style.background_color = c;
                }
            }
            "color" => {
                if let Some(c) = parse_color(val) {
                    style.color = c;
                }
            }
            "font-size" => {
                if let Some(v) = parse_length(val) {
                    style.font_size = v;
                }
            }
            "border-width" => {
                if let Some(v) = parse_length(val) {
                    style.border_width = v;
                }
            }
            "border-color" => {
                if let Some(c) = parse_color(val) {
                    style.border_color = c;
                }
            }
            "border-radius" => {
                if let Some(v) = parse_length(val) {
                    style.border_radius = v;
                }
            }
            "overflow" => {
                style.overflow_hidden = val == "hidden";
            }
            "pointer-events" => {
                style.pointer_events_none = val == "none";
            }
            "transform" => {
                style.transform = parse_transform(val);
            }
            "perspective" => {
                if let Some(v) = parse_length(val) {
                    style.perspective = Some(v);
                }
            }
            _ => {}
        }
    }
}

fn parse_transform(s: &str) -> [f32; 16] {
    let mut m = mat4_identity();
    // Use a simple regex-like split to find functions: e.g. "rotateX(45deg) translateZ(10px)"
    // The current simple split_whitespace won't handle spaces inside parentheses if any,
    // but our demo doesn't have them.
    for part in s.split(')') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(brace_start) = part.find('(') {
            let func = &part[..brace_start].trim();
            let args_str = &part[brace_start + 1..];

            match *func {
                "perspective" => {
                    if let Some(d) = parse_length(args_str) {
                        m = mat4_perspective(&m, d);
                    }
                }
                "rotateX" => {
                    if let Some(deg) = parse_angle(args_str) {
                        m = mat4_rotate_x(&m, deg.to_radians());
                    }
                }
                "rotateY" => {
                    if let Some(deg) = parse_angle(args_str) {
                        m = mat4_rotate_y(&m, deg.to_radians());
                    }
                }
                "translateZ" => {
                    if let Some(z) = parse_length(args_str) {
                        m = mat4_translate(&m, 0.0, 0.0, z);
                    }
                }
                _ => {}
            }
        }
    }
    m
}

fn parse_angle(s: &str) -> Option<f32> {
    let s = s.trim();
    if let Some(v) = s.strip_suffix("deg") {
        return v.trim().parse().ok();
    }
    s.parse().ok()
}

fn parse_length(s: &str) -> Option<f32> {
    let s = s.trim();
    if let Some(v) = s.strip_suffix("px") {
        return v.trim().parse().ok();
    }
    if let Some(v) = s.strip_suffix("em") {
        return v.trim().parse::<f32>().ok().map(|v| v * 16.0);
    }
    if let Some(v) = s.strip_suffix("rem") {
        return v.trim().parse::<f32>().ok().map(|v| v * 16.0);
    }
    if let Some(v) = s.strip_suffix('%') {
        return v.trim().parse::<f32>().ok();
    }
    s.parse().ok()
}

fn parse_color(s: &str) -> Option<[f32; 4]> {
    let s = s.trim();
    if s.starts_with('#') {
        let hex = &s[1..];
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0])
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0])
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some([
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    b as f32 / 255.0,
                    a as f32 / 255.0,
                ])
            }
            _ => None,
        };
    }
    if s.starts_with("rgb") {
        let inner = s.split('(').nth(1)?.split(')').next()?;
        let parts: Vec<f32> = inner
            .split(|c: char| c == ',' || c == '/')
            .filter_map(|p| p.trim().trim_end_matches('%').parse().ok())
            .collect();
        if parts.len() >= 3 {
            return Some([
                parts[0] / 255.0,
                parts[1] / 255.0,
                parts[2] / 255.0,
                parts.get(3).copied().unwrap_or(1.0),
            ]);
        }
    }
    match s {
        "white" => Some([1.0, 1.0, 1.0, 1.0]),
        "black" => Some([0.0, 0.0, 0.0, 1.0]),
        "red" => Some([1.0, 0.0, 0.0, 1.0]),
        "green" => Some([0.0, 0.5, 0.0, 1.0]),
        "blue" => Some([0.0, 0.0, 1.0, 1.0]),
        "transparent" => Some([0.0, 0.0, 0.0, 0.0]),
        _ => None,
    }
}

fn parse_all_css(sources: &[String]) -> Vec<CssRule> {
    let mut rules = Vec::new();
    for source in sources {
        rules.extend(parse_stylesheet(source));
    }
    rules
}

fn parse_stylesheet(css: &str) -> Vec<CssRule> {
    let mut rules = Vec::new();
    let mut rest = css;

    while let Some(brace_start) = rest.find('{') {
        let selector_part = rest[..brace_start].trim();
        let after_brace = &rest[brace_start + 1..];
        let Some(brace_end) = after_brace.find('}') else {
            break;
        };
        let body = &after_brace[..brace_end];
        rest = &after_brace[brace_end + 1..];

        let properties = parse_declarations(body);

        for sel_str in selector_part.split(',') {
            let sel_str = sel_str.trim();
            if sel_str.is_empty() {
                continue;
            }
            // Handle compound selectors like ".header h1" by splitting on
            // whitespace and only using the last component (child selector).
            // This is a simplification but covers the common cases.
            let parts: Vec<&str> = sel_str.split_whitespace().collect();

            if parts.len() == 1 {
                if let Some(sel) = parse_single_selector(parts[0]) {
                    rules.push(CssRule {
                        selector: sel,
                        properties: properties.clone(),
                    });
                }
            } else if parts.len() == 2 {
                // ".header h1" matches h1 elements inside .header
                // For now, just use the child selector -- good enough for our demo
                if let Some(sel) = parse_single_selector(parts[parts.len() - 1]) {
                    rules.push(CssRule {
                        selector: sel,
                        properties: properties.clone(),
                    });
                }
            }
        }
    }

    rules
}

fn parse_single_selector(s: &str) -> Option<Selector> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix('#') {
        Some(Selector::Id(rest.to_string()))
    } else if let Some(rest) = s.strip_prefix('.') {
        Some(Selector::Class(rest.to_string()))
    } else if s.contains('.') {
        let mut parts = s.splitn(2, '.');
        let tag = parts.next()?.to_string();
        let cls = parts.next()?.to_string();
        Some(Selector::TagClass(tag, cls))
    } else {
        Some(Selector::Tag(s.to_string()))
    }
}

fn parse_declarations(body: &str) -> Vec<(String, String)> {
    let mut props = Vec::new();
    for decl in body.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        if let Some(colon) = decl.find(':') {
            let name = decl[..colon].trim().to_string();
            let value = decl[colon + 1..].trim().to_string();
            if !name.is_empty() && !value.is_empty() {
                props.push((name, value));
            }
        }
    }
    props
}

pub fn apply_style_overrides(
    node: &mut crate::types::StyledNode,
    overrides: &std::collections::HashMap<String, Vec<(String, String)>>,
) {
    if let Some(id) = &node.dom_node.id {
        if let Some(props) = overrides.get(id) {
            apply_properties(props, &mut node.style);
        }
    }
    for child in &mut node.children {
        apply_style_overrides(child, overrides);
    }
}
