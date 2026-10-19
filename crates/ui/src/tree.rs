//! A small retained view tree: absolute rectangles, hit testing (deepest node, later
//! siblings on top), pointer capture, keyboard focus order and dirty tracking.

use vello::kurbo::{Point, Rect};

/// Index of a node in a [`ViewTree`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone)]
struct Node<T> {
    rect: Rect,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    data: T,
    focusable: bool,
    dirty: bool,
}

/// A retained tree of rectangles carrying `T`.
#[derive(Debug, Clone)]
pub struct ViewTree<T> {
    nodes: Vec<Node<T>>,
    captured: Option<NodeId>,
    focused: Option<NodeId>,
}

impl<T> ViewTree<T> {
    /// A tree with only a root.
    pub fn new(root_rect: Rect, data: T) -> Self {
        Self {
            nodes: vec![Node {
                rect: root_rect,
                parent: None,
                children: Vec::new(),
                data,
                focusable: false,
                dirty: false,
            }],
            captured: None,
            focused: None,
        }
    }

    /// The root node.
    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    /// Number of nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True when only the root exists.
    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    /// Adds a child of `parent`; later children are on top for hit testing.
    pub fn add(&mut self, parent: NodeId, rect: Rect, data: T, focusable: bool) -> NodeId {
        let id = NodeId(u32::try_from(self.nodes.len()).expect("fewer than 2^32 nodes"));
        self.nodes.push(Node {
            rect,
            parent: Some(parent),
            children: Vec::new(),
            data,
            focusable,
            dirty: false,
        });
        self.nodes[parent.0 as usize].children.push(id);
        id
    }

    /// A node's rectangle.
    pub fn rect(&self, id: NodeId) -> Rect {
        self.nodes[id.0 as usize].rect
    }

    /// A node's data.
    pub fn data(&self, id: NodeId) -> &T {
        &self.nodes[id.0 as usize].data
    }

