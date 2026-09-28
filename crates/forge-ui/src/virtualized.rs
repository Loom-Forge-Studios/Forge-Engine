//! Virtualisation indexes (Ch.21 §21.12; the plan's `virtual` module — `virtual` is a
//! reserved word in Rust).
//!
//! * [`CountedSeq`] is a B+tree with fanout 32 whose leaves are rows and whose internal
//!   nodes keep three sums over their subtree: the **item count**, the **weight** (rows a
//!   tree node shows) and the **height**. Row index → item, pixel offset → item, insert,
//!   remove and re-weighting one item all cost O(log₃₂ n) node visits. A Fenwick tree is
//!   deliberately not used: its positions are fixed, so an insertion in the middle would
//!   rebuild it in O(n).
//! * [`TreeIndex`] is the visible-row index of a tree: the model tree itself, counted.
//!   Each node keeps its children in its own `CountedSeq` whose weights are the rows each
//!   child shows; `inner(x)` is maintained whether or not `x` is expanded, so expanding
//!   or collapsing a 50,000-row subtree changes one weight per ancestor level —
//!   O(d · log₃₂ b) index-node updates, never O(k) in the rows that appear or vanish.
//!   Nothing is flattened, on expand, on edit, or per frame.
//!
//! Every mutation counts the index nodes it touched ([`CountedSeq::updates`],
//! [`TreeIndex::updates`]) so `ui_virtual_tree_100k` can gate the bound.

use std::collections::HashMap;

/// Children per B+tree node.
pub const FANOUT: usize = 32;
const NONE: u32 = u32::MAX;

/// A stable handle to an item of a [`CountedSeq`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u32);

#[derive(Clone, Debug)]
struct Item<T> {
    weight: u64,
    height: f64,
    leaf: u32,
    payload: Option<T>,
}

#[derive(Clone, Debug)]
struct BNode {
    parent: u32,
    leaf: bool,
    /// Item ids (leaf) or node ids (internal), in order.
    kids: Vec<u32>,
    n: u64,
    w: u64,
    h: f64,
}

impl BNode {
    fn empty(leaf: bool) -> Self {
        Self {
            parent: NONE,
            leaf,
            kids: Vec::new(),
            n: 0,
            w: 0,
            h: 0.0,
        }
    }
}

/// A counted sequence (see the module docs).
#[derive(Clone, Debug)]
pub struct CountedSeq<T> {
    nodes: Vec<BNode>,
    free_nodes: Vec<u32>,
    root: u32,
    items: Vec<Item<T>>,
    free_items: Vec<u32>,
    /// Index nodes written by mutations since creation (the budget tests read it).
    pub updates: u64,
}

