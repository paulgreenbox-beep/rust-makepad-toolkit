use drafting_core::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::f64::consts::{PI, TAU};

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn snapshot_metadata(display: &DisplaySnapshot, snapshot: &DraftSnapshot) -> Value {
    json!({"document_id":display.document_id,"document_revision":display.document_revision,"drawing_id":display.drawing_id,"drawing_revision":display.source_revision,
        "camera":display.camera,"viewport":display.viewport,"visible_layer_ids":display.visible_layer_ids,"selection":display.selected_ids,"hover_id":display.hover_id,"snapshot_token":display.token,
        "render_quality":{"representation":display.representation_quality,"max_chord_error_physical_px":display.pixel_error,"dpi_factor":display.dpi_factor},
        "source_sha256":sha256(&serde_json::to_vec(snapshot).unwrap_or_default()),"source_fingerprint":display.source_fingerprint,"primitive_count":display.primitives.len(),"png_renderer":"bounded_cpu_display_snapshot","units":"metre"})
}
pub fn id_map(display: &DisplaySnapshot, snapshot: &DraftSnapshot) -> Value {
    json!({"metadata":snapshot_metadata(display,snapshot),"primitives":display.primitives.iter().map(|p|json!({"primitive_id":p.primitive_id,"owner_entity_id":p.owner_entity_id,"drawing_id":p.drawing_id,"source_revision":p.source_revision,"screen_start":p.start,"screen_end":p.end})).collect::<Vec<_>>()})
}
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn color(style: &DraftStyle) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (style.color[0] * 255.).round() as u8,
        (style.color[1] * 255.).round() as u8,
        (style.color[2] * 255.).round() as u8
    )
}
pub fn render_svg(display: &DisplaySnapshot, snapshot: &DraftSnapshot) -> Result<String, String> {
    render_svg_with_metadata(display, &snapshot_metadata(display, snapshot))
}
pub fn render_svg_with_metadata(
    display: &DisplaySnapshot,
    metadata: &Value,
) -> Result<String, String> {
    let camera = &display.camera;
    camera.validate().map_err(|e| e.to_string())?;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\"><metadata>{}</metadata><defs><clipPath id=\"canvas\"><rect width=\"{}\" height=\"{}\"/></clipPath></defs><rect width=\"100%\" height=\"100%\" fill=\"#111c28\"/><g clip-path=\"url(#canvas)\" fill=\"none\">",
        camera.viewport_width,
        camera.viewport_height,
        camera.viewport_width,
        camera.viewport_height,
        xml(&metadata.to_string()),
        camera.viewport_width,
        camera.viewport_height
    );
    for entity in &display.analytic_entities {
        let mut style = entity.style.clone();
        if display.selected_ids.contains(&entity.id) {
            style.color = [1., 0.74, 0.22, 1.];
            style.width_px += 1.;
        } else if display.hover_id == Some(entity.id) {
            style.color = [0.35, 1., 0.9, 1.];
            style.width_px += 0.5;
        }
        let mut data = String::new();
        match &entity.geometry {
            EntityGeometry::Line { start, end } => {
                let a = camera.world_to_screen(*start);
                let b = camera.world_to_screen(*end);
                data = format!("M {} {} L {} {}", a.x, a.y, b.x, b.y);
            }
            EntityGeometry::Circle { center, radius } => {
                let c = camera.world_to_screen(*center);
                let r = radius * camera.pixels_per_metre;
                data = format!(
                    "M {} {} A {r} {r} 0 1 0 {} {} A {r} {r} 0 1 0 {} {}",
                    c.x + r,
                    c.y,
                    c.x - r,
                    c.y,
                    c.x + r,
                    c.y
                );
            }
            EntityGeometry::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => {
                arc_path(
                    &mut data,
                    AnalyticArc {
                        center: *center,
                        radius: *radius,
                        start_angle: *start_angle,
                        sweep_angle: *sweep_angle,
                    },
                    camera,
                    true,
                );
            }
            EntityGeometry::Polyline { vertices, closed } => {
                let first = camera.world_to_screen(vertices[0].point);
                data = format!("M {} {}", first.x, first.y);
                let n = if *closed {
                    vertices.len()
                } else {
                    vertices.len() - 1
                };
                for i in 0..n {
                    let a = vertices[i];
                    let b = vertices[(i + 1) % vertices.len()];
                    bulge_path(&mut data, a.point, b.point, a.bulge, camera)?;
                }
                if *closed {
                    data.push_str(" Z");
                }
            }
        }
        svg.push_str(&format!("<path data-entity-id=\"{}\" data-layer-id=\"{}\" d=\"{data}\" stroke=\"{}\" stroke-width=\"{}\" stroke-opacity=\"{}\" stroke-linecap=\"round\"/>",entity.id,entity.layer_id,color(&style),style.width_px,style.color[3]));
        if svg.len() > 32 * 1024 * 1024 {
            return Err("SVG byte budget exceeded".into());
        }
    }
    svg.push_str("</g></svg>");
    Ok(svg)
}
// SVG already accepts arc endpoints. Preserve the stored chord endpoint instead
// of reconstructing it by subtracting a large bulge centre and radius.
fn bulge_path(
    data: &mut String,
    start: Point2,
    end: Point2,
    bulge: f64,
    camera: &Camera2D,
) -> Result<(), String> {
    let endpoint = camera.world_to_screen(end);
    if let Some(arc) = bulge_arc(start, end, bulge).map_err(|e| e.to_string())? {
        let radius = arc.radius * camera.pixels_per_metre;
        let large = usize::from(bulge.abs() > 1.);
        let direction = usize::from(bulge < 0.);
        data.push_str(&format!(
            " A {radius} {radius} 0 {large} {direction} {} {}",
            endpoint.x, endpoint.y
        ));
    } else {
        data.push_str(&format!(" L {} {}", endpoint.x, endpoint.y));
    }
    Ok(())
}
fn arc_path(data: &mut String, arc: AnalyticArc, camera: &Camera2D, move_first: bool) {
    let radius = arc.radius * camera.pixels_per_metre;
    let project = |fraction: f64| {
        let relative = arc.point_relative(camera.center, fraction);
        Point2::new(
            camera.viewport_width * 0.5 + relative.x * camera.pixels_per_metre,
            camera.viewport_height * 0.5 - relative.y * camera.pixels_per_metre,
        )
    };
    if move_first {
        let p = project(0.);
        data.push_str(&format!("M {} {}", p.x, p.y));
    }
    let parts = if arc.sweep_angle.abs() >= TAU - 1e-12 {
        2
    } else {
        1
    };
    for part in 1..=parts {
        let sweep = arc.sweep_angle / parts as f64;
        let p = project(part as f64 / parts as f64);
        let large = usize::from(sweep.abs() > PI);
        let direction = usize::from(sweep < 0.);
        data.push_str(&format!(
            " A {radius} {radius} 0 {large} {direction} {} {}",
            p.x, p.y
        ));
    }
}
pub fn render_png(display: &DisplaySnapshot) -> Result<Vec<u8>, String> {
    render_png_with_underlay(display, &[])
}
pub fn render_png_with_underlay(
    display: &DisplaySnapshot,
    underlay: &[(Point2, Point2, DraftStyle)],
) -> Result<Vec<u8>, String> {
    let scale = display.dpi_factor;
    let w = (display.camera.viewport_width * scale).ceil() as usize;
    let h = (display.camera.viewport_height * scale).ceil() as usize;
    if w == 0 || h == 0 || w > 8192 || h > 8192 || w.checked_mul(h).is_none_or(|n| n > 16_777_216) {
        return Err("PNG pixel budget exceeded".into());
    }
    let mut pixels = vec![0u8; w * h * 4];
    for p in pixels.chunks_exact_mut(4) {
        p.copy_from_slice(&[17, 28, 40, 255]);
    }
    let mut work = 0usize;
    let segments = underlay.iter().map(|(a, b, style)| (*a, *b, style)).chain(
        display
            .primitives
            .iter()
            .map(|p| (p.start, p.end, &p.appearance)),
    );
    for (start, end, appearance) in segments {
        let a = Point2::new(start.x * scale, start.y * scale);
        let b = Point2::new(end.x * scale, end.y * scale);
        let radius = (appearance.width_px * scale * 0.5).max(0.5);
        let n = ((b.x - a.x).abs().max((b.y - a.y).abs()) * 1.5)
            .ceil()
            .max(1.) as usize;
        let brush = ((radius + 1.).ceil() as usize * 2 + 1).pow(2);
        work = work
            .checked_add(n.checked_mul(brush).ok_or("PNG work budget overflow")?)
            .ok_or("PNG work budget overflow")?;
        if work > 100_000_000 {
            return Err("PNG work budget exceeded".into());
        }
        for step in 0..=n {
            let t = step as f64 / n as f64;
            let x = a.x + (b.x - a.x) * t;
            let y = a.y + (b.y - a.y) * t;
            let r = (radius + 1.).ceil() as i64;
            let cx = x.floor() as i64;
            let cy = y.floor() as i64;
            for py in (cy - r).max(0)..=(cy + r).min(h as i64 - 1) {
                for px in (cx - r).max(0)..=(cx + r).min(w as i64 - 1) {
                    let coverage = (radius + 0.5
                        - (px as f64 + 0.5 - x).hypot(py as f64 + 0.5 - y))
                    .clamp(0., 1.)
                        * appearance.color[3] as f64;
                    if coverage > 0. {
                        let offset = (py as usize * w + px as usize) * 4;
                        for c in 0..3 {
                            pixels[offset + c] = (pixels[offset + c] as f64 * (1. - coverage)
                                + appearance.color[c] as f64 * 255. * coverage)
                                .round() as u8;
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, w as u32, h as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .add_text_chunk("snapshot_token".into(), display.token.clone())
            .map_err(|e| e.to_string())?;
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&pixels)
            .map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn path_numbers(svg: &str) -> Vec<&str> {
        let path = svg
            .split(" d=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        path.split_whitespace().collect()
    }
    #[test]
    fn svg_huge_start_angle_preserves_small_sweep() {
        let mut doc = LabDocument::new();
        let drawing = doc.default_drawing_id();
        let arc = AnalyticArc {
            center: Point2::new(0., 0.),
            radius: 2.,
            start_angle: 1e12,
            sweep_angle: 1e-6,
        };
        doc.apply(CommandEnvelope {
            command_id: "arc".into(),
            expected_revision: 0,
            operations: vec![DraftOperation::Create {
                drawing_id: drawing,
                layer_id: LayerId::new(1),
                geometry: EntityGeometry::Arc {
                    center: arc.center,
                    radius: arc.radius,
                    start_angle: arc.start_angle,
                    sweep_angle: arc.sweep_angle,
                },
                style: Default::default(),
            }],
        })
        .unwrap();
        let snapshot = doc.snapshot(drawing).unwrap();
        let camera = Camera2D::new(1000., 700.);
        let display = build_display(&snapshot, &camera, &Default::default()).unwrap();
        let svg = render_svg(&display, &snapshot).unwrap();
        let path = path_numbers(&svg);
        let start = camera.world_to_screen(arc.point(0.));
        let end = camera.world_to_screen(arc.point(1.));
        assert!((path[1].parse::<f64>().unwrap() - start.x).abs() < 1e-10);
        assert!((path[9].parse::<f64>().unwrap() - end.x).abs() < 1e-10);
        assert!((path[10].parse::<f64>().unwrap() - end.y).abs() < 1e-10);
        assert_ne!(path[1], path[9]);
        assert_ne!(path[2], path[10]);
    }
    #[test]
    fn svg_arc_endpoint_preserves_cancellation_at_the_domain_boundary() {
        let mut doc = LabDocument::new();
        let drawing = doc.default_drawing_id();
        let arc = AnalyticArc {
            center: Point2::new(-1e12, 0.),
            radius: 1e12,
            start_angle: 0.,
            sweep_angle: 1e-10,
        };
        doc.apply(CommandEnvelope {
            command_id: "cancel-arc".into(),
            expected_revision: 0,
            operations: vec![DraftOperation::Create {
                drawing_id: drawing,
                layer_id: LayerId::new(1),
                geometry: EntityGeometry::Arc {
                    center: arc.center,
                    radius: arc.radius,
                    start_angle: arc.start_angle,
                    sweep_angle: arc.sweep_angle,
                },
                style: Default::default(),
            }],
        })
        .unwrap();
        let snapshot = doc.snapshot(drawing).unwrap();
        let mut camera = Camera2D::new(1000., 700.);
        camera.center = Point2::new(-5e-9, 100.);
        camera.pixels_per_metre = 1e12;
        let display = build_display(&snapshot, &camera, &Default::default()).unwrap();
        let svg = render_svg(&display, &snapshot).unwrap();
        let path = path_numbers(&svg);
        assert!((path[9].parse::<f64>().unwrap() - 500.).abs() < 1e-6);
        // The stored binary sweep times 1e12 is not exactly decimal 100;
        // the vertical oracle uses the physical-pixel accuracy contract.
        let screen_y = path[10].parse::<f64>().unwrap();
        assert!((screen_y - 350.).abs() < 0.5, "endpoint y={screen_y}");
    }
    #[test]
    fn svg_nearly_flat_bulge_preserves_authoritative_endpoints() {
        for bulge in [1e-12, -1e-12] {
            for scale in [1e6, 1e8] {
                let mut doc = LabDocument::new();
                let drawing = doc.default_drawing_id();
                doc.apply(CommandEnvelope {
                    command_id: "polyline".into(),
                    expected_revision: 0,
                    operations: vec![DraftOperation::Create {
                        drawing_id: drawing,
                        layer_id: LayerId::new(1),
                        geometry: EntityGeometry::Polyline {
                            vertices: vec![
                                PolylineVertex {
                                    point: Point2::new(0., 0.),
                                    bulge,
                                },
                                PolylineVertex {
                                    point: Point2::new(1., 0.),
                                    bulge: 0.,
                                },
                            ],
                            closed: false,
                        },
                        style: Default::default(),
                    }],
                })
                .unwrap();
                let snapshot = doc.snapshot(drawing).unwrap();
                let mut camera = Camera2D::new(1000., 700.);
                camera.center = Point2::new(0.5, 0.);
                camera.pixels_per_metre = scale;
                let display = build_display(&snapshot, &camera, &Default::default()).unwrap();
                let svg = render_svg(&display, &snapshot).unwrap();
                let path = path_numbers(&svg);
                assert_eq!(path[0], "M");
                assert_eq!(path[3], "A");
                assert_eq!(path[10].parse::<f64>().unwrap(), 350.);
                assert_eq!(path[9].parse::<f64>().unwrap(), 500. + 0.5 * scale);
                assert_eq!(path[8].parse::<usize>().unwrap(), usize::from(bulge < 0.));
                assert_eq!(
                    snapshot.entity(DraftEntityId::new(1)).unwrap().geometry,
                    display.analytic_entities[0].geometry
                );
            }
        }
    }
    #[test]
    fn svg_closing_bulge_retains_signed_arc_and_endpoint() {
        for bulge in [0.5, -0.5, 2., -2.] {
            let mut doc = LabDocument::new();
            let drawing = doc.default_drawing_id();
            doc.apply(CommandEnvelope {
                command_id: "closed-polyline".into(),
                expected_revision: 0,
                operations: vec![DraftOperation::Create {
                    drawing_id: drawing,
                    layer_id: LayerId::new(1),
                    geometry: EntityGeometry::Polyline {
                        vertices: vec![
                            PolylineVertex {
                                point: Point2::new(0., 0.),
                                bulge: 0.,
                            },
                            PolylineVertex {
                                point: Point2::new(1., 0.),
                                bulge: 0.,
                            },
                            PolylineVertex {
                                point: Point2::new(1., 1.),
                                bulge,
                            },
                        ],
                        closed: true,
                    },
                    style: Default::default(),
                }],
            })
            .unwrap();
            let snapshot = doc.snapshot(drawing).unwrap();
            let camera = Camera2D::new(1000., 700.);
            let display = build_display(&snapshot, &camera, &Default::default()).unwrap();
            let svg = render_svg(&display, &snapshot).unwrap();
            let path = path_numbers(&svg);
            let arc = path.iter().position(|part| *part == "A").unwrap();
            assert_eq!(
                path[arc + 4].parse::<usize>().unwrap(),
                usize::from(bulge.abs() > 1.)
            );
            assert_eq!(
                path[arc + 5].parse::<usize>().unwrap(),
                usize::from(bulge < 0.)
            );
            assert_eq!(path[arc + 6].parse::<f64>().unwrap(), 500.);
            assert_eq!(path[arc + 7].parse::<f64>().unwrap(), 350.);
            assert_eq!(path.last(), Some(&"Z"));
        }
    }
    #[test]
    fn export_consistency_and_analytic_svg() {
        let mut doc = LabDocument::new();
        let id = doc.default_drawing_id();
        doc.apply(CommandEnvelope {
            command_id: "c".into(),
            expected_revision: 0,
            operations: vec![DraftOperation::Create {
                drawing_id: id,
                layer_id: LayerId::new(1),
                geometry: EntityGeometry::Circle {
                    center: Point2::new(0., 0.),
                    radius: 1.,
                },
                style: Default::default(),
            }],
        })
        .unwrap();
        let s = doc.snapshot(id).unwrap();
        let d = build_display(&s, &Camera2D::new(300., 200.), &Default::default()).unwrap();
        let svg = render_svg(&d, &s).unwrap();
        assert!(svg.contains(" A "));
        assert!(svg.contains(&d.token));
        assert_eq!(id_map(&d, &s)["metadata"]["snapshot_token"], d.token);
        assert!(
            d.primitives
                .iter()
                .all(|p| p.owner_entity_id == DraftEntityId::new(1))
        );
        assert!(render_png(&d).unwrap().starts_with(b"\x89PNG"));
        let mut c = d.camera.clone();
        c.pan_pixels(1., 2.).unwrap();
        assert!(!d.is_current(&s, &c, &Default::default()).unwrap());
    }
}
