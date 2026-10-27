//! Incremental broad-phase spatial queries over derived element bounds.
//!
//! The index owns only bounds and BIM IDs/metadata. It does not depend on a
//! document, renderer, or viewer. Mutations touch one leaf and the O(log n)
//! path around it; queries visit only tree nodes whose AABBs overlap the query.

use crate::GeometryObservationError;
use ai_revit_core::{ClassId, ElementId, LevelId};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::time::Instant;
use viewer_core::{Aabb, Vec3};

/// A coarse, inclusive AABB query with optional semantic filters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpatialQuery {
    pub bounds: Aabb,
    pub class_id: Option<ClassId>,
    pub level_id: Option<LevelId>,
}

/// Matching BIM element IDs and work actually performed by one query.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpatialQueryResult {
    pub element_ids: Vec<ElementId>,
    pub visited_nodes: usize,
    pub tested_bounds: usize,
    pub duration_us: u64,
}

/// Cumulative mutations/queries and the most recent query's traversal counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpatialIndexMetrics {
    pub element_count: usize,
    pub node_count: usize,
    pub update_count: usize,
    pub query_count: usize,
    pub last_query_visited_nodes: usize,
    pub last_query_tested_bounds: usize,
}

#[derive(Clone, Debug)]
struct ElementData {
    id: ElementId,
    class_id: ClassId,
    level_id: Option<LevelId>,
}

#[derive(Clone, Debug)]
struct TreeNode {
    bounds: Aabb,
    parent: Option<usize>,
    left: Option<usize>,
    right: Option<usize>,
    height: i32,
    element: Option<ElementData>,
}

impl TreeNode {
    fn leaf(bounds: Aabb, element: ElementData) -> Self {
        Self {
            bounds,
            parent: None,
            left: None,
            right: None,
            height: 0,
            element: Some(element),
        }
    }

    fn branch(bounds: Aabb, parent: Option<usize>, left: usize, right: usize, height: i32) -> Self {
        Self {
            bounds,
            parent,
            left: Some(left),
            right: Some(right),
            height,
            element: None,
        }
    }

    fn is_leaf(&self) -> bool {
        self.element.is_some()
    }
}

/// A height-balanced dynamic AABB tree.
///
/// The arena reuses detached branch slots. Every element is stored once as a
/// leaf and the BTreeMap provides direct removal/update lookup.
pub struct SpatialIndex {
    nodes: Vec<Option<TreeNode>>,
    free_nodes: Vec<usize>,
    root: Option<usize>,
    leaf_by_element: BTreeMap<ElementId, usize>,
    metrics: Cell<SpatialIndexMetrics>,
}

