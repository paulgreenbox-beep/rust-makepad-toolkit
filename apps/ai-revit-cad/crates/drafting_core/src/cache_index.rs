//! Incremental two-dimensional AABB index used by the view cache.
//!
//! The tree is a dynamic bounding-volume hierarchy. Leaves are addressed by
//! entity ID, so confirmed edits can replace or remove a single entry. Queries
//! compare camera-relative bounds at every visited node; callers can therefore
//! keep large world coordinates in f64 without first forming a large absolute
//! query rectangle.

use crate::{Bounds2, DraftEntityId, DraftError, DraftResult, Point2};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug)]
struct Node {
    parent: Option<usize>,
    left: Option<usize>,
    right: Option<usize>,
    bounds: Bounds2,
    height: i32,
    entity: Option<DraftEntityId>,
}

impl Node {
    fn leaf(entity: DraftEntityId, bounds: Bounds2) -> Self {
        Self {
            parent: None,
            left: None,
            right: None,
            bounds,
            height: 0,
            entity: Some(entity),
        }
    }

    fn branch(left: usize, right: usize, bounds: Bounds2, height: i32) -> Self {
        Self {
            parent: None,
            left: Some(left),
            right: Some(right),
            bounds,
            height,
            entity: None,
        }
    }

    fn vacant() -> Self {
        Self {
            parent: None,
            left: None,
            right: None,
            bounds: Bounds2::from_points(Point2::default(), Point2::default()),
            height: 0,
            entity: None,
        }
    }

    fn is_leaf(self) -> bool {
        self.entity.is_some()
    }
}

/// Result and actual traversal counters for one broad-phase query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct IndexQuery {
    pub(crate) ids: Vec<DraftEntityId>,
    pub(crate) nodes_visited: u64,
    pub(crate) leaves_examined: u64,
}

/// Maintained dynamic AABB hierarchy. A cache may keep one instance globally
/// and one per layer; upsert/remove update each affected hierarchy locally.
#[derive(Clone, Debug, Default)]
pub(crate) struct MaintainedIndex {
    nodes: Vec<Node>,
    free_nodes: Vec<usize>,
    leaves: BTreeMap<DraftEntityId, usize>,
    root: Option<usize>,
}

impl MaintainedIndex {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Builds a balanced hierarchy and rejects duplicate IDs or invalid AABBs.
    pub(crate) fn bulk_load(
        items: impl IntoIterator<Item = (DraftEntityId, Bounds2)>,
    ) -> DraftResult<Self> {
        let mut entries = Vec::new();
        let mut ids = BTreeSet::new();
        for (id, bounds) in items {
            validate_bounds(bounds)?;
            if !ids.insert(id) {
                return Err(DraftError::new(
                    "duplicate_spatial_entity",
                    format!("entity {id} occurs more than once in the spatial index"),
                ));
            }
            entries.push((id, bounds));
        }

        let mut index = Self::new();
        index.root = index.build_range(&mut entries);
        Ok(index)
    }

    /// Inserts or replaces one entity's current bounds.
    pub(crate) fn upsert(&mut self, id: DraftEntityId, bounds: Bounds2) -> DraftResult<()> {
        validate_bounds(bounds)?;
        if self.leaves.contains_key(&id) {
            self.remove(id);
        }
        let leaf = self.allocate(Node::leaf(id, bounds));
        self.leaves.insert(id, leaf);
        self.insert_leaf(leaf);
        Ok(())
    }

    /// Removes one entity. Returns false when the ID is not present.
    pub(crate) fn remove(&mut self, id: DraftEntityId) -> bool {
        let Some(leaf) = self.leaves.remove(&id) else {
            return false;
        };

        if self.root == Some(leaf) {
            self.root = None;
            self.release(leaf);
            self.compact_released_slots();
            return true;
        }

        let parent = self.nodes[leaf].parent.expect("non-root leaf has a parent");
        let grandparent = self.nodes[parent].parent;
        let sibling = if self.nodes[parent].left == Some(leaf) {
            self.nodes[parent].right.expect("branch has right child")
        } else {
            debug_assert_eq!(self.nodes[parent].right, Some(leaf));
            self.nodes[parent].left.expect("branch has left child")
        };

        match grandparent {
            Some(grandparent) => {
                self.replace_child(grandparent, parent, sibling);
                self.nodes[sibling].parent = Some(grandparent);
                self.release(parent);
                self.release(leaf);
                self.fix_upwards(Some(grandparent));
            }
            None => {
                self.root = Some(sibling);
                self.nodes[sibling].parent = None;
                self.release(parent);
                self.release(leaf);
            }
        }
        self.compact_released_slots();
        true
    }

