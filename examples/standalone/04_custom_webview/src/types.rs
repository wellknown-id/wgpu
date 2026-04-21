#[derive(Debug, Clone)]
pub struct DomNode {
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub inline_style: String,
    pub href: Option<String>,
    pub text: String,
    pub children: Vec<DomNode>,
}

#[derive(Debug, Clone)]
pub struct ComputedStyle {
    pub display: Display,
    pub position: Position,
    pub left: Option<f32>,
    pub top: Option<f32>,
    pub right: Option<f32>,
    pub bottom: Option<f32>,
    pub flex_direction: FlexDirection,
    pub justify_content: JustifyContent,
    pub align_items: AlignItems,
    pub width: Dimension,
    pub height: Dimension,
    pub min_width: Dimension,
    pub min_height: Dimension,
    pub padding: Edges,
    pub margin: Edges,
    pub gap: f32,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Dimension,
    pub background_color: [f32; 4],
    pub color: [f32; 4],
    pub font_size: f32,
    pub border_width: f32,
    pub border_color: [f32; 4],
    pub border_radius: f32,
    pub overflow_hidden: bool,
    pub pointer_events_none: bool,
    pub transform: [f32; 16],
    pub perspective: Option<f32>,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        Self {
            display: Display::Block,
            position: Position::Static,
            left: None,
            top: None,
            right: None,
            bottom: None,
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::Start,
            align_items: AlignItems::Stretch,
            width: Dimension::Auto,
            height: Dimension::Auto,
            min_width: Dimension::Auto,
            min_height: Dimension::Auto,
            padding: Edges::zero(),
            margin: Edges::zero(),
            gap: 0.0,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Dimension::Auto,
            background_color: [0.0; 4],
            color: [1.0, 1.0, 1.0, 1.0],
            font_size: 16.0,
            border_width: 0.0,
            border_color: [0.0; 4],
            border_radius: 0.0,
            overflow_hidden: false,
            pointer_events_none: false,
            transform: [
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                0.0, 0.0, 0.0, 1.0,
            ],
            perspective: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Display {
    Block,
    Flex,
    Inline,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Position {
    Static,
    Fixed,
    Absolute,
}

#[derive(Debug, Clone, Copy)]
pub enum FlexDirection {
    Row,
    Column,
}

#[derive(Debug, Clone, Copy)]
pub enum JustifyContent {
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Debug, Clone, Copy)]
pub enum AlignItems {
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Debug, Clone, Copy)]
pub enum Dimension {
    Px(f32),
    Percent(f32),
    Auto,
}

#[derive(Debug, Clone, Copy)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub fn zero() -> Self {
        Self {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }
    }

    pub fn uniform(v: f32) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LayoutRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone)]
pub enum DrawCommand {
    Rect {
        rect: LayoutRect,
        color: [f32; 4],
        border_radius: f32,
        element_id: Option<String>,
        transform: [f32; 16],
        is_fixed: bool,
    },
    Border {
        rect: LayoutRect,
        color: [f32; 4],
        width: f32,
        radius: f32,
        element_id: Option<String>,
        transform: [f32; 16],
        is_fixed: bool,
    },
    Text {
        text: String,
        x: f32,
        y: f32,
        max_width: f32,
        color: [f32; 4],
        font_size: f32,
        element_id: Option<String>,
        is_fixed: bool,
        transform: [f32; 16],
    },
    Line {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        color: [f32; 4],
        width: f32,
    },
}

#[derive(Debug, Clone)]
pub enum CanvasDrawOp {
    FillRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
    },
    StrokeRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        line_width: f32,
    },
    FillCircle {
        cx: f32,
        cy: f32,
        radius: f32,
        color: [f32; 4],
    },
    StrokeCircle {
        cx: f32,
        cy: f32,
        radius: f32,
        color: [f32; 4],
        line_width: f32,
    },
    Line {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        color: [f32; 4],
        line_width: f32,
    },
}

#[derive(Clone)]
pub struct StyledNode {
    pub dom_node: DomNode,
    pub style: ComputedStyle,
    pub children: Vec<StyledNode>,
}

pub fn mat4_identity() -> [f32; 16] {
    [
        1.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    ]
}

pub fn mat4_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                // a[k, row] * b[col, k]
                // Column-major index: col * 4 + row
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = sum;
        }
    }
    out
}

pub fn mat4_translate(m: &[f32; 16], x: f32, y: f32, z: f32) -> [f32; 16] {
    let mut t = mat4_identity();
    t[12] = x;
    t[13] = y;
    t[14] = z;
    mat4_mul(m, &t)
}

pub fn mat4_rotate_x(m: &[f32; 16], rad: f32) -> [f32; 16] {
    let mut r = mat4_identity();
    let c = rad.cos();
    let s = rad.sin();
    r[5] = c;
    r[6] = s;
    r[9] = -s;
    r[10] = c;
    mat4_mul(m, &r)
}

pub fn mat4_rotate_y(m: &[f32; 16], rad: f32) -> [f32; 16] {
    let mut r = mat4_identity();
    let c = rad.cos();
    let s = rad.sin();
    r[0] = c;
    r[2] = -s;
    r[8] = s;
    r[10] = c;
    mat4_mul(m, &r)
}

pub fn mat4_perspective(m: &[f32; 16], d: f32) -> [f32; 16] {
    let mut p = mat4_identity();
    if d != 0.0 {
        p[11] = -1.0 / d; // Column 2, Row 3
    }
    mat4_mul(m, &p)
}