impl<T> Default for CountedSeq<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> CountedSeq<T> {
    pub fn new() -> Self {
        Self {
            nodes: vec![BNode::empty(true)],
            free_nodes: Vec::new(),
            root: 0,
            items: Vec::new(),
            free_items: Vec::new(),
            updates: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.nodes[self.root as usize].n as usize
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn total_weight(&self) -> u64 {
        self.nodes[self.root as usize].w
    }
    pub fn total_height(&self) -> f64 {
        self.nodes[self.root as usize].h
    }
    /// Levels from the root to a leaf (1 for a single leaf).
    pub fn depth(&self) -> usize {
        let mut d = 1;
        let mut n = self.root;
        while !self.nodes[n as usize].leaf {
            n = self.nodes[n as usize].kids[0];
            d += 1;
        }
        d
    }

    fn live(&self, id: ItemId) -> bool {
        self.items
            .get(id.0 as usize)
            .is_some_and(|i| i.payload.is_some())
    }

    pub fn get(&self, id: ItemId) -> Option<&T> {
        self.items.get(id.0 as usize)?.payload.as_ref()
    }
    pub fn get_mut(&mut self, id: ItemId) -> Option<&mut T> {
        self.items.get_mut(id.0 as usize)?.payload.as_mut()
    }
    pub fn weight(&self, id: ItemId) -> u64 {
        self.items.get(id.0 as usize).map_or(0, |i| i.weight)
    }
    pub fn height(&self, id: ItemId) -> f64 {
        self.items.get(id.0 as usize).map_or(0.0, |i| i.height)
    }

    fn alloc_node(&mut self, n: BNode) -> u32 {
        if let Some(i) = self.free_nodes.pop() {
            self.nodes[i as usize] = n;
            i
        } else {
            self.nodes.push(n);
            (self.nodes.len() - 1) as u32
        }
    }

    /// Add `(dn, dw, dh)` to `node` and every ancestor. Returns the nodes touched.
    fn bubble(&mut self, mut node: u32, dn: i64, dw: i64, dh: f64) -> u64 {
        let mut touched = 0;
        while node != NONE {
            let b = &mut self.nodes[node as usize];
            b.n = (b.n as i64 + dn) as u64;
            b.w = (b.w as i64 + dw) as u64;
            b.h += dh;
            touched += 1;
            node = b.parent;
        }
        self.updates += touched;
        touched
    }

    /// Insert at item position `pos` (clamped to `len`).
    pub fn insert(&mut self, pos: usize, weight: u64, height: f64, payload: T) -> ItemId {
        let pos = pos.min(self.len());
        // Descend by item counts.
        let mut node = self.root;
        let mut local = pos as u64;
        while !self.nodes[node as usize].leaf {
            let kids = &self.nodes[node as usize].kids;
            let mut chosen = *kids.last().unwrap_or(&node);
            for &k in kids {
                let n = self.nodes[k as usize].n;
                if local <= n {
                    chosen = k;
                    break;
                }
                local -= n;
            }
            node = chosen;
        }
        let item = Item {
            weight,
            height,
            leaf: node,
            payload: Some(payload),
        };
        let id = if let Some(i) = self.free_items.pop() {
            self.items[i as usize] = item;
            i
        } else {
            self.items.push(item);
            (self.items.len() - 1) as u32
        };
        let at = (local as usize).min(self.nodes[node as usize].kids.len());
        let appended = at == self.nodes[node as usize].kids.len();
        self.nodes[node as usize].kids.insert(at, id);
        self.bubble(node, 1, weight as i64, height);
        if self.nodes[node as usize].kids.len() > FANOUT {
            self.split(node, appended);
        }
        ItemId(id)
    }

    /// Append.
    pub fn push(&mut self, weight: u64, height: f64, payload: T) -> ItemId {
        self.insert(self.len(), weight, height, payload)
    }

    fn recompute(&mut self, node: u32) {
        let (mut n, mut w, mut h) = (0u64, 0u64, 0.0f64);
        let b = &self.nodes[node as usize];
        if b.leaf {
            for &i in &b.kids {
                let it = &self.items[i as usize];
                n += 1;
                w += it.weight;
                h += it.height;
            }
        } else {
            for &c in &b.kids {
                let cb = &self.nodes[c as usize];
                n += cb.n;
                w += cb.w;
                h += cb.h;
            }
        }
        let b = &mut self.nodes[node as usize];
        b.n = n;
        b.w = w;
        b.h = h;
    }

    /// Split an overfull node. An append keeps the left node full (sequential building
    /// packs leaves densely, so the tree stays at ⌈log₃₂ n⌉ levels).
    fn split(&mut self, node: u32, appended: bool) {
        let len = self.nodes[node as usize].kids.len();
        let at = if appended { FANOUT } else { len / 2 };
        let leaf = self.nodes[node as usize].leaf;
        let moved: Vec<u32> = self.nodes[node as usize].kids.split_off(at);
        let parent = self.nodes[node as usize].parent;
        let sib = self.alloc_node(BNode {
            parent,
            leaf,
            kids: moved.clone(),
            n: 0,
            w: 0,
            h: 0.0,
        });
        for &m in &moved {
            if leaf {
                self.items[m as usize].leaf = sib;
            } else {
                self.nodes[m as usize].parent = sib;
            }
        }
        self.recompute(node);
        self.recompute(sib);
        self.updates += 2;
        if parent == NONE {
            let root = self.alloc_node(BNode::empty(false));
            self.nodes[root as usize].kids = vec![node, sib];
            self.nodes[node as usize].parent = root;
            self.nodes[sib as usize].parent = root;
            self.recompute(root);
            self.root = root;
            self.updates += 1;
        } else {
            let p = &mut self.nodes[parent as usize];
            let i = p
                .kids
                .iter()
                .position(|k| *k == node)
                .unwrap_or(p.kids.len() - 1);
            p.kids.insert(i + 1, sib);
            let last = p.kids.len() > FANOUT;
            let appended_up = i + 2 == p.kids.len();
            if last {
                self.split(parent, appended_up);
            }
        }
    }

    /// Remove an item; returns its payload.
    pub fn remove(&mut self, id: ItemId) -> Option<T> {
        if !self.live(id) {
            return None;
        }
        let (leaf, w, h) = {
            let it = &self.items[id.0 as usize];
            (it.leaf, it.weight, it.height)
        };
        self.nodes[leaf as usize].kids.retain(|k| *k != id.0);
        self.bubble(leaf, -1, -(w as i64), -h);
        // Drop empty nodes (never the root).
        let mut node = leaf;
        while node != self.root && self.nodes[node as usize].kids.is_empty() {
            let parent = self.nodes[node as usize].parent;
            self.nodes[parent as usize].kids.retain(|k| *k != node);
            self.free_nodes.push(node);
            node = parent;
        }
        // Collapse a root with one internal child.
        while !self.nodes[self.root as usize].leaf && self.nodes[self.root as usize].kids.len() == 1
        {
            let only = self.nodes[self.root as usize].kids[0];
            self.free_nodes.push(self.root);
            self.root = only;
            self.nodes[only as usize].parent = NONE;
        }
        self.free_items.push(id.0);
        self.items[id.0 as usize].payload.take()
    }

    /// Change an item's weight and height; returns the index nodes updated.
    pub fn set(&mut self, id: ItemId, weight: u64, height: f64) -> u64 {
        if !self.live(id) {
            return 0;
        }
        let it = &mut self.items[id.0 as usize];
        let dw = weight as i64 - it.weight as i64;
        let dh = height - it.height;
        it.weight = weight;
        it.height = height;
        let leaf = it.leaf;
        if dw == 0 && dh == 0.0 {
            return 0;
        }
        self.bubble(leaf, 0, dw, dh)
    }
    pub fn set_weight(&mut self, id: ItemId, weight: u64) -> u64 {
        let h = self.height(id);
        self.set(id, weight, h)
    }
    pub fn set_height(&mut self, id: ItemId, height: f64) -> u64 {
        let w = self.weight(id);
        self.set(id, w, height)
    }

    /// Sums of everything before `id`: `(items, weight, height)`.
    pub fn before(&self, id: ItemId) -> (u64, u64, f64) {
        if !self.live(id) {
            return (0, 0, 0.0);
        }
        let leaf = self.items[id.0 as usize].leaf;
        let (mut n, mut w, mut h) = (0u64, 0u64, 0.0f64);
        for &k in &self.nodes[leaf as usize].kids {
            if k == id.0 {
                break;
            }
            let it = &self.items[k as usize];
            n += 1;
            w += it.weight;
            h += it.height;
        }
        let mut child = leaf;
        let mut node = self.nodes[leaf as usize].parent;
        while node != NONE {
            for &k in &self.nodes[node as usize].kids {
                if k == child {
                    break;
                }
                let b = &self.nodes[k as usize];
                n += b.n;
                w += b.w;
                h += b.h;
            }
            child = node;
            node = self.nodes[node as usize].parent;
        }
        (n, w, h)
    }
    pub fn position(&self, id: ItemId) -> usize {
        self.before(id).0 as usize
    }

    fn find(
        &self,
        mut target: f64,
        pick: impl Fn(&BNode) -> f64,
        item_v: impl Fn(&Item<T>) -> f64,
    ) -> Option<(ItemId, f64)> {
        if self.is_empty() || target < 0.0 {
            return None;
        }
        let mut node = self.root;
        let mut before = 0.0;
        loop {
            let b = &self.nodes[node as usize];
            if b.leaf {
                for &k in &b.kids {
                    let v = item_v(&self.items[k as usize]);
                    if target < v {
                        return Some((ItemId(k), before));
                    }
                    target -= v;
                    before += v;
                }
                return None;
            }
            let mut next = None;
            for &k in &b.kids {
                let v = pick(&self.nodes[k as usize]);
                if target < v {
                    next = Some(k);
                    break;
                }
                target -= v;
                before += v;
            }
            node = next?;
        }
    }

    /// The item covering weight offset `offset`, and the weight before it.
    pub fn find_weight(&self, offset: u64) -> Option<(ItemId, u64)> {
        self.find(offset as f64, |b| b.w as f64, |i| i.weight as f64)
            .map(|(id, b)| (id, b as u64))
    }
    /// The item covering pixel offset `y`, and the height before it.
    pub fn find_height(&self, y: f64) -> Option<(ItemId, f64)> {
        self.find(y, |b| b.h, |i| i.height)
    }
    /// The item at position `pos`.
    pub fn item_at(&self, pos: usize) -> Option<ItemId> {
        self.find(pos as f64, |b| b.n as f64, |_| 1.0)
            .map(|(id, _)| id)
    }

    /// The item after `id` in order.
    pub fn next(&self, id: ItemId) -> Option<ItemId> {
        let leaf = self.items.get(id.0 as usize)?.leaf;
        let kids = &self.nodes[leaf as usize].kids;
        let i = kids.iter().position(|k| *k == id.0)?;
        if let Some(n) = kids.get(i + 1) {
            return Some(ItemId(*n));
        }
        // Up until a right sibling exists, then down its leftmost path.
        let mut child = leaf;
        let mut node = self.nodes[leaf as usize].parent;
        while node != NONE {
            let kids = &self.nodes[node as usize].kids;
            let ci = kids.iter().position(|k| *k == child)?;
            if let Some(&sib) = kids.get(ci + 1) {
                let mut d = sib;
                while !self.nodes[d as usize].leaf {
                    d = *self.nodes[d as usize].kids.first()?;
                }
                return self.nodes[d as usize].kids.first().map(|k| ItemId(*k));
            }
            child = node;
            node = self.nodes[node as usize].parent;
        }
        None
    }

    /// Items in order (tests; O(n)).
    pub fn iter(&self) -> impl Iterator<Item = ItemId> + '_ {
        let mut cur = self.item_at(0);
        std::iter::from_fn(move || {
            let c = cur?;
            cur = self.next(c);
            Some(c)
        })
    }
}

// ---- the tree index ---------------------------------------------------------------------

#[derive(Clone, Debug)]
struct TNode {
    parent: Option<u64>,
    /// This node's item in its parent's children sequence.
    item: ItemId,
    expanded: bool,
    children: Option<Box<CountedSeq<u64>>>,
    depth: u32,
}

/// The counted visible-row index of a tree (see the module docs). Nodes are identified by
/// model keys (`u64`); the root is implicit and always expanded (its row is not shown).
#[derive(Clone, Debug, Default)]
pub struct TreeIndex {
    top: CountedSeq<u64>,
    nodes: HashMap<u64, TNode>,
    /// Index nodes written by the last structural operation.
    pub last_updates: u64,
    /// Positive control for `ui_virtual_tree_100k`: expanding also splices every row that
    /// appears or disappears one by one (the O(k) algorithm §21.12 forbids).
    pub fault_splice_rows: crate::controls::Switch,
}

impl TreeIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Visible rows.
    pub fn row_count(&self) -> u64 {
        self.top.total_weight()
    }
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    pub fn contains(&self, key: u64) -> bool {
        self.nodes.contains_key(&key)
    }

