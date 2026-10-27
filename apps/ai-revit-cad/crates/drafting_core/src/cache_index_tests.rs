use super::MaintainedIndex;
use crate::{Bounds2, DraftEntityId, Point2};
use std::collections::BTreeSet;

fn bounds(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Bounds2 {
    Bounds2 {
        min: Point2::new(min_x, min_y),
        max: Point2::new(max_x, max_y),
    }
}

fn hit_ids(index: &MaintainedIndex, query: Bounds2) -> BTreeSet<DraftEntityId> {
    index
        .query_relative(Point2::default(), query)
        .ids
        .into_iter()
        .collect()
}

#[test]
fn bulk_and_incremental_queries_match_brute_force_for_mixed_aabbs() {
    let mut items = Vec::new();
    for i in 0..384_u64 {
        let x = ((i * 37 % 101) as f64 - 50.0) * 13.0;
        let y = ((i * 53 % 127) as f64 - 63.0) * 9.0;
        let (width, height) = match i % 6 {
            0 => (0.0, 0.0),          // point-like bound
            1 => (0.001, 80.0),       // vertical thin bound
            2 => (120.0, 0.001),      // horizontal thin bound
            3 => (900.0, 600.0),      // wide circle/arc-like envelope
            4 => (1.0, 1.0),          // short segment
            _ => (35.0, 210.0),       // partial-arc-like envelope
        };
        items.push((
            DraftEntityId::new(i + 1),
            bounds(x, y, x + width, y + height),
        ));
    }

    let mut index = MaintainedIndex::bulk_load(items.clone()).unwrap();
    let queries = [
        bounds(-1000.0, -1000.0, -700.0, -700.0),
        bounds(-25.0, -25.0, 25.0, 25.0),
        bounds(320.0, -180.0, 370.0, 220.0),
        bounds(-1500.0, 0.0, 1500.0, 1.0),
        bounds(800.0, 500.0, 1200.0, 900.0),
        bounds(-10000.0, -10000.0, 10000.0, 10000.0),
    ];

    for query in queries {
        let expected: BTreeSet<_> = items
            .iter()
            .filter(|(_, item_bounds)| item_bounds.intersects(query))
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(hit_ids(&index, query), expected);
    }

    // Exercise rotations and ancestor-bound maintenance after the balanced
    // bulk build, then compare the changed tree to the same brute-force rule.
    for i in 0..48_u64 {
        let id = DraftEntityId::new(i + 10_000);
        let x = i as f64 * 17.0 - 300.0;
        index
            .upsert(id, bounds(x, -x, x + 0.5, -x + 12.0))
            .unwrap();
        items.push((id, bounds(x, -x, x + 0.5, -x + 12.0)));
    }
    for query in queries {
        let expected: BTreeSet<_> = items
            .iter()
            .filter(|(_, item_bounds)| item_bounds.intersects(query))
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(hit_ids(&index, query), expected);
    }
}

#[test]
fn upsert_and_remove_drop_old_bounds_and_reuse_freed_nodes() {
    let moving = DraftEntityId::new(7);
    let fixed = DraftEntityId::new(8);
    let mut index = MaintainedIndex::new();
    index.upsert(moving, bounds(-2.0, -2.0, 2.0, 2.0)).unwrap();
    index.upsert(fixed, bounds(40.0, 40.0, 41.0, 41.0)).unwrap();
    assert!(hit_ids(&index, bounds(-1.0, -1.0, 1.0, 1.0)).contains(&moving));

    index.upsert(moving, bounds(100.0, 100.0, 102.0, 102.0)).unwrap();
    assert!(!hit_ids(&index, bounds(-1.0, -1.0, 1.0, 1.0)).contains(&moving));
    assert!(hit_ids(&index, bounds(100.5, 100.5, 101.0, 101.0)).contains(&moving));
    assert_eq!(index.bounds(), Some(bounds(40.0, 40.0, 102.0, 102.0)));

    assert!(index.remove(moving));
    assert!(!index.remove(moving));
    assert!(!hit_ids(&index, bounds(100.0, 100.0, 102.0, 102.0)).contains(&moving));
    assert_eq!(index.bounds(), Some(bounds(40.0, 40.0, 41.0, 41.0)));

    let restored = DraftEntityId::new(9);
    index
        .upsert(restored, bounds(-20.0, -20.0, -19.0, -19.0))
        .unwrap();
    assert!(hit_ids(&index, bounds(-20.0, -20.0, -19.0, -19.0)).contains(&restored));
}

#[test]
fn relative_query_preserves_small_aperture_at_large_world_origin() {
    let origin = Point2::new(1.0e9, -1.0e9);
    let id = DraftEntityId::new(1);
    let near = bounds(
        origin.x + 0.0001,
        origin.y + 0.0001,
        origin.x + 0.0002,
        origin.y + 0.0002,
    );
    let far = bounds(
        origin.x + 0.01,
        origin.y + 0.01,
        origin.x + 0.02,
        origin.y + 0.02,
    );
    let index = MaintainedIndex::bulk_load([
        (id, near),
        (DraftEntityId::new(2), far),
    ])
    .unwrap();

    let query = bounds(0.000099, 0.000099, 0.000101, 0.000101);
    assert_eq!(index.query_relative(origin, query).ids, [id]);
}

#[test]
fn sparse_twenty_thousand_entity_query_prunes_subtrees() {
    let count = 20_000_u64;
    let items = (0..count).map(|i| {
        let x = (i % 200) as f64 * 10.0;
        let y = (i / 200) as f64 * 10.0;
        (DraftEntityId::new(i + 1), bounds(x, y, x + 0.25, y + 0.25))
    });
    let index = MaintainedIndex::bulk_load(items).unwrap();
    let target = 12_345_u64;
    let x = (target % 200) as f64 * 10.0;
    let y = (target / 200) as f64 * 10.0;
    let result = index.query_relative(
        Point2::default(),
        bounds(x + 0.1, y + 0.1, x + 0.15, y + 0.15),
    );

    assert_eq!(result.ids, [DraftEntityId::new(target + 1)]);
    assert!(result.leaves_examined >= 1 && result.leaves_examined < count / 8);
    assert!(result.nodes_visited < count / 8, "visited {} nodes", result.nodes_visited);
}

#[test]
fn rejected_leaf_is_included_in_actual_examined_count() {
    let index = MaintainedIndex::bulk_load([(
        DraftEntityId::new(1), bounds(100.0, 100.0, 101.0, 101.0),
    )]).unwrap();
    let result = index.query_relative(Point2::default(), bounds(0.0, 0.0, 1.0, 1.0));
    assert!(result.ids.is_empty());
    assert_eq!(result.nodes_visited, 1);
    assert_eq!(result.leaves_examined, 1);
    assert_eq!(index.allocated_nodes(), 1);
}

#[test]
fn churn_reclaims_slots_and_preserves_all_relocated_tree_links() {
    let mut expected = std::collections::BTreeMap::new();
    let mut index = MaintainedIndex::new();
    for step in 0..768u64 {
        let id = DraftEntityId::new(step % 97 + 1);
        if step % 4 == 0 {
            let existed = expected.remove(&id).is_some();
            assert_eq!(index.remove(id), existed);
        } else {
            let x = ((step * 29) % 101) as f64;
            let b = bounds(x, -x, x + 0.5, -x + 0.25);
            expected.insert(id, b);
            index.upsert(id, b).unwrap();
        }
        assert_eq!(index.allocated_nodes(), expected.len().saturating_mul(2).saturating_sub(1));
        assert!(index.reserved_nodes() <= index.allocated_nodes().max(1) * 4);
        let query = bounds(10., -90., 90., -10.);
        let hits: BTreeSet<_> = expected.iter().filter(|(_, b)| b.intersects(query)).map(|(id, _)| *id).collect();
        assert_eq!(hit_ids(&index, query), hits);
        let all_bounds = expected.values().copied().reduce(|a, b| a.union(b));
        assert_eq!(index.bounds(), all_bounds);
    }
    for id in expected.keys().copied().collect::<Vec<_>>() {
        assert!(index.remove(id));
    }
    assert_eq!(index.allocated_nodes(), 0);
    assert_eq!(index.reserved_nodes(), 0);
    assert_eq!(index.bounds(), None);
}

#[test]
fn invalid_bounds_are_rejected_without_changing_existing_index() {
    let id = DraftEntityId::new(5);
    let original = bounds(-3.0, -2.0, -1.0, 4.0);
    let mut index = MaintainedIndex::new();
    index.upsert(id, original).unwrap();

    let reversed = bounds(2.0, -1.0, 1.0, 1.0);
    let error = index.upsert(id, reversed).unwrap_err();
    assert_eq!(error.code, "invalid_spatial_bounds");
    assert_eq!(index.bounds(), Some(original));
    assert_eq!(hit_ids(&index, original), [id].into_iter().collect());

    let non_finite = bounds(0.0, 0.0, f64::INFINITY, 1.0);
    assert_eq!(
        MaintainedIndex::bulk_load([(DraftEntityId::new(1), non_finite)])
            .unwrap_err()
            .code,
        "invalid_spatial_bounds"
    );
    assert_eq!(
        MaintainedIndex::bulk_load([(id, original), (id, original)])
            .unwrap_err()
            .code,
        "duplicate_spatial_entity"
    );
}
