//! Exact inherited dense limits exercised from a newly generated model.
//! These validation-only probes do not certify the augmented models as structures.
use structural_configurator::{FixtureKind, fixture, generate_building, to_structural_model};
use structural_core::{
    StructuralError, StructuralMemberId, StructuralNode, StructuralNodeId, validate,
};

#[test]
fn sixty_fifth_distinct_node_is_refused_before_dense_assembly() {
    let generated = generate_building(&fixture(FixtureKind::SteppedConnected)).unwrap();
    let mut model = to_structural_model(&generated).unwrap();
    let mut candidate = 1;
    while model.nodes.len() < 65 {
        let id = StructuralNodeId::new(candidate).unwrap();
        candidate += 1;
        if model.nodes.iter().any(|node| node.id == id) {
            continue;
        }
        model.nodes.push(StructuralNode {
            id,
            position_m: [candidate as f64, 100.0, 0.0],
        });
    }
    assert!(
        matches!(validate(&model), Err(StructuralError::CapacityExceeded {entity, count: 65, limit: 64}) if entity == "nodes")
    );
    assert_eq!(
        model.nodes.len(),
        65,
        "validation must not truncate the model"
    );
}

#[test]
fn hundred_twenty_ninth_member_is_refused_before_dense_assembly() {
    let generated = generate_building(&fixture(FixtureKind::SteppedConnected)).unwrap();
    let mut model = to_structural_model(&generated).unwrap();
    let template = model.members[0].clone();
    let mut candidate = 1;
    while model.members.len() < 129 {
        let id = StructuralMemberId::new(candidate).unwrap();
        candidate += 1;
        if model.members.iter().any(|member| member.id == id) {
            continue;
        }
        let mut member = template.clone();
        member.id = id;
        model.members.push(member);
    }
    assert!(
        matches!(validate(&model), Err(StructuralError::CapacityExceeded {entity, count: 129, limit: 128}) if entity == "members")
    );
    assert_eq!(
        model.members.len(),
        129,
        "validation must not truncate the model"
    );
}
