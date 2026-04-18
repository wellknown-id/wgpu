use crate::layout::{LayoutNode, LayoutTree};
use crate::types::*;

pub fn generate_draw_commands(tree: &LayoutTree) -> Vec<DrawCommand> {
    let mut commands = Vec::new();
    emit_node(&tree.taffy, &tree.root, 0.0, 0.0, &mut commands);
    commands
}

pub fn hit_test(tree: &LayoutTree, x: f32, y: f32) -> Option<HitResult> {
    let mut result = None;
    hit_test_node(&tree.taffy, &tree.root, 0.0, 0.0, x, y, &mut result);
    result
}

#[derive(Debug, Clone)]
pub struct HitResult {
    pub id: Option<String>,
    pub tag: String,
    pub dom_index: usize,
}

fn emit_node(
    taffy: &taffy::TaffyTree,
    node: &LayoutNode,
    parent_x: f32,
    parent_y: f32,
    commands: &mut Vec<DrawCommand>,
) {
    if node.style.display == Display::None {
        return;
    }

    let layout = match taffy.layout(node.taffy_id) {
        Ok(l) => l,
        Err(_) => return,
    };

    let x = parent_x + layout.location.x;
    let y = parent_y + layout.location.y;
    let w = layout.size.width;
    let h = layout.size.height;

    if node.style.background_color[3] > 0.0 {
        commands.push(DrawCommand::Rect {
            rect: LayoutRect { x, y, w, h },
            color: node.style.background_color,
            border_radius: node.style.border_radius,
        });
    }

    if node.style.border_width > 0.0 && node.style.border_color[3] > 0.0 {
        commands.push(DrawCommand::Border {
            rect: LayoutRect { x, y, w, h },
            color: node.style.border_color,
            width: node.style.border_width,
            radius: node.style.border_radius,
        });
    }

    if !node.text.is_empty() {
        commands.push(DrawCommand::Text {
            text: node.text.clone(),
            x,
            y,
            max_width: w,
            color: node.style.color,
            font_size: node.style.font_size,
        });
    }

    for child in &node.children {
        emit_node(taffy, child, x, y, commands);
    }
}

fn hit_test_node(
    taffy: &taffy::TaffyTree,
    node: &LayoutNode,
    parent_x: f32,
    parent_y: f32,
    mx: f32,
    my: f32,
    result: &mut Option<HitResult>,
) {
    if node.style.display == Display::None {
        return;
    }

    let layout = match taffy.layout(node.taffy_id) {
        Ok(l) => l,
        Err(_) => return,
    };

    let x = parent_x + layout.location.x;
    let y = parent_y + layout.location.y;
    let w = layout.size.width;
    let h = layout.size.height;

    if mx >= x && mx <= x + w && my >= y && my <= y + h {
        *result = Some(HitResult {
            id: node.id.clone(),
            tag: node.tag.clone(),
            dom_index: node.dom_index,
        });
    }

    for child in &node.children {
        hit_test_node(taffy, child, x, y, mx, my, result);
    }
}