    fn inner(&self, key: u64) -> u64 {
        self.nodes
            .get(&key)
            .and_then(|n| n.children.as_ref())
            .map_or(0, |c| c.total_weight())
    }
    fn rows(&self, key: u64) -> u64 {
        match self.nodes.get(&key) {
            Some(n) if n.expanded => 1 + self.inner(key),
            Some(_) => 1,
            None => 0,
        }
    }

    fn seq_of(&mut self, parent: Option<u64>) -> Option<&mut CountedSeq<u64>> {
        match parent {
            None => Some(&mut self.top),
            Some(p) => {
                let n = self.nodes.get_mut(&p)?;
                Some(n.children.get_or_insert_with(Default::default))
            }
        }
    }

    /// Re-weight `key` in its parent to its current `rows`, then propagate up through
    /// expanded ancestors. Returns index nodes touched.
    fn propagate(&mut self, mut key: u64) -> u64 {
        let mut touched = 0;
        loop {
            let rows = self.rows(key);
            let Some(n) = self.nodes.get(&key) else {
                return touched;
            };
            let (parent, item) = (n.parent, n.item);
            let before = match parent {
                None => self.top.total_weight(),
                Some(p) => self.inner(p),
            };
            let Some(seq) = self.seq_of(parent) else {
                return touched;
            };
            touched += seq.set_weight(item, rows);
            let after = seq.total_weight();
            match parent {
                Some(p) if after != before && self.nodes.get(&p).is_some_and(|n| n.expanded) => {
                    key = p;
                }
                _ => return touched,
            }
        }
    }

