use std::collections::HashMap;

use crate::types;

pub struct TextMeasure {
    pub text: String,
    pub text_width: f32,
    pub font_size: f32,
}

#[derive(Default)]
pub struct TextMeasureCache {
    cache: HashMap<(String, u32, u32), (f32, f32)>,
}

impl TextMeasureCache {
    pub fn measure(
        &mut self,
        text: &str,
        font_size: f32,
        max_width: f32,
        measure_fn: &mut dyn FnMut(&str, f32, f32) -> (f32, f32),
    ) -> (f32, f32) {
        let key = (text.to_string(), font_size.to_bits(), max_width.to_bits());
        if let Some(&val) = self.cache.get(&key) {
            return val;
        }
        let result = measure_fn(text, font_size, max_width);
        self.cache.insert(key, result);
        result
    }
}

pub type TaffyTree = taffy::TaffyTree<Option<TextMeasure>>;

pub struct LayoutTree {
    pub taffy: TaffyTree,
    pub root: LayoutNode,
}

pub struct LayoutNode {
    pub dom_index: usize,
    pub taffy_id: taffy::NodeId,
    pub style: types::ComputedStyle,
    pub text: String,
    pub tag: String,
    pub id: Option<String>,
    pub href: Option<String>,
    pub children: Vec<LayoutNode>,
}

pub fn build_layout(
    styled: &types::StyledNode,
    viewport_w: f32,
    viewport_h: f32,
    cache: &mut TextMeasureCache,
    measure_fn: &mut dyn FnMut(&str, f32, f32) -> (f32, f32),
) -> LayoutTree {
    let mut taffy = TaffyTree::new();
    let mut index_counter = 0;
    let root = build_node(&mut taffy, styled, &mut index_counter, cache, measure_fn);

    let mut root_style = taffy.style(root.taffy_id).unwrap().clone();
    root_style.size = taffy::Size {
        width: taffy::prelude::length(viewport_w),
        height: taffy::Dimension::auto(),
    };
    root_style.min_size = taffy::Size {
        width: taffy::prelude::length(viewport_w),
        height: taffy::prelude::length(viewport_h),
    };
    let _ = taffy.set_style(root.taffy_id, root_style);
    let _ = taffy.compute_layout_with_measure(
        root.taffy_id,
        taffy::Size {
            width: taffy::prelude::AvailableSpace::Definite(viewport_w),
            height: taffy::prelude::AvailableSpace::Definite(viewport_h),
        },
        |known_dimensions, available_space, _node_id, node_context, _style| {
            if let Some(Some(measure)) = node_context {
                let max_w = known_dimensions
                    .width
                    .unwrap_or_else(|| match available_space.width {
                        taffy::prelude::AvailableSpace::Definite(w) => measure.text_width.min(w),
                        taffy::prelude::AvailableSpace::MinContent => measure.font_size * 2.0,
                        taffy::prelude::AvailableSpace::MaxContent => measure.text_width,
                    });
                let line_h = measure.font_size * 1.2;
                let height = known_dimensions.height.unwrap_or_else(|| {
                    if max_w >= measure.text_width {
                        line_h
                    } else {
                        let (_, h) =
                            cache.measure(&measure.text, measure.font_size, max_w, measure_fn);
                        h.max(line_h)
                    }
                });
                taffy::Size {
                    width: max_w,
                    height,
                }
            } else {
                taffy::Size {
                    width: known_dimensions.width.unwrap_or(0.0),
                    height: known_dimensions.height.unwrap_or(0.0),
                }
            }
        },
    );

    LayoutTree { taffy, root }
}

impl LayoutTree {
    pub fn content_height(&self) -> f32 {
        self.taffy
            .layout(self.root.taffy_id)
            .map(|l| l.size.height)
            .unwrap_or(0.0)
    }

    pub fn collect_element_rects(&self) -> std::collections::HashMap<String, types::LayoutRect> {
        let mut map = std::collections::HashMap::new();
        collect_rects_recursive(&self.taffy, &self.root, 0.0, 0.0, &mut map);
        map
    }

    pub fn collect_descendant_ids(&self, parent_id: &str) -> Vec<String> {
        let mut result = Vec::new();
        if let Some(node) = find_node_by_id(&self.root, parent_id) {
            gather_child_ids(node, &mut result);
        }
        result
    }
}

fn find_node_by_id<'a>(node: &'a LayoutNode, id: &str) -> Option<&'a LayoutNode> {
    if node.id.as_deref() == Some(id) {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_node_by_id(child, id) {
            return Some(found);
        }
    }
    None
}

fn gather_child_ids(node: &LayoutNode, result: &mut Vec<String>) {
    for child in &node.children {
        if let Some(ref id) = child.id {
            result.push(id.clone());
        }
        gather_child_ids(child, result);
    }
}

fn collect_rects_recursive(
    taffy: &TaffyTree,
    node: &LayoutNode,
    parent_x: f32,
    parent_y: f32,
    map: &mut std::collections::HashMap<String, types::LayoutRect>,
) {
    let layout = match taffy.layout(node.taffy_id) {
        Ok(l) => l,
        Err(_) => return,
    };
    let x = parent_x + layout.location.x;
    let y = parent_y + layout.location.y;
    let w = layout.size.width;
    let h = layout.size.height;

    if let Some(ref id) = node.id {
        map.insert(id.clone(), types::LayoutRect { x, y, w, h });
    }
    for child in &node.children {
        collect_rects_recursive(taffy, child, x, y, map);
    }
}

