// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Binary Space Partitioning layout tree.

use ratatui::layout::Rect;

/// Split direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    Horizontal,
    Vertical,
}

/// A node in the BSP tree.
#[derive(Debug, Clone)]
enum LayoutNode {
    Pane {
        id: u32,
    },
    Split {
        direction: Split,
        ratio: f32,
        left: Box<LayoutNode>,
        right: Box<LayoutNode>,
    },
}

/// BSP layout manager.
pub struct BspLayout {
    root: LayoutNode,
    next_id: u32,
    pane_count: usize,
}

impl Default for BspLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl BspLayout {
    pub fn new() -> Self {
        Self::with_root_id(0)
    }

    /// Create a BSP layout with a specific root pane ID.
    pub fn with_root_id(root_id: u32) -> Self {
        Self {
            root: LayoutNode::Pane { id: root_id },
            next_id: root_id + 1,
            pane_count: 1,
        }
    }

    pub fn split(&mut self, pane_id: u32, direction: Split, ratio: f32) -> u32 {
        let new_id = self.next_id;
        self.next_id += 1;
        self.root = self.split_node(self.root.clone(), pane_id, direction, ratio, new_id);
        self.pane_count += 1;
        new_id
    }

    pub fn remove(&mut self, pane_id: u32) {
        if let Some(remaining) = self.remove_node(self.root.clone(), pane_id) {
            self.root = remaining;
            self.pane_count -= 1;
        }
    }

    pub fn pane_count(&self) -> usize {
        self.pane_count
    }

    pub fn compute_layout(&self, area: Rect) -> Vec<(u32, Rect)> {
        let mut result = Vec::new();
        self.compute_node(&self.root, area, &mut result);
        result
    }

    fn compute_node(&self, node: &LayoutNode, area: Rect, out: &mut Vec<(u32, Rect)>) {
        match node {
            LayoutNode::Pane { id } => {
                out.push((*id, area));
            }
            LayoutNode::Split {
                direction,
                ratio,
                left,
                right,
            } => {
                let (left_rect, right_rect) = match direction {
                    Split::Horizontal => {
                        let split = (area.width as f32 * ratio) as u16;
                        (
                            Rect {
                                width: split,
                                ..area
                            },
                            Rect {
                                x: area.x + split,
                                width: area.width - split,
                                ..area
                            },
                        )
                    }
                    Split::Vertical => {
                        let split = (area.height as f32 * ratio) as u16;
                        (
                            Rect {
                                height: split,
                                ..area
                            },
                            Rect {
                                y: area.y + split,
                                height: area.height - split,
                                ..area
                            },
                        )
                    }
                };
                self.compute_node(left, left_rect, out);
                self.compute_node(right, right_rect, out);
            }
        }
    }

    fn split_node(
        &self,
        node: LayoutNode,
        pane_id: u32,
        direction: Split,
        ratio: f32,
        new_id: u32,
    ) -> LayoutNode {
        match node {
            LayoutNode::Pane { id } if id == pane_id => LayoutNode::Split {
                direction,
                ratio,
                left: Box::new(LayoutNode::Pane { id }),
                right: Box::new(LayoutNode::Pane { id: new_id }),
            },
            LayoutNode::Split {
                direction: d,
                ratio: r,
                left,
                right,
            } => LayoutNode::Split {
                direction: d,
                ratio: r,
                left: Box::new(self.split_node(*left, pane_id, direction, ratio, new_id)),
                right: Box::new(self.split_node(*right, pane_id, direction, ratio, new_id)),
            },
            other => other,
        }
    }

    fn remove_node(&self, node: LayoutNode, pane_id: u32) -> Option<LayoutNode> {
        match node {
            LayoutNode::Pane { id } if id == pane_id => None,
            LayoutNode::Pane { .. } => Some(node),
            LayoutNode::Split {
                direction,
                ratio,
                left,
                right,
            } => {
                let l = self.remove_node(*left, pane_id);
                let r = self.remove_node(*right, pane_id);
                match (l, r) {
                    (Some(n), None) | (None, Some(n)) => Some(n),
                    (Some(l), Some(r)) => Some(LayoutNode::Split {
                        direction,
                        ratio,
                        left: Box::new(l),
                        right: Box::new(r),
                    }),
                    (None, None) => None,
                }
            }
        }
    }
}