    /// Insert `key` under `parent` (`None`: top level) at child position `pos`.
    pub fn insert(&mut self, parent: Option<u64>, pos: usize, key: u64, expanded: bool) -> bool {
        if self.nodes.contains_key(&key) || parent.is_some_and(|p| !self.nodes.contains_key(&p)) {
            return false;
        }
        let depth = parent
            .and_then(|p| self.nodes.get(&p))
            .map_or(0, |n| n.depth + 1);
        let Some(seq) = self.seq_of(parent) else {
            return false;
        };
        let item = seq.insert(pos, 1, 0.0, key);
        self.nodes.insert(
            key,
            TNode {
                parent,
                item,
                expanded,
                children: None,
                depth,
            },
        );
        self.last_updates = match parent {
            Some(p) if self.nodes.get(&p).is_some_and(|n| n.expanded) => self.propagate(p),
            _ => 0,
        };
        true
    }

    /// Remove `key` and its subtree.
    pub fn remove(&mut self, key: u64) -> bool {
        self.remove_subtree(key).is_some()
    }

    /// Remove `key` and its subtree; returns the keys removed (`key` first), or `None` if
    /// `key` is not a node. O(subtree + d · log₃₂ b).
    pub fn remove_subtree(&mut self, key: u64) -> Option<Vec<u64>> {
        // Copy the fields: cloning the node would copy its whole child sequence.
        let (parent, item) = self.nodes.get(&key).map(|n| (n.parent, n.item))?;
        if let Some(seq) = self.seq_of(parent) {
            seq.remove(item);
        }
        let mut removed = Vec::new();
        let mut stack = vec![key];
        while let Some(k) = stack.pop() {
            if let Some(t) = self.nodes.remove(&k) {
                removed.push(k);
                if let Some(c) = t.children {
                    stack.extend(c.iter().filter_map(|i| c.get(i).copied()));
                }
            }
        }
        self.last_updates = match parent {
            Some(p) if self.nodes.get(&p).is_some_and(|x| x.expanded) => self.propagate(p),
            _ => 0,
        };
        Some(removed)
    }

