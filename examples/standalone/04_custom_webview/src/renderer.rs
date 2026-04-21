use std::collections::HashMap;

use crate::layout::{LayoutNode, LayoutTree};
use crate::types::*;

pub fn generate_draw_commands(
    tree: &LayoutTree,
    canvas_ops: &HashMap<String, Vec<CanvasDrawOp>>,
) -> Vec<DrawCommand> {
    let mut commands = Vec::new();
    emit_node(&tree.taffy, &tree.root, 0.0, 0.0, canvas_ops, &mut commands);
    commands
}

pub fn hit_test(tree: &LayoutTree, x: f32, y: f32) -> Option<HitResult> {
    let mut result = None;
    hit_test_node(
        &tree.taffy,
        &tree.root,
        0.0,
        0.0,
        x,
        y,
        &[],
        None,
        &mut result,
    );
    result
}

#[derive(Debug, Clone)]
pub struct HitResult {
    pub id: Option<String>,
    pub tag: String,
    pub dom_index: usize,
    pub id_chain: Vec<String>,
    pub href: Option<String>,
}

fn emit_node(
    taffy: &crate::layout::TaffyTree,
    node: &LayoutNode,
    parent_x: f32,
    parent_y: f32,
    canvas_ops: &HashMap<String, Vec<CanvasDrawOp>>,
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
            text: node.text.trim().to_string(),
            x,
            y,
            max_width: w + 1.0,
            color: node.style.color,
            font_size: node.style.font_size,
        });
    }

    if node.tag == "canvas" {
        if let Some(ref id) = node.id {
            if let Some(ops) = canvas_ops.get(id) {
                emit_canvas_ops(ops, x, y, commands);
            }
        }
    }

    for child in &node.children {
        emit_node(taffy, child, x, y, canvas_ops, commands);
    }
}

fn emit_canvas_ops(ops: &[CanvasDrawOp], cx: f32, cy: f32, commands: &mut Vec<DrawCommand>) {
    for op in ops {
        match op {
            CanvasDrawOp::FillRect { x, y, w, h, color } => {
                commands.push(DrawCommand::Rect {
                    rect: LayoutRect {
                        x: cx + x,
                        y: cy + y,
                        w: *w,
                        h: *h,
                    },
                    color: *color,
                    border_radius: 0.0,
                });
            }
            CanvasDrawOp::StrokeRect {
                x,
                y,
                w,
                h,
                color,
                line_width,
            } => {
                commands.push(DrawCommand::Border {
                    rect: LayoutRect {
                        x: cx + x,
                        y: cy + y,
                        w: *w,
                        h: *h,
                    },
                    color: *color,
                    width: *line_width,
                    radius: 0.0,
                });
            }
            CanvasDrawOp::FillCircle {
                cx: ocx,
                cy: ocy,
                radius,
                color,
            } => {
                let r = *radius;
                commands.push(DrawCommand::Rect {
                    rect: LayoutRect {
                        x: cx + ocx - r,
                        y: cy + ocy - r,
                        w: r * 2.0,
                        h: r * 2.0,
                    },
                    color: *color,
                    border_radius: r,
                });
            }
            CanvasDrawOp::StrokeCircle {
                cx: ocx,
                cy: ocy,
                radius,
                color,
                line_width,
            } => {
                let r = *radius;
                commands.push(DrawCommand::Border {
                    rect: LayoutRect {
                        x: cx + ocx - r,
                        y: cy + ocy - r,
                        w: r * 2.0,
                        h: r * 2.0,
                    },
                    color: *color,
                    width: *line_width,
                    radius: r,
                });
            }
            CanvasDrawOp::Line {
                x0,
                y0,
                x1,
                y1,
                color,
                line_width,
            } => {
                commands.push(DrawCommand::Line {
                    x0: cx + x0,
                    y0: cy + y0,
                    x1: cx + x1,
                    y1: cy + y1,
                    color: *color,
                    width: *line_width,
                });
            }
        }
    }
}

fn hit_test_node(
    taffy: &crate::layout::TaffyTree,
    node: &LayoutNode,
    parent_x: f32,
    parent_y: f32,
    mx: f32,
    my: f32,
    ancestor_ids: &[String],
    ancestor_href: Option<&str>,
    result: &mut Option<HitResult>,
) {
    if node.style.display == Display::None {
        return;
    }
    if node.style.pointer_events_none {
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
        let mut chain: Vec<String> = Vec::new();
        if let Some(id) = &node.id {
            chain.push(id.clone());
        }
        chain.extend(ancestor_ids.iter().cloned());

        let effective_href = node.href.as_deref().or(ancestor_href);

        *result = Some(HitResult {
            id: node.id.clone(),
            tag: node.tag.clone(),
            dom_index: node.dom_index,
            id_chain: chain,
            href: effective_href.map(String::from),
        });

        let new_ancestors = if let Some(id) = &node.id {
            let mut a = vec![id.clone()];
            a.extend(ancestor_ids.iter().cloned());
            a
        } else {
            ancestor_ids.to_vec()
        };

        let new_href = node.href.as_deref().or(ancestor_href);

        for child in &node.children {
            hit_test_node(taffy, child, x, y, mx, my, &new_ancestors, new_href, result);
        }
    }
}