    /// Bounds enclosing all indexed entities, including entities on hidden
    /// layers. Visibility filtering belongs to the caller's layer cache.
    pub(crate) fn bounds(&self) -> Option<Bounds2> {
        self.root.map(|root| self.nodes[root].bounds)
    }

    /// Retained logical slots. Dense storage uses two nodes per leaf minus one.
    pub(crate) fn allocated_nodes(&self) -> usize {
        self.nodes.len()
    }

    pub(crate) fn reserved_nodes(&self) -> usize {
        self.nodes.capacity()
    }

    /// Returns leaf AABBs intersecting a query expressed relative to `origin`.
    /// Traversal counters report nodes actually visited and leaves actually
    /// tested, including nodes rejected by their aggregate bounds.
    pub(crate) fn query_relative(&self, origin: Point2, bounds: Bounds2) -> IndexQuery {
        if !origin.is_finite() || !valid_bounds(bounds) {
            return IndexQuery::default();
        }

        let Some(root) = self.root else {
            return IndexQuery::default();
        };
        let mut result = IndexQuery::default();
        let mut pending = vec![root];
        while let Some(node_index) = pending.pop() {
            result.nodes_visited += 1;
            let node = self.nodes[node_index];
            if node.is_leaf() {
                result.leaves_examined += 1;
            }
            if !relative(node.bounds, origin).intersects(bounds) {
                continue;
            }

            if node.is_leaf() {
                result.ids.push(node.entity.expect("leaf has entity ID"));
            } else {
                pending.push(node.right.expect("branch has right child"));
                pending.push(node.left.expect("branch has left child"));
            }
        }
        result.ids.sort_unstable();
        result
    }

    fn build_range(&mut self, entries: &mut [(DraftEntityId, Bounds2)]) -> Option<usize> {
        match entries.len() {
            0 => return None,
            1 => {
                let (id, bounds) = entries[0];
                let leaf = self.allocate(Node::leaf(id, bounds));
                self.leaves.insert(id, leaf);
                return Some(leaf);
            }
            _ => {}
        }

        let (x_spread, y_spread) = centroid_spread(entries);
        let split_x = x_spread >= y_spread;
        let middle = entries.len() / 2;
        entries.select_nth_unstable_by(middle, |a, b| {
            centroid_axis(a.1, split_x)
                .total_cmp(&centroid_axis(b.1, split_x))
                .then_with(|| a.0.cmp(&b.0))
        });
        let (left_entries, right_entries) = entries.split_at_mut(middle);
        let left = self.build_range(left_entries).expect("nonempty left partition");
        let right = self
            .build_range(right_entries)
            .expect("nonempty right partition");
        let bounds = self.nodes[left].bounds.union(self.nodes[right].bounds);
        let height = 1 + self.nodes[left].height.max(self.nodes[right].height);
        let branch = self.allocate(Node::branch(left, right, bounds, height));
        self.nodes[left].parent = Some(branch);
        self.nodes[right].parent = Some(branch);
        Some(branch)
    }

    fn insert_leaf(&mut self, leaf: usize) {
        let Some(root) = self.root else {
            self.root = Some(leaf);
            self.nodes[leaf].parent = None;
            return;
        };

        let leaf_bounds = self.nodes[leaf].bounds;
        let sibling = self.choose_sibling(root, leaf_bounds);
        let old_parent = self.nodes[sibling].parent;
        let branch_bounds = self.nodes[sibling].bounds.union(leaf_bounds);
        let branch_height = self.nodes[sibling].height + 1;
        let branch = self.allocate(Node::branch(
            sibling,
            leaf,
            branch_bounds,
            branch_height,
        ));
        self.nodes[sibling].parent = Some(branch);
        self.nodes[leaf].parent = Some(branch);

        if let Some(parent) = old_parent {
            self.replace_child(parent, sibling, branch);
            self.nodes[branch].parent = Some(parent);
            self.fix_upwards(Some(branch));
        } else {
            self.root = Some(branch);
            self.fix_upwards(Some(branch));
        }
    }

