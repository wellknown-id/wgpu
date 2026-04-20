use crate::types;

pub struct LayoutTree {
    pub taffy: taffy::TaffyTree,
    pub root: LayoutNode,
}

pub struct LayoutNode {
    pub dom_index: usize,
    pub taffy_id: taffy::NodeId,
    pub style: types::ComputedStyle,
    pub text: String,
    pub tag: String,
    pub id: Option<String>,
    pub children: Vec<LayoutNode>,
}

pub fn build_layout(styled: &types::StyledNode, viewport_w: f32, viewport_h: f32) -> LayoutTree {
    let mut taffy = taffy::TaffyTree::new();
    let mut index_counter = 0;
    let root = build_node(&mut taffy, styled, &mut index_counter, false, false);

    let mut root_style = taffy.style(root.taffy_id).unwrap().clone();
    root_style.size = taffy::Size {
        width: taffy::prelude::length(viewport_w),
        height: taffy::prelude::length(viewport_h),
    };
    let _ = taffy.set_style(root.taffy_id, root_style);
    let _ = taffy.compute_layout(
        root.taffy_id,
        taffy::Size {
            width: taffy::prelude::AvailableSpace::Definite(viewport_w),
            height: taffy::prelude::AvailableSpace::Definite(viewport_h),
        },
    );

    LayoutTree { taffy, root }
}

fn is_row_container(s: &types::ComputedStyle) -> bool {
    match s.display {
        types::Display::Flex => matches!(s.flex_direction, types::FlexDirection::Row),
        _ => false, // block/inline are column containers
    }
}

fn build_node(
    taffy: &mut taffy::TaffyTree,
    styled: &types::StyledNode,
    index_counter: &mut usize,
    parent_is_row: bool,
    need_intrinsic_width: bool,
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
            children: Vec::new(),
        };
    }

    let this_is_row = is_row_container(s);
    // Column containers in row parents need children to provide intrinsic width,
    // UNLESS this element's width is externally constrained (explicit width or
    // flex-basis in a row parent). Row containers always reset the flag.
    let has_constrained_width = !matches!(s.width, types::Dimension::Auto)
        || (parent_is_row && !matches!(s.flex_basis, types::Dimension::Auto));
    let children_need_intrinsic = if this_is_row || has_constrained_width {
        false
    } else {
        parent_is_row || need_intrinsic_width
    };

    let child_nodes: Vec<LayoutNode> = styled
        .children
        .iter()
        .map(|c| build_node(taffy, c, index_counter, this_is_row, children_need_intrinsic))
        .collect();

    let child_ids: Vec<taffy::NodeId> = child_nodes.iter().map(|c| c.taffy_id).collect();

    let taffy_style = convert_style(s);

    let taffy_id = if child_ids.is_empty() {
        if !styled.dom_node.text.is_empty() {
            let text_h = s.font_size * 1.3;

            let mut style = taffy_style;
            if parent_is_row || need_intrinsic_width {
                let text_len = styled.dom_node.text.len() as f32;
                let char_w = s.font_size * 0.6;
                let text_w = text_len * char_w;
                style.size.width = taffy::prelude::length(text_w);
            }
            style.min_size.height = taffy::prelude::length(text_h);
            style.flex_shrink = 1.0;
            taffy.new_leaf(style).unwrap()
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