impl Default for SpatialIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl SpatialIndex {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            free_nodes: Vec::new(),
            root: None,
            leaf_by_element: BTreeMap::new(),
            metrics: Cell::new(SpatialIndexMetrics::default()),
        }
    }

    /// Removes every indexed element. Cumulative query/update counters remain
    /// available; clearing a nonempty index records one mutation.
    pub fn clear(&mut self) {
        let changed = !self.leaf_by_element.is_empty();
        self.nodes.clear();
        self.free_nodes.clear();
        self.root = None;
        self.leaf_by_element.clear();

        let mut metrics = self.metrics.get();
        metrics.element_count = 0;
        metrics.node_count = 0;
        if changed {
            metrics.update_count = metrics.update_count.saturating_add(1);
        }
        self.metrics.set(metrics);
    }

    /// Inserts or replaces an element's bounds and semantic filters.
    ///
    /// Validation happens before the existing leaf is detached, so an invalid
    /// update leaves the previous indexed value intact.
    pub fn upsert(
        &mut self,
        id: ElementId,
        bounds: Aabb,
        class_id: ClassId,
        level_id: Option<LevelId>,
    ) -> Result<(), GeometryObservationError> {
        validate_bounds(&bounds, Some(id))?;

        if let Some(leaf) = self.leaf_by_element.get(&id).copied() {
            if self.node(leaf).bounds == bounds {
                let existing = self.node(leaf).element.as_ref().expect("indexed leaf");
                if existing.class_id == class_id && existing.level_id == level_id {
                    return Ok(());
                }
                self.node_mut(leaf).element = Some(ElementData {
                    id,
                    class_id,
                    level_id,
                });
                let mut metrics = self.metrics.get();
                metrics.update_count = metrics.update_count.saturating_add(1);
                self.metrics.set(metrics);
                return Ok(());
            }
            self.detach_leaf(leaf);
            {
                let node = self.node_mut(leaf);
                node.bounds = bounds;
                node.element = Some(ElementData {
                    id,
                    class_id,
                    level_id,
                });
                node.height = 0;
                node.left = None;
                node.right = None;
            }
            self.insert_leaf(leaf);
        } else {
            let leaf = self.allocate_node(TreeNode::leaf(
                bounds,
                ElementData {
                    id,
                    class_id,
                    level_id,
                },
            ));
            self.leaf_by_element.insert(id, leaf);
            self.insert_leaf(leaf);
        }

        self.record_mutation();
        self.sync_structure_metrics();
        Ok(())
    }

    /// Removes an element, returning whether it was present.
    pub fn remove(&mut self, id: ElementId) -> bool {
        let Some(leaf) = self.leaf_by_element.remove(&id) else {
            return false;
        };
        self.detach_leaf(leaf);
        self.release_node(leaf);
        self.record_mutation();
        self.sync_structure_metrics();
        true
    }

    /// Returns every element whose derived bounds overlap the query bounds.
    /// Touching faces, edges, and points count as intersections.
    pub fn bounds_query(
        &self,
        query: &SpatialQuery,
    ) -> Result<SpatialQueryResult, GeometryObservationError> {
        let started = Instant::now();
        if let Err(error) = validate_bounds(&query.bounds, None) {
            self.record_query(0, 0);
            return Err(error);
        }

        let mut result = SpatialQueryResult::default();
        let mut stack = Vec::new();
        let mut leaf_indices = Vec::new();
        if let Some(root) = self.root {
            stack.push(root);
        }
        while let Some(index) = stack.pop() {
            result.visited_nodes = result.visited_nodes.saturating_add(1);
            let node = self.node(index);
            if node.is_leaf() {
                // Defer leaf tests so a large, already-pruned candidate set can
                // be checked in one batch. Internal nodes still prune on CPU.
                leaf_indices.push(index);
                continue;
            }

            result.tested_bounds = result.tested_bounds.saturating_add(1);
            if !overlaps_inclusive(node.bounds, query.bounds) {
                continue;
            }

            // Internal nodes always have both children.
            stack.push(node.right.expect("branch has right child"));
            stack.push(node.left.expect("branch has left child"));
        }

        let gpu_matches = if crate::gpu::aabb_batch_admitted(leaf_indices.len()) {
            let packed_bounds: Vec<[f64; 6]> = leaf_indices
                .iter()
                .map(|index| {
                    let bounds = self.node(*index).bounds;
                    [
                        bounds.min.x,
                        bounds.min.y,
                        bounds.min.z,
                        bounds.max.x,
                        bounds.max.y,
                        bounds.max.z,
                    ]
                })
                .collect();
            let query_bounds = [
                query.bounds.min.x,
                query.bounds.min.y,
                query.bounds.min.z,
                query.bounds.max.x,
                query.bounds.max.y,
                query.bounds.max.z,
            ];
            crate::gpu::aabb_overlaps(&packed_bounds, query_bounds)
        } else {
            None
        };

        // Keep tested_bounds as one logical test per visited node. The GPU
        // batch counts once per leaf; positive results get a CPU confirmation.
        result.tested_bounds = result.tested_bounds.saturating_add(leaf_indices.len());
        if let Some(matches) = gpu_matches.filter(|matches| matches.len() == leaf_indices.len()) {
            for (index, overlaps) in leaf_indices.into_iter().zip(matches) {
                if !overlaps {
                    continue;
                }
                let node = self.node(index);
                if !overlaps_inclusive(node.bounds, query.bounds) {
                    continue;
                }
                let Some(element) = node.element.as_ref() else {
                    continue;
                };
                let class_matches = query
                    .class_id
                    .as_ref()
                    .is_none_or(|class_id| class_id == &element.class_id);
                let level_matches = query
                    .level_id
                    .as_ref()
                    .is_none_or(|level_id| Some(level_id) == element.level_id.as_ref());
                if class_matches && level_matches {
                    result.element_ids.push(element.id);
                }
            }
        } else {
            for index in leaf_indices {
                let node = self.node(index);
                if !overlaps_inclusive(node.bounds, query.bounds) {
                    continue;
                }
                let Some(element) = node.element.as_ref() else {
                    continue;
                };
                let class_matches = query
                    .class_id
                    .as_ref()
                    .is_none_or(|class_id| class_id == &element.class_id);
                let level_matches = query
                    .level_id
                    .as_ref()
                    .is_none_or(|level_id| Some(level_id) == element.level_id.as_ref());
                if class_matches && level_matches {
                    result.element_ids.push(element.id);
                }
            }
        }

        result.element_ids.sort_unstable();
        result.duration_us = elapsed_micros(started);
        self.record_query(result.visited_nodes, result.tested_bounds);
        Ok(result)
    }

    /// Performs the same coarse AABB test as bounds_query.
    pub fn intersects_query(
        &self,
        query: &SpatialQuery,
    ) -> Result<SpatialQueryResult, GeometryObservationError> {
        self.bounds_query(query)
    }

    pub fn metrics(&self) -> SpatialIndexMetrics {
        self.metrics.get()
    }

    fn node(&self, index: usize) -> &TreeNode {
        self.nodes[index]
            .as_ref()
            .expect("active tree node exists in arena")
    }

    fn node_mut(&mut self, index: usize) -> &mut TreeNode {
        self.nodes[index]
            .as_mut()
            .expect("active tree node exists in arena")
    }

    fn allocate_node(&mut self, node: TreeNode) -> usize {
        if let Some(index) = self.free_nodes.pop() {
            self.nodes[index] = Some(node);
            index
        } else {
            let index = self.nodes.len();
            self.nodes.push(Some(node));
            index
        }
    }

    fn release_node(&mut self, index: usize) {
        self.nodes[index] = None;
        self.free_nodes.push(index);
    }

    fn sync_structure_metrics(&self) {
        let mut metrics = self.metrics.get();
        metrics.element_count = self.leaf_by_element.len();
        metrics.node_count = self.nodes.len().saturating_sub(self.free_nodes.len());
        self.metrics.set(metrics);
    }

    fn record_mutation(&self) {
        let mut metrics = self.metrics.get();
        metrics.update_count = metrics.update_count.saturating_add(1);
        self.metrics.set(metrics);
    }

    fn record_query(&self, visited_nodes: usize, tested_bounds: usize) {
        let mut metrics = self.metrics.get();
        metrics.query_count = metrics.query_count.saturating_add(1);
        metrics.last_query_visited_nodes = visited_nodes;
        metrics.last_query_tested_bounds = tested_bounds;
        self.metrics.set(metrics);
    }

    /// Removes a leaf from the tree but keeps its arena slot and ID map entry.
    fn detach_leaf(&mut self, leaf: usize) {
        let Some(parent) = self.node(leaf).parent else {
            if self.root == Some(leaf) {
                self.root = None;
            }
            return;
        };

        let (sibling, grandparent) = {
            let parent_node = self.node(parent);
            let sibling = if parent_node.left == Some(leaf) {
                parent_node.right.expect("branch has right child")
            } else {
                debug_assert_eq!(parent_node.right, Some(leaf));
                parent_node.left.expect("branch has left child")
            };
            (sibling, parent_node.parent)
        };

        if let Some(grandparent) = grandparent {
            {
                let parent_node = self.node_mut(grandparent);
                if parent_node.left == Some(parent) {
                    parent_node.left = Some(sibling);
                } else {
                    debug_assert_eq!(parent_node.right, Some(parent));
                    parent_node.right = Some(sibling);
                }
            }
            self.node_mut(sibling).parent = Some(grandparent);
            self.release_node(parent);
            self.rebalance_upwards(Some(grandparent));
        } else {
            self.root = Some(sibling);
            self.node_mut(sibling).parent = None;
            self.release_node(parent);
        }

        self.node_mut(leaf).parent = None;
    }

    fn insert_leaf(&mut self, leaf: usize) {
        let Some(root) = self.root else {
            self.root = Some(leaf);
            self.node_mut(leaf).parent = None;
            return;
        };

        let leaf_bounds = self.node(leaf).bounds;
        let mut sibling = root;
        loop {
            let (is_leaf, node_bounds, left, right, height) = {
                let node = self.node(sibling);
                (
                    node.is_leaf(),
                    node.bounds,
                    node.left,
                    node.right,
                    node.height,
                )
            };
            if is_leaf {
                break;
            }

            let combined = node_bounds.union(leaf_bounds);
            let combined_measure = bounds_measure(combined);
            let inheritance_cost = twice_saturated(positive_difference(
                combined_measure,
                bounds_measure(node_bounds),
            ));
            let parent_cost = twice_saturated(combined_measure);
            let left = left.expect("branch has left child");
            let right = right.expect("branch has right child");
            let left_cost = self.insertion_cost(left, leaf_bounds, inheritance_cost);
            let right_cost = self.insertion_cost(right, leaf_bounds, inheritance_cost);

            if parent_cost < left_cost && parent_cost < right_cost {
                break;
            }

            sibling = if left_cost < right_cost {
                left
            } else if right_cost < left_cost {
                right
            } else {
                let left_height = self.node(left).height;
                let right_height = self.node(right).height;
                if left_height <= right_height {
                    left
                } else {
                    right
                }
            };

            debug_assert!(height >= self.node(sibling).height);
        }

        let old_parent = self.node(sibling).parent;
        let bounds = self.node(sibling).bounds.union(leaf_bounds);
        let height = self.node(sibling).height.saturating_add(1);
        let new_parent =
            self.allocate_node(TreeNode::branch(bounds, old_parent, sibling, leaf, height));
        self.node_mut(sibling).parent = Some(new_parent);
        self.node_mut(leaf).parent = Some(new_parent);

        if let Some(parent) = old_parent {
            let parent_node = self.node_mut(parent);
            if parent_node.left == Some(sibling) {
                parent_node.left = Some(new_parent);
            } else {
                debug_assert_eq!(parent_node.right, Some(sibling));
                parent_node.right = Some(new_parent);
            }
        } else {
            self.root = Some(new_parent);
        }

        self.rebalance_upwards(Some(new_parent));
    }

    fn insertion_cost(&self, child: usize, leaf_bounds: Aabb, inheritance_cost: f64) -> f64 {
        let child_node = self.node(child);
        let union_measure = bounds_measure(child_node.bounds.union(leaf_bounds));
        let child_cost = if child_node.is_leaf() {
            twice_saturated(union_measure)
        } else {
            twice_saturated(positive_difference(
                union_measure,
                bounds_measure(child_node.bounds),
            ))
        };
        add_saturated(child_cost, inheritance_cost)
    }

    fn rebalance_upwards(&mut self, mut cursor: Option<usize>) {
        while let Some(index) = cursor {
            self.refit(index);
            let subtree_root = self.balance(index);
            cursor = self.node(subtree_root).parent;
        }
    }

    fn refit(&mut self, index: usize) {
        let (left, right, is_leaf) = {
            let node = self.node(index);
            (node.left, node.right, node.is_leaf())
        };
        if is_leaf {
            return;
        }

        let left = left.expect("branch has left child");
        let right = right.expect("branch has right child");
        let bounds = self.node(left).bounds.union(self.node(right).bounds);
        let height = 1 + self.node(left).height.max(self.node(right).height);
        let node = self.node_mut(index);
        node.bounds = bounds;
        node.height = height;
    }

    fn balance(&mut self, index: usize) -> usize {
        if self.node(index).is_leaf() {
            return index;
        }

        let (left, right) = {
            let node = self.node(index);
            (
                node.left.expect("branch has left child"),
                node.right.expect("branch has right child"),
            )
        };
        let balance = self.node(right).height - self.node(left).height;
        if balance > 1 {
            let right_left = self.node(right).left.expect("branch has left child");
            let right_right = self.node(right).right.expect("branch has right child");
            if self.node(right_left).height > self.node(right_right).height {
                self.rotate_right(right);
            }
            return self.rotate_left(index);
        }
        if balance < -1 {
            let left_left = self.node(left).left.expect("branch has left child");
            let left_right = self.node(left).right.expect("branch has right child");
            if self.node(left_right).height > self.node(left_left).height {
                self.rotate_left(left);
            }
            return self.rotate_right(index);
        }
        index
    }

    fn replace_subtree_root(&mut self, old: usize, new: usize, parent: Option<usize>) {
        self.node_mut(new).parent = parent;
        if let Some(parent) = parent {
            let parent_node = self.node_mut(parent);
            if parent_node.left == Some(old) {
                parent_node.left = Some(new);
            } else {
                debug_assert_eq!(parent_node.right, Some(old));
                parent_node.right = Some(new);
            }
        } else {
            self.root = Some(new);
        }
    }

    fn rotate_left(&mut self, index: usize) -> usize {
        let right = self
            .node(index)
            .right
            .expect("right-heavy node has right child");
        let (middle, parent) = (self.node(right).left, self.node(index).parent);
        let middle = middle.expect("internal right child has left child");

        self.replace_subtree_root(index, right, parent);
        self.node_mut(right).left = Some(index);
        self.node_mut(index).parent = Some(right);
        self.node_mut(index).right = Some(middle);
        self.node_mut(middle).parent = Some(index);

        self.refit(index);
        self.refit(right);
        right
    }

    fn rotate_right(&mut self, index: usize) -> usize {
        let left = self
            .node(index)
            .left
            .expect("left-heavy node has left child");
        let (middle, parent) = (self.node(left).right, self.node(index).parent);
        let middle = middle.expect("internal left child has right child");

        self.replace_subtree_root(index, left, parent);
        self.node_mut(left).right = Some(index);
        self.node_mut(index).parent = Some(left);
        self.node_mut(index).left = Some(middle);
        self.node_mut(middle).parent = Some(index);

        self.refit(index);
        self.refit(left);
        left
    }
}

