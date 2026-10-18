//! Binary split tree describing how a tab's panes are arranged.

use crate::pane::PaneId;
use crate::renderer::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Children side by side (a vertical divider).
    Horizontal,
    /// Children stacked (a horizontal divider).
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug)]
pub enum Node {
    Leaf(PaneId),
    Split {
        axis: Axis,
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

/// A draggable divider between two children of a split.
#[derive(Clone, Debug)]
pub struct Divider {
    pub rect: Rect,
    pub axis: Axis,
    /// Area covered by the split, used to turn a mouse position into a ratio.
    pub area: Rect,
    /// Path of a/b choices from the root to the split.
    pub path: Vec<bool>,
}

impl Node {
    pub fn panes(&self) -> Vec<PaneId> {
        let mut v = Vec::new();
        self.collect(&mut v);
        v
    }

    fn collect(&self, v: &mut Vec<PaneId>) {
        match self {
            Node::Leaf(id) => v.push(*id),
            Node::Split { a, b, .. } => {
                a.collect(v);
                b.collect(v);
            }
        }
    }

    pub fn contains(&self, id: PaneId) -> bool {
        match self {
            Node::Leaf(p) => *p == id,
            Node::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Replace leaf `target` with a split of `target` and `new`.
    pub fn split(&mut self, target: PaneId, new: PaneId, axis: Axis) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                *self = Node::Split {
                    axis,
                    ratio: 0.5,
                    a: Box::new(Node::Leaf(target)),
                    b: Box::new(Node::Leaf(new)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split(target, new, axis) || b.split(target, new, axis),
        }
    }

    /// Swap leaf `old` for leaf `new`.
    pub fn replace(&mut self, old: PaneId, new: PaneId) -> bool {
        match self {
            Node::Leaf(id) if *id == old => {
                *id = new;
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.replace(old, new) || b.replace(old, new),
        }
    }

    /// Put `node` next to leaf `target`: before it (left/up) or after it (right/down).
    pub fn insert(&mut self, target: PaneId, node: &mut Option<Node>, dir: Dir) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                let Some(new) = node.take() else { return false };
                let (axis, before) = match dir {
                    Dir::Left => (Axis::Horizontal, true),
                    Dir::Right => (Axis::Horizontal, false),
                    Dir::Up => (Axis::Vertical, true),
                    Dir::Down => (Axis::Vertical, false),
                };
                let old = Box::new(Node::Leaf(target));
                let new = Box::new(new);
                let (a, b) = if before { (new, old) } else { (old, new) };
                *self = Node::Split { axis, ratio: 0.5, a, b };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.insert(target, node, dir) || b.insert(target, node, dir),
        }
    }

    /// Remove leaf `target`, collapsing its parent split. Returns false if this node is that leaf.
    pub fn remove(&mut self, target: PaneId) -> bool {
        let Node::Split { a, b, .. } = self else {
            return !matches!(self, Node::Leaf(id) if *id == target);
        };
        let replacement = if matches!(**a, Node::Leaf(id) if id == target) {
            Some(std::mem::replace(&mut **b, Node::Leaf(0)))
        } else if matches!(**b, Node::Leaf(id) if id == target) {
            Some(std::mem::replace(&mut **a, Node::Leaf(0)))
        } else {
            None
        };
        if let Some(r) = replacement {
            *self = r;
            return true;
        }
        a.remove(target);
        b.remove(target);
        true
    }

    pub fn layout(&self, area: Rect, gap: f32, out: &mut Vec<(PaneId, Rect)>, dividers: &mut Vec<Divider>) {
        self.layout_inner(area, gap, out, dividers, &mut Vec::new());
    }

    fn layout_inner(
        &self,
        area: Rect,
        gap: f32,
        out: &mut Vec<(PaneId, Rect)>,
        dividers: &mut Vec<Divider>,
        path: &mut Vec<bool>,
    ) {
        match self {
            Node::Leaf(id) => out.push((*id, area)),
            Node::Split { axis, ratio, a, b } => {
                let (ra, rb, div) = split_rect(area, *axis, *ratio, gap);
                dividers.push(Divider {
                    rect: div,
                    axis: *axis,
                    area,
                    path: path.clone(),
                });
                path.push(false);
                a.layout_inner(ra, gap, out, dividers, path);
                path.pop();
                path.push(true);
                b.layout_inner(rb, gap, out, dividers, path);
                path.pop();
            }
        }
    }

    pub fn set_ratio(&mut self, path: &[bool], value: f32) {
        match self {
            Node::Leaf(_) => {}
            Node::Split { ratio, a, b, .. } => match path.split_first() {
                None => *ratio = value.clamp(0.08, 0.92),
                Some((false, rest)) => a.set_ratio(rest, value),
                Some((true, rest)) => b.set_ratio(rest, value),
            },
        }
    }

    /// Move the nearest divider on `dir`'s axis that borders `target`.
    pub fn resize(&mut self, target: PaneId, dir: Dir, delta: f32) -> bool {
        let want = match dir {
            Dir::Left | Dir::Right => Axis::Horizontal,
            Dir::Up | Dir::Down => Axis::Vertical,
        };
        match self {
            Node::Leaf(_) => false,
            Node::Split { axis, ratio, a, b } => {
                let in_a = a.contains(target);
                if !in_a && !b.contains(target) {
                    return false;
                }
                let child = if in_a { a } else { b };
                if child.resize(target, dir, delta) {
                    return true;
                }
                if *axis != want {
                    return false;
                }
                let sign = if matches!(dir, Dir::Right | Dir::Down) {
                    1.0
                } else {
                    -1.0
                };
                *ratio = (*ratio + sign * delta).clamp(0.08, 0.92);
                true
            }
        }
    }
}

fn split_rect(area: Rect, axis: Axis, ratio: f32, gap: f32) -> (Rect, Rect, Rect) {
    match axis {
        Axis::Horizontal => {
            let wa = ((area.w - gap) * ratio).round();
            let a = Rect::new(area.x, area.y, wa, area.h);
            let div = Rect::new(area.x + wa, area.y, gap, area.h);
            let b = Rect::new(area.x + wa + gap, area.y, area.w - wa - gap, area.h);
            (a, b, div)
        }
        Axis::Vertical => {
            let ha = ((area.h - gap) * ratio).round();
            let a = Rect::new(area.x, area.y, area.w, ha);
            let div = Rect::new(area.x, area.y + ha, area.w, gap);
            let b = Rect::new(area.x, area.y + ha + gap, area.w, area.h - ha - gap);
            (a, b, div)
        }
    }
}

/// Pick the pane in direction `dir` from `from`, preferring the most overlap then the nearest.
pub fn neighbor(rects: &[(PaneId, Rect)], from: PaneId, dir: Dir) -> Option<PaneId> {
    let (_, src) = *rects.iter().find(|(id, _)| *id == from)?;
    let eps = 1.0;
    rects
        .iter()
        .filter(|(id, _)| *id != from)
        .filter_map(|(id, r)| {
            let (dist, overlap) = match dir {
                Dir::Left => (src.x - r.right(), overlap(src.y, src.bottom(), r.y, r.bottom())),
                Dir::Right => (r.x - src.right(), overlap(src.y, src.bottom(), r.y, r.bottom())),
                Dir::Up => (src.y - r.bottom(), overlap(src.x, src.right(), r.x, r.right())),
                Dir::Down => (r.y - src.bottom(), overlap(src.x, src.right(), r.x, r.right())),
            };
            (dist >= -eps && overlap > 0.0).then_some((*id, dist, overlap))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)))
        .map(|(id, _, _)| id)
}

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rects(n: &Node) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        n.layout(Rect::new(0.0, 0.0, 100.0, 50.0), 2.0, &mut out, &mut Vec::new());
        out
    }

