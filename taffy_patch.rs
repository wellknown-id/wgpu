use std::collections::HashMap;

enum DomNode {
    Document { children: Vec<DomNode> },
    Element { tag_name: String, attributes: HashMap<String, String>, children: Vec<DomNode> },
    Text(String),
}

// wait, I can just append `taffy_patch.rs` contents directly inside `GpuState::render` or as global functions in `main.rs`!