fn validate_bounds(
    bounds: &Aabb,
    element_id: Option<ElementId>,
) -> Result<(), GeometryObservationError> {
    if !bounds.min.is_finite() || !bounds.max.is_finite() {
        return Err(GeometryObservationError::new(
            "non_finite_bounds",
            element_id,
            "spatial bounds must contain only finite coordinates",
        ));
    }
    if bounds.min.x > bounds.max.x || bounds.min.y > bounds.max.y || bounds.min.z > bounds.max.z {
        return Err(GeometryObservationError::new(
            "invalid_bounds",
            element_id,
            "spatial bounds minimum must not exceed its maximum",
        ));
    }
    Ok(())
}

fn overlaps_inclusive(left: Aabb, right: Aabb) -> bool {
    left.min.x <= right.max.x
        && left.max.x >= right.min.x
        && left.min.y <= right.max.y
        && left.max.y >= right.min.y
        && left.min.z <= right.max.z
        && left.max.z >= right.min.z
}

/// A bounded surface-area heuristic. The extent-sum fallback keeps flat or
/// line-shaped finite boxes useful for choosing insertion siblings.
fn bounds_measure(bounds: Aabb) -> f64 {
    let extent = Vec3::new(
        safe_extent(bounds.min.x, bounds.max.x),
        safe_extent(bounds.min.y, bounds.max.y),
        safe_extent(bounds.min.z, bounds.max.z),
    );
    let area = twice_saturated(add_saturated(
        add_saturated(
            multiply_saturated(extent.x, extent.y),
            multiply_saturated(extent.y, extent.z),
        ),
        multiply_saturated(extent.z, extent.x),
    ));
    if area == 0.0 {
        // A line or point has zero surface area, but its nonzero extent must
        // still influence which sibling receives a new leaf.
        extent.max_component()
    } else {
        area
    }
}