    /// Move `key` (with its subtree) under `parent` at `pos`. Refuses to move a node into
    /// its own subtree.
    pub fn move_node(&mut self, key: u64, parent: Option<u64>, pos: usize) -> bool {
        if parent.is_some_and(|p| p == key || self.is_ancestor(key, p)) {
            return false;
        }
        // Copy the fields: cloning the node would copy its whole child sequence.
        let Some((old_parent, old_item, old_depth)) =
            self.nodes.get(&key).map(|n| (n.parent, n.item, n.depth))
        else {
            return false;
        };
        let rows = self.rows(key);
        if let Some(seq) = self.seq_of(old_parent) {
            seq.remove(old_item);
        }
        let mut touched = match old_parent {
            Some(p) if self.nodes.get(&p).is_some_and(|x| x.expanded) => self.propagate(p),
            _ => 0,
        };
        let Some(seq) = self.seq_of(parent) else {
            return false;
        };
        let item = seq.insert(pos, rows, 0.0, key);
        let depth = parent
            .and_then(|p| self.nodes.get(&p))
            .map_or(0, |x| x.depth + 1);
        if let Some(t) = self.nodes.get_mut(&key) {
            t.parent = parent;
            t.item = item;
            t.depth = depth;
        }
        if depth != old_depth {
            self.redepth(key, depth);
        }
        touched += match parent {
            Some(p) if self.nodes.get(&p).is_some_and(|x| x.expanded) => self.propagate(p),
            _ => 0,
        };
        self.last_updates = touched;
        true
    }

    fn redepth(&mut self, key: u64, depth: u32) {
        let mut stack = vec![(key, depth)];
        while let Some((k, d)) = stack.pop() {
            let kids: Vec<u64> = match self.nodes.get_mut(&k) {
                Some(t) => {
                    t.depth = d;
                    t.children
                        .as_ref()
                        .map(|c| c.iter().filter_map(|i| c.get(i).copied()).collect())
                        .unwrap_or_default()
                }
                None => Vec::new(),
            };
            stack.extend(kids.into_iter().map(|c| (c, d + 1)));
        }
    }