    #[test]
    fn split_and_remove() {
        let mut root = Node::Leaf(1);
        assert!(root.split(1, 2, Axis::Horizontal));
        assert!(root.split(2, 3, Axis::Vertical));
        assert_eq!(root.panes(), vec![1, 2, 3]);
        let r = rects(&root);
        assert_eq!(r[0].1, Rect::new(0.0, 0.0, 49.0, 50.0));
        assert_eq!(r[1].1.x, 51.0);
        root.remove(2);
        assert_eq!(root.panes(), vec![1, 3]);
        root.remove(1);
        assert!(matches!(root, Node::Leaf(3)));
        assert!(!root.remove(3), "the root leaf can't remove itself");
    }

    #[test]
    fn insert_subtree_before_and_after() {
        let mut root = Node::Leaf(1);
        let mut sub = Some(Node::Split {
            axis: Axis::Vertical,
            ratio: 0.5,
            a: Box::new(Node::Leaf(2)),
            b: Box::new(Node::Leaf(3)),
        });
        assert!(root.insert(1, &mut sub, Dir::Left));
        assert_eq!(root.panes(), vec![2, 3, 1]);
        let mut leaf = Some(Node::Leaf(4));
        assert!(root.insert(1, &mut leaf, Dir::Down));
        assert_eq!(root.panes(), vec![2, 3, 1, 4]);
        let mut missing = Some(Node::Leaf(5));
        assert!(!root.insert(9, &mut missing, Dir::Up));
        assert!(missing.is_some(), "node is handed back when the target doesn't exist");
    }

    #[test]
    fn directional_focus() {
        let mut root = Node::Leaf(1);
        root.split(1, 2, Axis::Horizontal);
        root.split(2, 3, Axis::Vertical);
        let r = rects(&root);
        assert_eq!(neighbor(&r, 1, Dir::Right), Some(2));
        assert_eq!(neighbor(&r, 3, Dir::Up), Some(2));
        assert_eq!(neighbor(&r, 3, Dir::Left), Some(1));
        assert_eq!(neighbor(&r, 1, Dir::Left), None);
    }

    #[test]
    fn resize_moves_matching_divider() {
        let mut root = Node::Leaf(1);
        root.split(1, 2, Axis::Horizontal);
        root.split(2, 3, Axis::Vertical);
        assert!(root.resize(3, Dir::Left, 0.1));
        let Node::Split { ratio, .. } = &root else { panic!() };
        assert!((*ratio - 0.4).abs() < 1e-6);
        assert!(root.resize(3, Dir::Up, 0.1));
        let Node::Split { b, .. } = &root else { panic!() };
        let Node::Split { ratio, .. } = &**b else { panic!() };
        assert!((*ratio - 0.4).abs() < 1e-6);
    }
}