    fn choose_sibling(&self, mut node_index: usize, leaf_bounds: Bounds2) -> usize {
        loop {
            let node = self.nodes[node_index];
            if node.is_leaf() {
                return node_index;
            }

            let left = node.left.expect("branch has left child");
            let right = node.right.expect("branch has right child");
            let combined = node.bounds.union(leaf_bounds);
            let inheritance = twice(nonnegative_difference(
                perimeter(combined),
                perimeter(node.bounds),
            ));
            let parent_cost = twice(perimeter(combined));
            let left_cost = child_insertion_cost(self.nodes[left], leaf_bounds, inheritance);
            let right_cost = child_insertion_cost(self.nodes[right], leaf_bounds, inheritance);
            if parent_cost < left_cost && parent_cost < right_cost {
                return node_index;
            }
            node_index = if left_cost < right_cost {
                left
            } else if right_cost < left_cost {
                right
            } else if self.nodes[left].height <= self.nodes[right].height {
                left
            } else {
                right
            };
        }
    }

    fn fix_upwards(&mut self, mut node_index: Option<usize>) {
        while let Some(index) = node_index {
            let new_root = self.balance(index);
            node_index = self.nodes[new_root].parent;
        }
    }

    fn balance(&mut self, index: usize) -> usize {
        if self.nodes[index].is_leaf() {
            return index;
        }
        self.recompute(index);
        let left = self.nodes[index].left.expect("branch has left child");
        let right = self.nodes[index].right.expect("branch has right child");
        let balance = self.nodes[right].height - self.nodes[left].height;

        if balance > 1 {
            let right_left = self.nodes[right].left.expect("branch has left child");
            let right_right = self.nodes[right].right.expect("branch has right child");
            let old_parent = self.nodes[index].parent;
            self.replace_parent_link(old_parent, index, right);
            self.nodes[right].parent = old_parent;
            self.nodes[right].left = Some(index);
            self.nodes[index].parent = Some(right);

            if self.nodes[right_left].height > self.nodes[right_right].height {
                self.nodes[index].right = Some(right_right);
                self.nodes[right_right].parent = Some(index);
                self.nodes[right].right = Some(right_left);
                self.nodes[right_left].parent = Some(right);
            } else {
                self.nodes[index].right = Some(right_left);
                self.nodes[right_left].parent = Some(index);
                self.nodes[right].right = Some(right_right);
                self.nodes[right_right].parent = Some(right);
            }
            self.recompute(index);
            self.recompute(right);
            return right;
        }

        if balance < -1 {
            let left_left = self.nodes[left].left.expect("branch has left child");
            let left_right = self.nodes[left].right.expect("branch has right child");
            let old_parent = self.nodes[index].parent;
            self.replace_parent_link(old_parent, index, left);
            self.nodes[left].parent = old_parent;
            self.nodes[left].right = Some(index);
            self.nodes[index].parent = Some(left);

            if self.nodes[left_left].height > self.nodes[left_right].height {
                self.nodes[index].left = Some(left_right);
                self.nodes[left_right].parent = Some(index);
                self.nodes[left].left = Some(left_left);
                self.nodes[left_left].parent = Some(left);
            } else {
                self.nodes[index].left = Some(left_left);
                self.nodes[left_left].parent = Some(index);
                self.nodes[left].left = Some(left_right);
                self.nodes[left_right].parent = Some(left);
            }
            self.recompute(index);
            self.recompute(left);
            return left;
        }

        index
    }

    fn recompute(&mut self, index: usize) {
        if self.nodes[index].is_leaf() {
            self.nodes[index].height = 0;
            return;
        }
        let left = self.nodes[index].left.expect("branch has left child");
        let right = self.nodes[index].right.expect("branch has right child");
        self.nodes[index].bounds = self.nodes[left].bounds.union(self.nodes[right].bounds);
        self.nodes[index].height = 1 + self.nodes[left].height.max(self.nodes[right].height);
    }

    fn replace_child(&mut self, parent: usize, old_child: usize, new_child: usize) {
        if self.nodes[parent].left == Some(old_child) {
            self.nodes[parent].left = Some(new_child);
        } else {
            debug_assert_eq!(self.nodes[parent].right, Some(old_child));
            self.nodes[parent].right = Some(new_child);
        }
    }

    fn replace_parent_link(
        &mut self,
        parent: Option<usize>,
        old_child: usize,
        new_child: usize,
    ) {
        match parent {
            Some(parent) => self.replace_child(parent, old_child, new_child),
            None => self.root = Some(new_child),
        }
    }

    fn allocate(&mut self, node: Node) -> usize {
        if let Some(index) = self.free_nodes.pop() {
            self.nodes[index] = node;
            index
        } else {
            self.nodes.push(node);
            self.nodes.len() - 1
        }
    }

    fn release(&mut self, index: usize) {
        self.nodes[index] = Node::vacant();
        self.free_nodes.push(index);
    }