fn build_node(
    taffy: &mut TaffyTree,
    styled: &types::StyledNode,
    index_counter: &mut usize,
    cache: &mut TextMeasureCache,
    measure_fn: &mut dyn FnMut(&str, f32, f32) -> (f32, f32),
) -> LayoutNode {
    let idx = *index_counter;
    *index_counter += 1;

    let s = &styled.style;

    if s.display == types::Display::None {
        let taffy_id = taffy.new_leaf(taffy::Style::default()).unwrap();
        return LayoutNode {
            dom_index: idx,
            taffy_id,
            style: s.clone(),
            text: styled.dom_node.text.clone(),
            tag: styled.dom_node.tag.clone(),
            id: styled.dom_node.id.clone(),
            href: styled.dom_node.href.clone(),
            children: Vec::new(),
        };
    }

    let child_nodes: Vec<LayoutNode> = styled
        .children
        .iter()
        .map(|c| build_node(taffy, c, index_counter, cache, measure_fn))
        .collect();

    let child_ids: Vec<taffy::NodeId> = child_nodes.iter().map(|c| c.taffy_id).collect();

    let taffy_style = convert_style(s);

    let taffy_id = if child_ids.is_empty() {
        if !styled.dom_node.text.is_empty() {
            let trimmed = styled.dom_node.text.trim();
            let (tw, _) = cache.measure(trimmed, s.font_size, f32::MAX, measure_fn);
            let context = TextMeasure {
                text: trimmed.to_string(),
                text_width: tw,
                font_size: s.font_size,
            };
            taffy
                .new_leaf_with_context(taffy_style, Some(context))
                .unwrap()
        } else {
            taffy.new_leaf(taffy_style).unwrap()
        }
    } else {
        taffy.new_with_children(taffy_style, &child_ids).unwrap()
    };

    LayoutNode {
        dom_index: idx,
        taffy_id,
        style: s.clone(),
        text: styled.dom_node.text.clone(),
        tag: styled.dom_node.tag.clone(),
        id: styled.dom_node.id.clone(),
        href: styled.dom_node.href.clone(),
        children: child_nodes,
    }
}

fn convert_style(s: &types::ComputedStyle) -> taffy::Style {
    use taffy::prelude::*;

    let position = match s.position {
        types::Position::Fixed | types::Position::Absolute => taffy::Position::Absolute,
        types::Position::Static => taffy::Position::Relative,
    };

    let inset = taffy::Rect {
        left: s
            .left
            .map(|v| length(v))
            .unwrap_or(taffy::LengthPercentageAuto::auto()),
        top: s
            .top
            .map(|v| length(v))
            .unwrap_or(taffy::LengthPercentageAuto::auto()),
        right: s
            .right
            .map(|v| length(v))
            .unwrap_or(taffy::LengthPercentageAuto::auto()),
        bottom: s
            .bottom
            .map(|v| length(v))
            .unwrap_or(taffy::LengthPercentageAuto::auto()),
    };

    taffy::Style {
        box_sizing: taffy::BoxSizing::ContentBox,
        display: match s.display {
            types::Display::Flex => taffy::Display::Flex,
            types::Display::None => taffy::Display::None,
            _ => taffy::Display::Flex,
        },
        position,
        inset,
        flex_direction: match s.display {
            types::Display::Block | types::Display::Inline => taffy::FlexDirection::Column,
            _ => match s.flex_direction {
                types::FlexDirection::Row => taffy::FlexDirection::Row,
                types::FlexDirection::Column => taffy::FlexDirection::Column,
            },
        },
        justify_content: Some(match s.justify_content {
            types::JustifyContent::Start => taffy::JustifyContent::FlexStart,
            types::JustifyContent::Center => taffy::JustifyContent::Center,
            types::JustifyContent::End => taffy::JustifyContent::FlexEnd,
            types::JustifyContent::SpaceBetween => taffy::JustifyContent::SpaceBetween,
            types::JustifyContent::SpaceAround => taffy::JustifyContent::SpaceAround,
            types::JustifyContent::SpaceEvenly => taffy::JustifyContent::SpaceEvenly,
        }),
        align_items: Some(match s.align_items {
            types::AlignItems::Start => taffy::AlignItems::FlexStart,
            types::AlignItems::Center => taffy::AlignItems::Center,
            types::AlignItems::End => taffy::AlignItems::FlexEnd,
            types::AlignItems::Stretch => taffy::AlignItems::Stretch,
        }),
        size: taffy::Size {
            width: convert_dimension(s.width),
            height: convert_dimension(s.height),
        },
        min_size: taffy::Size {
            width: convert_dimension(s.min_width),
            height: convert_dimension(s.min_height),
        },
        padding: taffy::Rect {
            top: length(s.padding.top),
            right: length(s.padding.right),
            bottom: length(s.padding.bottom),
            left: length(s.padding.left),
        },
        margin: taffy::Rect {
            top: length(s.margin.top),
            right: length(s.margin.right),
            bottom: length(s.margin.bottom),
            left: length(s.margin.left),
        },
        gap: taffy::Size {
            width: length(s.gap),
            height: length(s.gap),
        },
        flex_grow: s.flex_grow,
        flex_shrink: s.flex_shrink,
        flex_basis: convert_dimension(s.flex_basis),
        flex_wrap: taffy::FlexWrap::NoWrap,
        ..Default::default()
    }
}

fn convert_dimension(d: types::Dimension) -> taffy::Dimension {
    match d {
        types::Dimension::Px(v) => taffy::prelude::length(v),
        types::Dimension::Percent(v) => taffy::prelude::percent(v / 100.0),
        types::Dimension::Auto => taffy::Dimension::auto(),
    }
}