fn multiply_saturated(left: f64, right: f64) -> f64 {
    let product = left * right;
    if product.is_finite() {
        product
    } else {
        f64::MAX
    }
}

fn safe_extent(minimum: f64, maximum: f64) -> f64 {
    let extent = maximum - minimum;
    if extent.is_finite() { extent } else { f64::MAX }
}

fn positive_difference(left: f64, right: f64) -> f64 {
    (left - right).max(0.0)
}

fn twice_saturated(value: f64) -> f64 {
    if value >= f64::MAX / 2.0 {
        f64::MAX
    } else {
        value * 2.0
    }
}

fn add_saturated(left: f64, right: f64) -> f64 {
    if left >= f64::MAX - right {
        f64::MAX
    } else {
        left + right
    }
}

fn elapsed_micros(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(minimum: Vec3, maximum: Vec3) -> Aabb {
        Aabb {
            min: minimum,
            max: maximum,
        }
    }

    fn box_at(x: f64, y: f64, z: f64, size: f64) -> Aabb {
        bounds(Vec3::new(x, y, z), Vec3::new(x + size, y + size, z + size))
    }

    fn query(bounds: Aabb) -> SpatialQuery {
        SpatialQuery {
            bounds,
            class_id: None,
            level_id: None,
        }
    }

    fn class(name: &str) -> ClassId {
        ClassId::new(name)
    }

    #[test]
    fn insert_remove_update_and_metrics_are_incremental() {
        let mut index = SpatialIndex::new();
        let first = ElementId::new(1);
        let second = ElementId::new(2);
        index
            .upsert(first, box_at(0.0, 0.0, 0.0, 1.0), class("wall"), None)
            .unwrap();
        index
            .upsert(second, box_at(10.0, 0.0, 0.0, 1.0), class("floor"), None)
            .unwrap();

        assert_eq!(
            index
                .bounds_query(&query(box_at(0.0, 0.0, 0.0, 1.0)))
                .unwrap()
                .element_ids,
            vec![first]
        );
        index
            .upsert(first, box_at(20.0, 0.0, 0.0, 1.0), class("door"), None)
            .unwrap();
        assert!(
            index
                .bounds_query(&query(box_at(0.0, 0.0, 0.0, 1.0)))
                .unwrap()
                .element_ids
                .is_empty()
        );
        assert_eq!(
            index
                .bounds_query(&query(box_at(20.0, 0.0, 0.0, 1.0)))
                .unwrap()
                .element_ids,
            vec![first]
        );

        assert!(index.remove(second));
        assert!(!index.remove(second));
        assert_eq!(index.metrics().element_count, 1);
        assert_eq!(index.metrics().node_count, 1);
        assert_eq!(index.metrics().update_count, 4);
        assert!(index.remove(first));
        assert_eq!(index.metrics().element_count, 0);
        assert_eq!(index.metrics().node_count, 0);
    }

    #[test]
    fn filters_and_touching_bounds_match_in_sorted_id_order() {
        let mut index = SpatialIndex::new();
        let bounds = box_at(0.0, 0.0, 0.0, 1.0);
        let level_one = LevelId::new(1);
        let level_two = LevelId::new(2);
        index
            .upsert(ElementId::new(4), bounds, class("wall"), Some(level_one))
            .unwrap();
        index
            .upsert(ElementId::new(2), bounds, class("floor"), Some(level_one))
            .unwrap();
        index
            .upsert(ElementId::new(1), bounds, class("wall"), Some(level_two))
            .unwrap();

        let mut class_query = query(box_at(1.0, 1.0, 1.0, 1.0));
        class_query.class_id = Some(class("wall"));
        assert_eq!(
            index.bounds_query(&class_query).unwrap().element_ids,
            vec![ElementId::new(1), ElementId::new(4)]
        );

        let mut level_query = query(bounds);
        level_query.level_id = Some(level_one);
        assert_eq!(
            index.intersects_query(&level_query).unwrap().element_ids,
            vec![ElementId::new(2), ElementId::new(4)]
        );

        let mut combined_query = query(bounds);
        combined_query.class_id = Some(class("wall"));
        combined_query.level_id = Some(level_two);
        assert_eq!(
            index.bounds_query(&combined_query).unwrap().element_ids,
            vec![ElementId::new(1)]
        );
    }

    #[test]
    fn separated_ten_thousand_elements_prune_query_traversal() {
        let mut index = SpatialIndex::new();
        for offset in 0..10_000u64 {
            let x = offset as f64 * 4.0;
            index
                .upsert(
                    ElementId::new(offset + 1),
                    box_at(x, 0.0, 0.0, 1.0),
                    class("wall"),
                    None,
                )
                .unwrap();
        }

        let target_offset = 5_432u64;
        let result = index
            .bounds_query(&query(box_at(target_offset as f64 * 4.0, 0.0, 0.0, 1.0)))
            .unwrap();
        assert_eq!(result.element_ids, vec![ElementId::new(target_offset + 1)]);
        println!(
            "10k linear spatial query visited {} nodes",
            result.visited_nodes
        );
        assert!(
            result.visited_nodes < 100,
            "visited {} of 19,999 nodes",
            result.visited_nodes
        );
        assert_eq!(result.tested_bounds, result.visited_nodes);
        let metrics = index.metrics();
        assert_eq!(metrics.element_count, 10_000);
        assert_eq!(metrics.node_count, 19_999);
        assert_eq!(metrics.query_count, 1);
        assert_eq!(metrics.last_query_visited_nodes, result.visited_nodes);
        assert_eq!(metrics.last_query_tested_bounds, result.tested_bounds);
    }

    #[test]
    fn shuffled_two_dimensional_grid_prunes_query_traversal() {
        let mut index = SpatialIndex::new();
        for ordinal in 0..10_000u64 {
            // 7,919 is coprime to 10,000: insertion order traverses the whole
            // grid without giving the tree spatially sorted inputs.
            let position = (ordinal * 7_919) % 10_000;
            let x = (position % 100) as f64 * 4.0;
            let y = (position / 100) as f64 * 4.0;
            index
                .upsert(
                    ElementId::new(position + 1),
                    box_at(x, y, 0.0, 1.0),
                    class("wall"),
                    None,
                )
                .unwrap();
        }
        let result = index
            .bounds_query(&query(box_at(54.0 * 4.0, 32.0 * 4.0, 0.0, 1.0)))
            .unwrap();
        assert_eq!(result.element_ids, vec![ElementId::new(3_255)]);
        println!(
            "10k shuffled grid query visited {} nodes",
            result.visited_nodes
        );
        assert!(
            result.visited_nodes < 200,
            "visited {} of 19,999 nodes",
            result.visited_nodes
        );
        assert_eq!(result.tested_bounds, result.visited_nodes);
    }

    #[test]
    fn finite_degenerate_bounds_are_valid_and_inclusive() {
        let mut index = SpatialIndex::new();
        let point = bounds(Vec3::new(3.0, 4.0, 5.0), Vec3::new(3.0, 4.0, 5.0));
        index
            .upsert(ElementId::new(7), point, class("marker"), None)
            .unwrap();

        assert_eq!(
            index.bounds_query(&query(point)).unwrap().element_ids,
            vec![ElementId::new(7)]
        );
    }

    #[test]
    fn invalid_bounds_return_structured_errors_without_replacing_existing_data() {
        let mut index = SpatialIndex::new();
        let id = ElementId::new(8);
        index
            .upsert(id, box_at(1.0, 2.0, 3.0, 1.0), class("wall"), None)
            .unwrap();

        let non_finite = bounds(Vec3::new(f64::NAN, 0.0, 0.0), Vec3::new(f64::NAN, 1.0, 1.0));
        let error = index
            .upsert(id, non_finite, class("door"), None)
            .unwrap_err();
        assert_eq!(error.error_code, "non_finite_bounds");
        assert_eq!(error.element_id, Some(id));

        let reversed = bounds(Vec3::new(2.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0));
        let error = index.upsert(id, reversed, class("door"), None).unwrap_err();
        assert_eq!(error.error_code, "invalid_bounds");
        assert_eq!(error.element_id, Some(id));

        assert_eq!(
            index
                .bounds_query(&query(box_at(1.0, 2.0, 3.0, 1.0)))
                .unwrap()
                .element_ids,
            vec![id]
        );

        let error = index.bounds_query(&query(reversed)).unwrap_err();
        assert_eq!(error.error_code, "invalid_bounds");
        assert_eq!(error.element_id, None);
        assert_eq!(index.metrics().query_count, 2);
        assert_eq!(index.metrics().last_query_visited_nodes, 0);
    }

    #[test]
    fn public_query_and_metrics_types_serialize() {
        let value = SpatialQuery {
            bounds: box_at(0.0, 0.0, 0.0, 1.0),
            class_id: Some(class("wall")),
            level_id: Some(LevelId::new(3)),
        };
        let encoded = serde_json::to_string(&value).unwrap();
        let decoded: SpatialQuery = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, value);

        let metrics = SpatialIndexMetrics {
            element_count: 1,
            node_count: 1,
            update_count: 2,
            query_count: 4,
            last_query_visited_nodes: 1,
            last_query_tested_bounds: 1,
        };
        let encoded = serde_json::to_string(&metrics).unwrap();
        let decoded: SpatialIndexMetrics = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, metrics);
    }
}