    /// `a` is a proper ancestor of `b`.
    pub fn is_ancestor(&self, a: u64, b: u64) -> bool {
        let mut cur = self.nodes.get(&b).and_then(|n| n.parent);
        while let Some(p) = cur {
            if p == a {
                return true;
            }
            cur = self.nodes.get(&p).and_then(|n| n.parent);
        }
        false
    }

    /// Expand or collapse. Returns the index nodes updated: O(d · log₃₂ b), whatever the
    /// number of rows that appear or disappear.
    pub fn set_expanded(&mut self, key: u64, expanded: bool) -> u64 {
        match self.nodes.get_mut(&key) {
            Some(n) if n.expanded != expanded => n.expanded = expanded,
            _ => {
                self.last_updates = 0;
                return 0;
            }
        }
        self.last_updates = self.propagate(key);
        if self.fault_splice_rows.on() {
            // One index write per row that appears or disappears.
            self.last_updates += self.inner(key);
        }
        self.last_updates
    }

    pub fn is_expanded(&self, key: u64) -> bool {
        self.nodes.get(&key).is_some_and(|n| n.expanded)
    }
    pub fn has_children(&self, key: u64) -> bool {
        self.nodes
            .get(&key)
            .and_then(|n| n.children.as_ref())
            .is_some_and(|c| !c.is_empty())
    }
    pub fn child_count(&self, key: u64) -> usize {
        self.nodes
            .get(&key)
            .and_then(|n| n.children.as_ref())
            .map_or(0, |c| c.len())
    }
    pub fn parent(&self, key: u64) -> Option<u64> {
        self.nodes.get(&key).and_then(|n| n.parent)
    }
    pub fn depth(&self, key: u64) -> Option<u32> {
        self.nodes.get(&key).map(|n| n.depth)
    }
    /// Children under `parent` (`None`: top level), O(1).
    pub fn len_under(&self, parent: Option<u64>) -> usize {
        match parent {
            None => self.top.len(),
            Some(p) => self.child_count(p),
        }
    }
    /// Child position of `key` under its parent.
    pub fn index_in_parent(&self, key: u64) -> Option<usize> {
        let n = self.nodes.get(&key)?;
        let seq = match n.parent {
            None => &self.top,
            Some(p) => self.nodes.get(&p)?.children.as_deref()?,
        };
        Some(seq.position(n.item))
    }
    /// The child at position `pos` under `parent` (`None`: top level). O(log₃₂ b).
    pub fn child_at(&self, parent: Option<u64>, pos: usize) -> Option<u64> {
        let seq = match parent {
            None => &self.top,
            Some(p) => self.nodes.get(&p)?.children.as_deref()?,
        };
        seq.item_at(pos).and_then(|i| seq.get(i).copied())
    }
    /// Every node's key, in no particular order.
    pub fn keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.nodes.keys().copied()
    }
    /// Children of `key` (`None`: top level), in order (O(children)).
    pub fn children(&self, key: Option<u64>) -> Vec<u64> {
        let seq = match key {
            None => Some(&self.top),
            Some(k) => self.nodes.get(&k).and_then(|n| n.children.as_deref()),
        };
        seq.map(|s| s.iter().filter_map(|i| s.get(i).copied()).collect())
            .unwrap_or_default()
    }

    /// The node shown at visible row `row`, with its depth. O(d · log₃₂ b).
    pub fn node_at_row(&self, row: u64) -> Option<(u64, u32)> {
        let mut seq = &self.top;
        let mut r = row;
        loop {
            let (item, before) = seq.find_weight(r)?;
            let key = *seq.get(item)?;
            let n = self.nodes.get(&key)?;
            let off = r - before;
            if off == 0 {
                return Some((key, n.depth));
            }
            r = off - 1;
            seq = n.children.as_deref()?;
        }
    }

    /// The visible row of `key`, or `None` if an ancestor is collapsed. O(d · log₃₂ b).
    pub fn row_of(&self, key: u64) -> Option<u64> {
        let mut row = 0u64;
        let mut cur = key;
        loop {
            let n = self.nodes.get(&cur)?;
            let seq = match n.parent {
                None => &self.top,
                Some(p) => self.nodes.get(&p)?.children.as_deref()?,
            };
            row += seq.before(n.item).1;
            match n.parent {
                None => return Some(row),
                Some(p) => {
                    if !self.nodes.get(&p)?.expanded {
                        return None;
                    }
                    row += 1;
                    cur = p;
                }
            }
        }
    }

    /// Depth of the deepest node and the largest fanout (the gate's `d` and `b`).
    pub fn shape(&self) -> (u32, usize) {
        let d = self.nodes.values().map(|n| n.depth + 1).max().unwrap_or(0);
        let b = self
            .nodes
            .values()
            .filter_map(|n| n.children.as_ref().map(|c| c.len()))
            .max()
            .unwrap_or(0)
            .max(self.top.len());
        (d, b)
    }
}

