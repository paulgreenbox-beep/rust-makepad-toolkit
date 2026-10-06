use rudra_runtime::{EntityId, Registry};

#[derive(Debug)]
struct Project {
    name: &'static str,
}

#[derive(Debug)]
struct Scene {
    name: &'static str,
    project: EntityId<Project>,
}

#[derive(Debug)]
struct Asset {
    name: &'static str,
    scene: EntityId<Scene>,
}

fn build_demo() -> (Registry<Project>, Registry<Scene>, Registry<Asset>) {
    let mut projects = Registry::new();
    let mut scenes = Registry::new();
    let mut assets = Registry::new();

    let project = projects.insert(Project { name: "Atlas" }).unwrap();
    let scene = scenes
        .insert(Scene {
            name: "Landing",
            project,
        })
        .unwrap();
    assets
        .insert(Asset {
            name: "HeroTexture",
            scene,
        })
        .unwrap();

    (projects, scenes, assets)
}

fn main() {
    let (projects, scenes, assets) = build_demo();
    let (_, project) = projects.iter().next().unwrap();
    let (_, scene) = scenes.iter().next().unwrap();
    let (_, asset) = assets.iter().next().unwrap();
    println!(
        "project={} scene={} project_id={} asset={} scene_id={}",
        project.name, scene.name, scene.project, asset.name, asset.scene
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_scene_asset_use_distinct_typed_registries() {
        let (projects, scenes, assets) = build_demo();
        assert_eq!(projects.len(), 1);
        assert_eq!(scenes.len(), 1);
        assert_eq!(assets.len(), 1);
        let (_, scene) = scenes.iter().next().unwrap();
        assert!(projects.contains(scene.project));
        let (_, asset) = assets.iter().next().unwrap();
        assert!(scenes.contains(asset.scene));
    }
}
