//! Pure layout engines: BSP, Dwindle and Scrolling. No platform calls.

mod engine;
mod scrolling;

pub use engine::{LayoutSettings, LayoutSummary, TilingLayout};
pub use scrolling::{ColumnPlacement, ScrollingLayout, ScrollingMode, ScrollingView};

use std::collections::HashMap;
use tylora_core::{
    commands::Direction,
    coordinator::LayoutPolicy,
    layout::{DwindleSettings, FocusOrigin, SplitOrientation},
    registry::ManagedWindowId,
    Rect, WindowId,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub id: WindowId,
    pub frame: Rect,
}

/// Stage-one compatibility API.
pub fn preserve_positions(windows: impl IntoIterator<Item = Placement>) -> Vec<Placement> {
    windows.into_iter().collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    InvalidRatio,
    NodeNotFound,
    InvalidGap,
}

#[derive(Debug, Clone)]
enum Node {
    Leaf(ManagedWindowId),
    Split {
        id: NodeId,
        axis: Axis,
        ratio: f64,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// How a new window enters the tree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InsertStrategy {
    /// Original Tylora BSP: descend the second child, alternating by depth.
    Spiral,
    /// Hyprland-style dwindle: split the focused window's leaf.
    Dwindle(DwindleSettings),
}

#[derive(Debug, Clone)]
pub struct BspLayout {
    root: Option<Node>,
    next_node: u64,
    inner_gap: f64,
    outer_gap: f64,
    min_width: f64,
    min_height: f64,
    window_min_size: HashMap<ManagedWindowId, (f64, f64)>,
    strategy: InsertStrategy,
    focused: Option<ManagedWindowId>,
}

impl Default for BspLayout {
    fn default() -> Self {
        Self {
            root: None,
            next_node: 0,
            inner_gap: 8.0,
            outer_gap: 8.0,
            min_width: 80.0,
            min_height: 60.0,
            window_min_size: HashMap::new(),
            strategy: InsertStrategy::Spiral,
            focused: None,
        }
    }
}

impl BspLayout {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn dwindle(settings: DwindleSettings) -> Self {
        Self {
            strategy: InsertStrategy::Dwindle(settings),
            ..Self::default()
        }
    }
    pub fn strategy(&self) -> InsertStrategy {
        self.strategy
    }
    /// Changes future insertions only; existing splits keep their ratios.
    pub fn set_dwindle_settings(&mut self, settings: DwindleSettings) {
        if let InsertStrategy::Dwindle(current) = &mut self.strategy {
            *current = settings;
        }
    }
    /// Anchor for the next dwindle insertion.
    pub fn set_focused(&mut self, id: Option<ManagedWindowId>) {
        self.focused = id.filter(|id| self.contains(*id));
    }
    pub fn focused(&self) -> Option<ManagedWindowId> {
        self.focused
    }
    pub fn contains(&self, id: ManagedWindowId) -> bool {
        fn find(node: &Node, id: ManagedWindowId) -> bool {
            match node {
                Node::Leaf(found) => *found == id,
                Node::Split { first, second, .. } => find(first, id) || find(second, id),
            }
        }
        self.root.as_ref().is_some_and(|root| find(root, id))
    }
    pub fn len(&self) -> usize {
        self.windows().len()
    }
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }
    pub fn windows(&self) -> Vec<ManagedWindowId> {
        fn collect(node: &Node, out: &mut Vec<ManagedWindowId>) {
            match node {
                Node::Leaf(id) => out.push(*id),
                Node::Split { first, second, .. } => {
                    collect(first, out);
                    collect(second, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(node) = &self.root {
            collect(node, &mut out);
        }
        out
    }

    pub fn splits(&self) -> Vec<(NodeId, Axis, f64)> {
        fn collect(node: &Node, out: &mut Vec<(NodeId, Axis, f64)>) {
            if let Node::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } = node
            {
                out.push((*id, *axis, *ratio));
                collect(first, out);
                collect(second, out);
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            collect(root, &mut out);
        }
        out
    }

    pub fn insert_window(&mut self, id: ManagedWindowId) -> bool {
        if self.contains(id) {
            return false;
        }
        if let InsertStrategy::Dwindle(settings) = self.strategy {
            self.insert_dwindle(id, settings);
            return true;
        }
        self.next_node = self.next_node.wrapping_add(1);
        fn insert(node: &mut Node, id: ManagedWindowId, split_id: NodeId, depth: usize) {
            match node {
                Node::Leaf(_) => {
                    let old = std::mem::replace(node, Node::Leaf(id));
                    *node = Node::Split {
                        id: split_id,
                        axis: if depth % 2 == 0 {
                            Axis::Vertical
                        } else {
                            Axis::Horizontal
                        },
                        ratio: 0.5,
                        first: Box::new(old),
                        second: Box::new(Node::Leaf(id)),
                    };
                }
                Node::Split { second, .. } => insert(second, id, split_id, depth + 1),
            }
        }
        if let Some(root) = &mut self.root {
            insert(root, id, NodeId(self.next_node), 0);
        } else {
            self.root = Some(Node::Leaf(id));
        }
        true
    }

    pub fn remove_window(&mut self, id: ManagedWindowId) -> bool {
        fn remove(node: Node, id: ManagedWindowId) -> (Option<Node>, bool) {
            match node {
                Node::Leaf(found) => {
                    if found == id {
                        (None, true)
                    } else {
                        (Some(Node::Leaf(found)), false)
                    }
                }
                Node::Split {
                    id: split_id,
                    axis,
                    ratio,
                    first,
                    second,
                } => {
                    let (first, removed_first) = remove(*first, id);
                    let (second, removed_second) = remove(*second, id);
                    let next = match (first, second) {
                        (Some(a), Some(b)) => Some(Node::Split {
                            id: split_id,
                            axis,
                            ratio,
                            first: Box::new(a),
                            second: Box::new(b),
                        }),
                        (Some(a), None) | (None, Some(a)) => Some(a),
                        (None, None) => None,
                    };
                    (next, removed_first || removed_second)
                }
            }
        }
        let Some(root) = self.root.take() else {
            return false;
        };
        let (root, removed) = remove(root, id);
        self.root = root;
        if removed {
            self.window_min_size.remove(&id);
            if self.focused == Some(id) {
                self.focused = None;
            }
        }
        removed
    }

    /// Splits the anchor leaf (focused window, else the last window). The
    /// anchor keeps the first half; the axis alternates with its parent split.
    fn insert_dwindle(&mut self, id: ManagedWindowId, settings: DwindleSettings) {
        let Some(root) = self.root.as_mut() else {
            self.root = Some(Node::Leaf(id));
            self.focused = Some(id);
            return;
        };
        let anchor = self
            .focused
            .or_else(|| {
                let mut windows = Vec::new();
                fn last(node: &Node, out: &mut Vec<ManagedWindowId>) {
                    match node {
                        Node::Leaf(id) => out.push(*id),
                        Node::Split { first, second, .. } => {
                            last(first, out);
                            last(second, out);
                        }
                    }
                }
                last(root, &mut windows);
                windows.last().copied()
            })
            .expect("nonempty tree has a leaf");
        self.next_node = self.next_node.wrapping_add(1);
        let split_id = NodeId(self.next_node);
        let initial = match settings.initial_orientation {
            SplitOrientation::Vertical => Axis::Vertical,
            SplitOrientation::Horizontal => Axis::Horizontal,
        };
        fn split(
            node: &mut Node,
            anchor: ManagedWindowId,
            id: ManagedWindowId,
            parent: Option<Axis>,
            initial: Axis,
            split_id: NodeId,
            ratio: f64,
        ) -> bool {
            match node {
                Node::Leaf(found) if *found == anchor => {
                    let axis = match parent {
                        Some(Axis::Vertical) => Axis::Horizontal,
                        Some(Axis::Horizontal) => Axis::Vertical,
                        None => initial,
                    };
                    let old = std::mem::replace(node, Node::Leaf(id));
                    *node = Node::Split {
                        id: split_id,
                        axis,
                        ratio,
                        first: Box::new(old),
                        second: Box::new(Node::Leaf(id)),
                    };
                    true
                }
                Node::Leaf(_) => false,
                Node::Split {
                    axis,
                    first,
                    second,
                    ..
                } => {
                    let axis = *axis;
                    split(first, anchor, id, Some(axis), initial, split_id, ratio)
                        || split(second, anchor, id, Some(axis), initial, split_id, ratio)
                }
            }
        }
        let inserted = split(
            root,
            anchor,
            id,
            None,
            initial,
            split_id,
            settings.split_ratio.clamp(0.1, 0.9),
        );
        debug_assert!(inserted, "dwindle anchor must be a leaf");
        self.focused = Some(id);
    }

    /// Records a minimum inferred from a window that repeatedly refused a
    /// smaller AX geometry. Bounds only grow until the window leaves the tree.
    pub fn set_window_min_size(&mut self, id: ManagedWindowId, width: f64, height: f64) -> bool {
        if !width.is_finite()
            || !height.is_finite()
            || width < 0.0
            || height < 0.0
            || !self.contains(id)
        {
            return false;
        }
        let current = self.window_min_size.entry(id).or_insert((0.0, 0.0));
        let next = (current.0.max(width), current.1.max(height));
        if next == *current {
            return false;
        }
        *current = next;
        true
    }

    pub fn set_gaps(&mut self, inner: f64, outer: f64) -> Result<(), LayoutError> {
        if !inner.is_finite() || !outer.is_finite() || inner < 0.0 || outer < 0.0 {
            return Err(LayoutError::InvalidGap);
        }
        self.inner_gap = inner;
        self.outer_gap = outer;
        Ok(())
    }
    pub fn set_min_size(&mut self, width: f64, height: f64) -> Result<(), LayoutError> {
        if !width.is_finite() || !height.is_finite() || width < 0.0 || height < 0.0 {
            return Err(LayoutError::InvalidGap);
        }
        self.min_width = width;
        self.min_height = height;
        Ok(())
    }

    pub fn resize_split(&mut self, id: NodeId, ratio: f64) -> Result<(), LayoutError> {
        if !ratio.is_finite() || !(0.0..1.0).contains(&ratio) {
            return Err(LayoutError::InvalidRatio);
        }
        fn find(node: &mut Node, id: NodeId, ratio: f64) -> bool {
            match node {
                Node::Leaf(_) => false,
                Node::Split {
                    id: found,
                    ratio: current,
                    first,
                    second,
                    ..
                } => {
                    if *found == id {
                        *current = ratio;
                        true
                    } else {
                        find(first, id, ratio) || find(second, id, ratio)
                    }
                }
            }
        }
        if self.root.as_mut().is_some_and(|root| find(root, id, ratio)) {
            Ok(())
        } else {
            Err(LayoutError::NodeNotFound)
        }
    }

    pub fn swap_windows(&mut self, first: ManagedWindowId, second: ManagedWindowId) -> bool {
        if first == second || !self.contains(first) || !self.contains(second) {
            return false;
        }
        fn swap(node: &mut Node, a: ManagedWindowId, b: ManagedWindowId) {
            match node {
                Node::Leaf(id) if *id == a => *id = b,
                Node::Leaf(id) if *id == b => *id = a,
                Node::Leaf(_) => {}
                Node::Split { first, second, .. } => {
                    swap(first, a, b);
                    swap(second, a, b);
                }
            }
        }
        if let Some(root) = &mut self.root {
            swap(root, first, second);
        }
        true
    }

    /// Renames a leaf, keeping its place in the tree and every split ratio.
    pub fn replace_window(&mut self, old: ManagedWindowId, new: ManagedWindowId) -> bool {
        if old == new || !self.contains(old) || self.contains(new) {
            return false;
        }
        fn rename(node: &mut Node, old: ManagedWindowId, new: ManagedWindowId) {
            match node {
                Node::Leaf(id) if *id == old => *id = new,
                Node::Leaf(_) => {}
                Node::Split { first, second, .. } => {
                    rename(first, old, new);
                    rename(second, old, new);
                }
            }
        }
        if let Some(root) = &mut self.root {
            rename(root, old, new);
        }
        if let Some(size) = self.window_min_size.remove(&old) {
            self.window_min_size.insert(new, size);
        }
        if self.focused == Some(old) {
            self.focused = Some(new);
        }
        true
    }

    /// Moves the closest BSP boundary on the requested side of a leaf.
    pub fn resize_window(
        &mut self,
        id: ManagedWindowId,
        direction: Direction,
        amount: f64,
        area: Rect,
    ) -> bool {
        if !amount.is_finite() || amount <= 0.0 {
            return false;
        }
        let axis = match direction {
            Direction::Left | Direction::Right => Axis::Vertical,
            Direction::Up | Direction::Down => Axis::Horizontal,
        };
        let want_first = matches!(direction, Direction::Right | Direction::Down);
        fn contains(node: &Node, id: ManagedWindowId) -> bool {
            match node {
                Node::Leaf(found) => *found == id,
                Node::Split { first, second, .. } => contains(first, id) || contains(second, id),
            }
        }
        fn resize(
            node: &mut Node,
            id: ManagedWindowId,
            axis: Axis,
            want_first: bool,
            amount: f64,
            rect: Rect,
            gap: f64,
        ) -> bool {
            let Node::Split {
                axis: split_axis,
                ratio,
                first,
                second,
                ..
            } = node
            else {
                return false;
            };
            let in_first = contains(first, id);
            let in_second = !in_first && contains(second, id);
            if !in_first && !in_second {
                return false;
            }
            let length = if *split_axis == Axis::Vertical {
                rect.width
            } else {
                rect.height
            };
            let available = (length - gap.min(length)).max(1.0);
            let first_length = available * *ratio;
            let first_rect = if *split_axis == Axis::Vertical {
                Rect::new(rect.x, rect.y, first_length, rect.height).unwrap()
            } else {
                Rect::new(rect.x, rect.y, rect.width, first_length).unwrap()
            };
            let second_rect = if *split_axis == Axis::Vertical {
                Rect::new(
                    rect.x + first_length + gap,
                    rect.y,
                    (available - first_length).max(0.0),
                    rect.height,
                )
                .unwrap()
            } else {
                Rect::new(
                    rect.x,
                    rect.y + first_length + gap,
                    rect.width,
                    (available - first_length).max(0.0),
                )
                .unwrap()
            };
            if in_first && resize(first, id, axis, want_first, amount, first_rect, gap) {
                return true;
            }
            if in_second && resize(second, id, axis, want_first, amount, second_rect, gap) {
                return true;
            }
            if *split_axis != axis || in_first != want_first {
                return false;
            }
            let delta = (amount / available) * if in_first { 1.0 } else { -1.0 };
            let next = (*ratio + delta).clamp(0.05, 0.95);
            if (next - *ratio).abs() < 1e-6 {
                return false;
            }
            *ratio = next;
            true
        }
        let Some(root) = &mut self.root else {
            return false;
        };
        let bounds = Rect::new(
            area.x + self.outer_gap,
            area.y + self.outer_gap,
            (area.width - 2.0 * self.outer_gap).max(0.0),
            (area.height - 2.0 * self.outer_gap).max(0.0),
        )
        .unwrap();
        resize(root, id, axis, want_first, amount, bounds, self.inner_gap)
    }

    pub fn calculate_layout(&self, area: Rect) -> Vec<(ManagedWindowId, Rect)> {
        let mut out = Vec::new();
        let Some(root) = &self.root else {
            return out;
        };
        let bounds = Rect::new(
            area.x + self.outer_gap.min(area.width / 2.0),
            area.y + self.outer_gap.min(area.height / 2.0),
            (area.width - 2.0 * self.outer_gap).max(0.0),
            (area.height - 2.0 * self.outer_gap).max(0.0),
        )
        .expect("valid inset");
        fn required(
            node: &Node,
            gap: f64,
            min_w: f64,
            min_h: f64,
            window_min: &HashMap<ManagedWindowId, (f64, f64)>,
            split_min: &mut HashMap<NodeId, (f64, f64)>,
        ) -> (f64, f64) {
            match node {
                Node::Leaf(id) => {
                    let (width, height) = window_min.get(id).copied().unwrap_or((0.0, 0.0));
                    (min_w.max(width), min_h.max(height))
                }
                Node::Split {
                    id,
                    axis,
                    first,
                    second,
                    ..
                } => {
                    let a = required(first, gap, min_w, min_h, window_min, split_min);
                    let b = required(second, gap, min_w, min_h, window_min, split_min);
                    let size = match axis {
                        Axis::Vertical => (a.0 + gap + b.0, a.1.max(b.1)),
                        Axis::Horizontal => (a.0.max(b.0), a.1 + gap + b.1),
                    };
                    split_min.insert(*id, size);
                    size
                }
            }
        }
        let mut split_min = HashMap::new();
        required(
            root,
            self.inner_gap,
            self.min_width,
            self.min_height,
            &self.window_min_size,
            &mut split_min,
        );
        struct Measure<'a> {
            gap: f64,
            min_w: f64,
            min_h: f64,
            window_min: &'a HashMap<ManagedWindowId, (f64, f64)>,
            split_min: &'a HashMap<NodeId, (f64, f64)>,
        }
        fn visit(
            node: &Node,
            rect: Rect,
            measure: &Measure<'_>,
            out: &mut Vec<(ManagedWindowId, Rect)>,
        ) {
            match node {
                Node::Leaf(id) => out.push((*id, rect)),
                Node::Split {
                    axis,
                    ratio,
                    first,
                    second,
                    ..
                } => {
                    let minimum = |node: &Node| match node {
                        Node::Leaf(id) => {
                            let (w, h) = measure.window_min.get(id).copied().unwrap_or((0.0, 0.0));
                            (w.max(measure.min_w), h.max(measure.min_h))
                        }
                        Node::Split { id, .. } => measure.split_min[id],
                    };
                    let (length, first_min, second_min) = match axis {
                        Axis::Vertical => (rect.width, minimum(first).0, minimum(second).0),
                        Axis::Horizontal => (rect.height, minimum(first).1, minimum(second).1),
                    };
                    let gap = measure.gap.min(length);
                    let available = (length - gap).max(0.0);
                    let first_len = if available >= first_min + second_min {
                        (available * ratio).clamp(first_min, available - second_min)
                    } else {
                        available * ratio
                    };
                    let second_len = available - first_len;
                    let (a, b) = match axis {
                        Axis::Vertical => (
                            Rect::new(rect.x, rect.y, first_len, rect.height).unwrap(),
                            Rect::new(rect.x + first_len + gap, rect.y, second_len, rect.height)
                                .unwrap(),
                        ),
                        Axis::Horizontal => (
                            Rect::new(rect.x, rect.y, rect.width, first_len).unwrap(),
                            Rect::new(rect.x, rect.y + first_len + gap, rect.width, second_len)
                                .unwrap(),
                        ),
                    };
                    visit(first, a, measure, out);
                    visit(second, b, measure, out);
                }
            }
        }
        let measure = Measure {
            gap: self.inner_gap,
            min_w: self.min_width,
            min_h: self.min_height,
            window_min: &self.window_min_size,
            split_min: &split_min,
        };
        visit(root, bounds, &measure, &mut out);
        out
    }
}

impl LayoutPolicy for BspLayout {
    fn insert_window(&mut self, id: ManagedWindowId) {
        BspLayout::insert_window(self, id);
    }
    fn remove_window(&mut self, id: ManagedWindowId) {
        BspLayout::remove_window(self, id);
    }
    fn calculate_layout(&self, area: Rect) -> Vec<(ManagedWindowId, Rect)> {
        BspLayout::calculate_layout(self, area)
    }
    fn set_window_min_size(&mut self, id: ManagedWindowId, width: f64, height: f64) -> bool {
        BspLayout::set_window_min_size(self, id, width, height)
    }
    fn swap_windows(&mut self, first: ManagedWindowId, second: ManagedWindowId) -> bool {
        BspLayout::swap_windows(self, first, second)
    }
    fn resize_window(
        &mut self,
        id: ManagedWindowId,
        direction: Direction,
        amount: f64,
        area: Rect,
    ) -> bool {
        BspLayout::resize_window(self, id, direction, amount, area)
    }
    fn order(&self) -> Vec<ManagedWindowId> {
        self.windows()
    }
    fn focus_changed(&mut self, id: ManagedWindowId, _origin: FocusOrigin, _area: Rect) -> bool {
        // Only the insertion anchor changes; geometry does not.
        self.set_focused(Some(id));
        false
    }
    fn replace_window(&mut self, old: ManagedWindowId, new: ManagedWindowId) -> bool {
        BspLayout::replace_window(self, old, new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tylora_core::registry::ProcessInstance;
    fn id(n: u64) -> ManagedWindowId {
        ManagedWindowId {
            process: ProcessInstance { pid: 1, epoch: 1 },
            token: n,
        }
    }
    fn area() -> Rect {
        Rect::new(-100.0, 20.0, 1000.0, 600.0).unwrap()
    }

    #[test]
    fn duplicate_insert_and_repeated_remove() {
        let mut tree = BspLayout::new();
        assert!(tree.insert_window(id(1)));
        assert!(!tree.insert_window(id(1)));
        assert!(tree.remove_window(id(1)));
        assert!(!tree.remove_window(id(1)));
        assert!(tree.is_empty());
    }
    #[test]
    fn split_collapse_and_swap() {
        let mut tree = BspLayout::new();
        for n in 1..=3 {
            tree.insert_window(id(n));
        }
        assert_eq!(tree.windows(), vec![id(1), id(2), id(3)]);
        assert!(tree.swap_windows(id(1), id(3)));
        assert_eq!(tree.windows(), vec![id(3), id(2), id(1)]);
        assert!(tree.remove_window(id(2)));
        assert_eq!(tree.windows(), vec![id(3), id(1)]);
    }
    #[test]
    fn directional_resize_changes_nearest_split_and_preserves_bounds() {
        let mut tree = BspLayout::new();
        tree.insert_window(id(1));
        tree.insert_window(id(2));
        let before = tree.calculate_layout(area());
        assert!(tree.resize_window(id(1), Direction::Right, 50.0, area()));
        let after = tree.calculate_layout(area());
        assert!(after[0].1.width > before[0].1.width);
        assert!(after[1].1.width < before[1].1.width);
        assert!(!tree.resize_window(id(1), Direction::Left, 50.0, area()));
        assert!(tree.splits()[0].2 > 0.5);
    }
    #[test]
    fn many_insert_remove_stays_in_bounds() {
        let mut tree = BspLayout::new();
        for n in 0..64 {
            tree.insert_window(id(n));
        }
        for n in (0..64).step_by(3) {
            tree.remove_window(id(n));
        }
        let placements = tree.calculate_layout(area());
        assert_eq!(placements.len(), tree.len());
        let unique: std::collections::HashSet<_> = placements.iter().map(|(id, _)| id).collect();
        assert_eq!(unique.len(), placements.len());
        for (_, r) in placements {
            assert!(r.x >= -100.0 && r.y >= 20.0);
            assert!(r.x + r.width <= 900.0 + 1e-8);
            assert!(r.y + r.height <= 620.0 + 1e-8);
            assert!(r.width >= 0.0 && r.height >= 0.0);
        }
    }
    #[test]
    fn screen_resize_and_ratio() {
        let mut tree = BspLayout::new();
        tree.insert_window(id(1));
        tree.insert_window(id(2));
        tree.resize_split(NodeId(2), 0.7).unwrap();
        let wide = tree.calculate_layout(area());
        let narrow = tree.calculate_layout(Rect::new(0.0, 0.0, 300.0, 200.0).unwrap());
        assert!(wide[0].1.width > narrow[0].1.width);
    }

    #[test]
    fn gaps_and_minimums_are_applied_when_feasible() {
        let mut tree = BspLayout::new();
        tree.set_gaps(20.0, 30.0).unwrap();
        tree.set_min_size(150.0, 80.0).unwrap();
        tree.insert_window(id(1));
        tree.insert_window(id(2));
        tree.resize_split(NodeId(2), 0.05).unwrap();
        let frames = tree.calculate_layout(Rect::new(0.0, 0.0, 600.0, 400.0).unwrap());
        assert_eq!(frames[0].1.x, 30.0);
        assert_eq!(frames[0].1.width, 150.0);
        assert_eq!(frames[1].1.x - (frames[0].1.x + frames[0].1.width), 20.0);
        assert!(frames.iter().all(|(_, frame)| frame.width >= 150.0));
    }

    #[test]
    fn learned_window_minimum_reflows_sibling_and_clears_on_remove() {
        let mut tree = BspLayout::new();
        for n in 1..=3 {
            tree.insert_window(id(n));
        }
        let before = tree.calculate_layout(area());
        assert!(tree.set_window_min_size(id(2), 0.0, 400.0));
        assert!(!tree.set_window_min_size(id(2), 0.0, 400.0));
        let after = tree.calculate_layout(area());
        assert!(after[1].1.height >= 400.0);
        assert!(after[2].1.y >= after[1].1.y + after[1].1.height + 8.0);
        assert!(after[2].1.height < before[2].1.height);
        tree.remove_window(id(2));
        tree.insert_window(id(2));
        let reset = tree.calculate_layout(area());
        assert!(
            reset
                .iter()
                .find(|(window, _)| *window == id(2))
                .unwrap()
                .1
                .height
                < 400.0
        );
    }

    #[test]
    fn sequence_invariants_under_churn() {
        let mut tree = BspLayout::new();
        let mut seed = 0x9e37_79b9_u64;
        for _ in 0..300 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let candidate = id((seed >> 32) % 40);
            if seed & 1 == 0 {
                tree.insert_window(candidate);
            } else {
                tree.remove_window(candidate);
            }
            let rect = Rect::new(-800.0, -300.0, 1920.0, 1080.0).unwrap();
            let placements = tree.calculate_layout(rect);
            let unique: std::collections::HashSet<_> =
                placements.iter().map(|(id, _)| id).collect();
            assert_eq!(unique.len(), placements.len());
            for (_, frame) in placements {
                assert!(frame.width >= 0.0 && frame.height >= 0.0);
                assert!(frame.x >= rect.x - 1e-8 && frame.y >= rect.y - 1e-8);
                assert!(frame.x + frame.width <= rect.x + rect.width + 1e-8);
                assert!(frame.y + frame.height <= rect.y + rect.height + 1e-8);
            }
        }
    }
}