/// The §21.12 bound on index-node updates for one expand/collapse:
/// `(d + 1) · (⌈log₃₂ b⌉ + 1)`.
pub fn update_bound(depth: u32, fanout: usize) -> u64 {
    let mut levels = 0u32;
    let mut cap = 1usize;
    while cap < fanout.max(1) {
        cap = cap.saturating_mul(FANOUT);
        levels += 1;
    }
    u64::from(depth + 1) * u64::from(levels + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_positions_weights_and_heights() {
        let mut s: CountedSeq<u32> = CountedSeq::new();
        for i in 0..1000u32 {
            s.push(1 + u64::from(i % 3), 10.0 + f64::from(i % 5), i);
        }
        assert_eq!(s.len(), 1000);
        assert!(s.depth() <= 3, "depth {}", s.depth());
        for pos in [0usize, 1, 31, 32, 33, 500, 999] {
            let id = s.item_at(pos).unwrap_or(ItemId(0));
            assert_eq!(s.get(id), Some(&(pos as u32)));
            assert_eq!(s.position(id), pos);
        }
        let (id, before) = s
            .find_weight(s.total_weight() - 1)
            .unwrap_or((ItemId(0), 0));
        assert_eq!(s.get(id), Some(&999));
        assert_eq!(before + s.weight(id), s.total_weight());
        let (hid, hb) = s.find_height(5_432.5).unwrap_or((ItemId(0), -1.0));
        assert!(hb <= 5_432.5 && hb + s.height(hid) > 5_432.5);
        assert_eq!(s.before(hid).2, hb);
        assert!(s.find_height(s.total_height() + 1.0).is_none());
        let mid = s.item_at(500).unwrap_or(ItemId(0));
        s.insert(500, 1, 1.0, 7777);
        assert_eq!(s.position(mid), 501);
        assert_eq!(s.remove(mid), Some(500));
        assert_eq!(s.len(), 1000);
        let order: Vec<u32> = s.iter().filter_map(|i| s.get(i).copied()).collect();
        assert_eq!(order.len(), 1000);
        assert_eq!(order[500], 7777);
    }

    #[test]
    fn tree_rows_follow_expansion_without_flattening() {
        let mut t = TreeIndex::new();
        t.insert(None, 0, 1, true);
        for i in 0..100u64 {
            t.insert(Some(1), i as usize, 100 + i, false);
        }
        for i in 0..50u64 {
            t.insert(Some(100), i as usize, 1000 + i, false);
        }
        assert_eq!(t.row_count(), 101);
        let u = t.set_expanded(100, true);
        assert_eq!(t.row_count(), 151);
        assert!(u <= update_bound(3, 100), "{u} updates");
        assert_eq!(t.node_at_row(2), Some((1000, 2)));
        assert_eq!(t.row_of(1049), Some(51));
        assert_eq!(t.row_of(101), Some(52));
        t.set_expanded(1, false);
        assert_eq!(t.row_count(), 1);
        assert_eq!(t.row_of(1000), None);
        t.set_expanded(1, true);
        assert_eq!(t.row_count(), 151, "inner(x) survives collapse");
        assert!(t.move_node(1000, Some(101), 0));
        assert_eq!(
            t.row_count(),
            150,
            "101 is collapsed: the moved row is hidden"
        );
        assert!(!t.move_node(1, Some(100), 0), "no cycles");
        assert!(t.remove(100));
        assert_eq!(t.row_count(), 100);
    }
}