    /// A node's parent.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0 as usize].parent
    }

    /// The first node (in tree order) whose data matches.
    pub fn find(&self, pred: impl Fn(&T) -> bool) -> Option<NodeId> {
        self.nodes
            .iter()
            .position(|n| pred(&n.data))
            .map(|i| NodeId(i as u32))
    }

    /// The deepest node containing `p` (last-added sibling wins), or `None` outside the root.
    pub fn hit_test(&self, p: Point) -> Option<NodeId> {
        let root = self.root();
        if !self.rect(root).contains(p) {
            return None;
        }
        let mut current = root;
        'descend: loop {
            for &child in self.nodes[current.0 as usize].children.iter().rev() {
                if self.rect(child).contains(p) {
                    current = child;
                    continue 'descend;
                }
            }
            return Some(current);
        }
    }

    /// Captures the pointer: [`Self::target`] returns `id` until released.
    pub fn set_capture(&mut self, id: NodeId) {
        self.captured = Some(id);
    }

    /// Releases pointer capture.
    pub fn release_capture(&mut self) {
        self.captured = None;
    }

    /// The node holding pointer capture.
    pub fn captured(&self) -> Option<NodeId> {
        self.captured
    }

    /// The captured node, else the hit node.
    pub fn target(&self, p: Point) -> Option<NodeId> {
        self.captured.or_else(|| self.hit_test(p))
    }

    /// The focused node.
    pub fn focused(&self) -> Option<NodeId> {
        self.focused
    }

    /// Sets keyboard focus (ignored for non-focusable nodes).
    pub fn set_focus(&mut self, id: Option<NodeId>) {
        match id {
            None => self.focused = None,
            Some(id) if self.nodes[id.0 as usize].focusable => self.focused = Some(id),
            Some(_) => {}
        }
    }

    /// Focusable nodes in tree (depth-first) order.
    fn focus_order(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![self.root()];
        while let Some(id) = stack.pop() {
            let n = &self.nodes[id.0 as usize];
            if n.focusable {
                out.push(id);
            }
            stack.extend(n.children.iter().rev());
        }
        out
    }

    fn step_focus(&mut self, forward: bool) -> Option<NodeId> {
        let order = self.focus_order();
        if order.is_empty() {
            self.focused = None;
            return None;
        }
        let pos = self
            .focused
            .and_then(|f| order.iter().position(|&n| n == f));
        let next = match (pos, forward) {
            (None, true) => 0,
            (None, false) => order.len() - 1,
            (Some(i), true) => (i + 1) % order.len(),
            (Some(i), false) => (i + order.len() - 1) % order.len(),
        };
        self.focused = Some(order[next]);
        self.focused
    }

    /// Moves focus to the next focusable node in tree order (wrapping).
    pub fn focus_next(&mut self) -> Option<NodeId> {
        self.step_focus(true)
    }

    /// Moves focus to the previous focusable node in tree order (wrapping).
    pub fn focus_prev(&mut self) -> Option<NodeId> {
        self.step_focus(false)
    }

    /// Marks a node and its ancestors dirty.
    pub fn mark_dirty(&mut self, id: NodeId) {
        let mut cur = Some(id);
        while let Some(i) = cur {
            let n = &mut self.nodes[i.0 as usize];
            n.dirty = true;
            cur = n.parent;
        }
    }

    /// Whether a node is dirty.
    pub fn is_dirty(&self, id: NodeId) -> bool {
        self.nodes[id.0 as usize].dirty
    }

    /// Clears every dirty flag.
    pub fn clear_dirty(&mut self) {
        for n in &mut self.nodes {
            n.dirty = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> (ViewTree<&'static str>, [NodeId; 4]) {
        let mut t = ViewTree::new(Rect::new(0.0, 0.0, 100.0, 100.0), "root");
        let a = t.add(t.root(), Rect::new(0.0, 0.0, 50.0, 100.0), "a", false);
        let a1 = t.add(a, Rect::new(10.0, 10.0, 30.0, 30.0), "a1", true);
        let b = t.add(t.root(), Rect::new(40.0, 0.0, 100.0, 100.0), "b", false);
        let b1 = t.add(b, Rect::new(60.0, 60.0, 80.0, 80.0), "b1", true);
        (t, [a, a1, b, b1])
    }

    #[test]
    fn hit_test_finds_deepest_and_topmost() {
        let (t, [a, a1, b, b1]) = tree();
        assert_eq!(t.hit_test(Point::new(15.0, 15.0)), Some(a1));
        assert_eq!(t.hit_test(Point::new(5.0, 50.0)), Some(a));
        // Overlap of a and b: b was added later, so it is on top.
        assert_eq!(t.hit_test(Point::new(45.0, 50.0)), Some(b));
        assert_eq!(t.hit_test(Point::new(70.0, 70.0)), Some(b1));
        assert_eq!(t.hit_test(Point::new(150.0, 10.0)), None);
        assert_eq!(*t.data(b1), "b1");
        assert_eq!(t.parent(b1), Some(b));
        assert_eq!(t.find(|d| *d == "a1"), Some(a1));
    }

    #[test]
    fn capture_redirects_pointer_target() {
        let (mut t, [_, a1, _, b1]) = tree();
        assert_eq!(t.target(Point::new(70.0, 70.0)), Some(b1));
        t.set_capture(a1);
        assert_eq!(t.target(Point::new(70.0, 70.0)), Some(a1));
        assert_eq!(t.target(Point::new(500.0, 500.0)), Some(a1));
        t.release_capture();
        assert_eq!(t.captured(), None);
        assert_eq!(t.target(Point::new(70.0, 70.0)), Some(b1));
    }

    #[test]
    fn focus_cycles_focusable_nodes_in_order() {
        let (mut t, [a, a1, _, b1]) = tree();
        assert_eq!(t.focus_next(), Some(a1));
        assert_eq!(t.focus_next(), Some(b1));
        assert_eq!(t.focus_next(), Some(a1));
        assert_eq!(t.focus_prev(), Some(b1));
        t.set_focus(Some(a)); // not focusable: ignored
        assert_eq!(t.focused(), Some(b1));
        t.set_focus(None);
        assert_eq!(t.focused(), None);
        assert_eq!(t.focus_prev(), Some(b1));
    }

    #[test]
    fn dirty_propagates_to_ancestors_and_clears() {
        let (mut t, [a, a1, b, _]) = tree();
        assert!(!t.is_dirty(a1));
        t.mark_dirty(a1);
        assert!(t.is_dirty(a1) && t.is_dirty(a) && t.is_dirty(t.root()));
        assert!(!t.is_dirty(b));
        t.clear_dirty();
        assert!(!t.is_dirty(a) && !t.is_dirty(t.root()));
    }

    #[test]
    fn focus_with_no_focusable_nodes_is_none() {
        let mut t = ViewTree::new(Rect::new(0.0, 0.0, 1.0, 1.0), ());
        assert_eq!(t.focus_next(), None);
        assert_eq!(t.focus_prev(), None);
    }
}