    /// Removing a leaf also releases its former parent. Relocate at most two
    /// tail nodes after balancing, so storage stays dense without rebuilding
    /// the hierarchy or visiting unrelated entity bounds.
    fn compact_released_slots(&mut self) {
        while let Some(hole) = self.free_nodes.pop() {
            while self.nodes.last().is_some_and(|node| {
                node.entity.is_none() && node.left.is_none() && node.right.is_none()
            }) {
                let tail = self.nodes.len() - 1;
                self.nodes.pop();
                if let Some(position) = self.free_nodes.iter().position(|slot| *slot == tail) {
                    self.free_nodes.swap_remove(position);
                }
            }
            if hole >= self.nodes.len() {
                continue;
            }
            let former_tail = self.nodes.len() - 1;
            let moved = self.nodes.pop().expect("an active tail remains");
            self.nodes[hole] = moved;
            match moved.parent {
                Some(parent) => self.replace_child(parent, former_tail, hole),
                None => self.root = Some(hole),
            }
            if let Some(left) = moved.left { self.nodes[left].parent = Some(hole); }
            if let Some(right) = moved.right { self.nodes[right].parent = Some(hole); }
            if let Some(entity) = moved.entity { self.leaves.insert(entity, hole); }
        }
        // Release historical peaks. This copies only the compact node array,
        // amortized over deletions; it does not rebuild bounds or traverse the
        // source drawing. Index-based links survive allocation relocation.
        if self.nodes.is_empty() || self.nodes.capacity() > self.nodes.len().saturating_mul(4) {
            self.nodes.shrink_to_fit();
        }
    }
}

fn validate_bounds(bounds: Bounds2) -> DraftResult<()> {
    if valid_bounds(bounds) {
        Ok(())
    } else {
        Err(DraftError::new(
            "invalid_spatial_bounds",
            "spatial bounds must be finite and ordered",
        ))
    }
}

fn valid_bounds(bounds: Bounds2) -> bool {
    bounds.min.is_finite()
        && bounds.max.is_finite()
        && bounds.min.x <= bounds.max.x
        && bounds.min.y <= bounds.max.y
}

fn relative(bounds: Bounds2, origin: Point2) -> Bounds2 {
    Bounds2 {
        min: Point2::new(bounds.min.x - origin.x, bounds.min.y - origin.y),
        max: Point2::new(bounds.max.x - origin.x, bounds.max.y - origin.y),
    }
}

fn centroid(bounds: Bounds2, x_axis: bool) -> f64 {
    let (min, max) = if x_axis {
        (bounds.min.x, bounds.max.x)
    } else {
        (bounds.min.y, bounds.max.y)
    };
    min * 0.5 + max * 0.5
}

fn centroid_axis(bounds: Bounds2, x_axis: bool) -> f64 {
    centroid(bounds, x_axis)
}

fn centroid_spread(entries: &[(DraftEntityId, Bounds2)]) -> (f64, f64) {
    let first = entries[0].1;
    let mut min_x = centroid(first, true);
    let mut max_x = min_x;
    let mut min_y = centroid(first, false);
    let mut max_y = min_y;
    for (_, bounds) in &entries[1..] {
        let x = centroid(*bounds, true);
        let y = centroid(*bounds, false);
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    (extent(min_x, max_x), extent(min_y, max_y))
}

fn extent(min: f64, max: f64) -> f64 {
    let value = max - min;
    if value.is_finite() {
        value.max(0.0)
    } else {
        f64::MAX
    }
}

fn perimeter(bounds: Bounds2) -> f64 {
    saturated_add(
        extent(bounds.min.x, bounds.max.x),
        extent(bounds.min.y, bounds.max.y),
    )
}

fn child_insertion_cost(node: Node, added: Bounds2, inherited: f64) -> f64 {
    let combined = node.bounds.union(added);
    let growth = nonnegative_difference(perimeter(combined), perimeter(node.bounds));
    let local = if node.is_leaf() {
        perimeter(combined)
    } else {
        growth
    };
    saturated_add(local, inherited)
}

fn nonnegative_difference(a: f64, b: f64) -> f64 {
    let value = a - b;
    if value.is_finite() {
        value.max(0.0)
    } else {
        f64::MAX
    }
}

fn twice(value: f64) -> f64 {
    saturated_add(value, value)
}

fn saturated_add(a: f64, b: f64) -> f64 {
    let value = a + b;
    if value.is_finite() {
        value
    } else {
        f64::MAX
    }
}

#[cfg(test)]
#[path = "cache_index_tests.rs"]
mod tests;
