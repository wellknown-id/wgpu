use crate::types;

pub struct TextMeasure {
    pub text_len: f32,
    pub font_size: f32,
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

pub fn build_layout(styled: &types::StyledNode, viewport_w: f32, viewport_h: f32) -> LayoutTree {
    let mut taffy = TaffyTree::new();
    let mut index_counter = 0;
    let root = build_node(&mut taffy, styled, &mut index_counter);

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
                measure_text(known_dimensions, available_space, measure)
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
}

fn measure_text(
    known_dimensions: taffy::Size<Option<f32>>,
    available_space: taffy::Size<taffy::prelude::AvailableSpace>,
    measure: &TextMeasure,
) -> taffy::Size<f32> {
    let char_w = measure.font_size * 0.6;
    let full_text_w = measure.text_len * char_w;
    let line_h = measure.font_size * 1.3;

    let width = known_dimensions
        .width
        .unwrap_or_else(|| match available_space.width {
            taffy::prelude::AvailableSpace::Definite(w) => full_text_w.min(w),
            taffy::prelude::AvailableSpace::MinContent => char_w * 3.0,
            taffy::prelude::AvailableSpace::MaxContent => full_text_w,
        });

    let height = known_dimensions.height.unwrap_or_else(|| {
        if width > 0.0 && full_text_w > 0.0 {
            let lines = (full_text_w / width).ceil().max(1.0);
            lines * line_h
        } else {
            line_h
        }
    });

    taffy::Size { width, height }
}

fn build_node(
    taffy: &mut TaffyTree,
    styled: &types::StyledNode,
    index_counter: &mut usize,
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
        .map(|c| build_node(taffy, c, index_counter))
        .collect();

    let child_ids: Vec<taffy::NodeId> = child_nodes.iter().map(|c| c.taffy_id).collect();

    let taffy_style = convert_style(s);

    let taffy_id = if child_ids.is_empty() {
        if !styled.dom_node.text.is_empty() {
            let context = TextMeasure {
                text_len: styled.dom_node.text.len() as f32,
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

    taffy::Style {
        display: match s.display {
            types::Display::Flex => taffy::Display::Flex,
            types::Display::None => taffy::Display::None,
            _ => taffy::Display::Flex, // block maps to flex column in taffy
        },
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
