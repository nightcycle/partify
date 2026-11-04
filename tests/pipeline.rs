use glam::{Mat3, Vec2, Vec3};
use image::{Rgba, RgbaImage};
use partify::{
    config::Tuning,
    export,
    geometry::{Shape, Triangle},
    import::{self, Assets, Source},
    pipeline,
};
use rbx_dom_weak::{
    InstanceBuilder, WeakDom,
    types::{CFrame, Color3, Matrix3, Variant, Vector3},
};
use std::{fs, sync::atomic::AtomicBool};

fn tuning(budget: usize) -> Tuning {
    Tuning {
        budget,
        sample_limit: 384,
        image_resolution: 24,
        fit_samples: 32,
        probes: 24,
        split_candidates: 1,
        refinement_passes: 0,
        ..Default::default()
    }
}
fn build(source: &Source, t: &Tuning) -> pipeline::Build {
    pipeline::run(source, t, &AtomicBool::new(false), |_| {}).unwrap()
}
fn image() -> Source {
    let mut image = RgbaImage::new(24, 16);
    for (x, _, p) in image.enumerate_pixels_mut() {
        *p = if x < 12 {
            Rgba([220, 40, 30, 255])
        } else {
            Rgba([30, 80, 220, 255])
        }
    }
    Source::Image(image)
}
fn mesh(shape: Shape) -> Source {
    Source::Mesh {
        triangles: shape
            .triangles()
            .into_iter()
            .map(|p| Triangle {
                p,
                colors: [Vec3::new(0.7, 0.2, 0.1); 3],
                uv: [Vec2::ZERO; 3],
                texture: None,
            })
            .collect(),
        textures: vec![],
    }
}

#[test]
fn hard_budgets_and_baked_tuning_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    for budget in [1, 2, 7] {
        let t = tuning(budget);
        let b = build(&image(), &t);
        assert!(b.parts.len() <= budget);
        assert!(b.metrics.coverage > 0.95);
        assert!(b.metrics.sampled_overlap < 0.01);
        for ext in ["rbxm", "rbxmx"] {
            let p = dir.path().join(format!("result.{ext}"));
            export::save(&b, &p, "fixture.png").unwrap();
            assert_eq!(export::read_tuning(&p).unwrap(), t);
            let dom = import::read_dom(&fs::read(&p).unwrap()).unwrap();
            let model = dom.get_by_ref(dom.root().children()[0]).unwrap();
            assert_eq!(model.children().len(), b.parts.len());
            for &id in model.children() {
                let p = dom.get_by_ref(id).unwrap();
                assert!(matches!(
                    p.class.as_str(),
                    "Part" | "WedgePart" | "CornerWedgePart" | "MeshPart"
                ));
                assert!(matches!(
                    p.properties.get(&"Anchored".into()),
                    Some(Variant::Bool(true))
                ))
            }
        }
    }
}
#[test]
fn color_boundaries_improve_with_budget() {
    let one = build(&image(), &tuning(1));
    let two = build(&image(), &tuning(2));
    assert!(two.metrics.mean_color_error < one.metrics.mean_color_error * 0.25);
    assert_eq!(two.parts.len(), 2);
    assert!(!two.connections.is_empty());
}
#[test]
fn diagonal_color_boundary_uses_complementary_wedges() {
    let dir = tempfile::tempdir().unwrap();
    for (width, height, flip) in [
        (24, 24, false),
        (24, 24, true),
        (24, 16, false),
        (16, 24, true),
    ] {
        let pixels = RgbaImage::from_fn(width, height, |x, y| {
            let x = if flip { width - 1 - x } else { x };
            if (2 * x + 1) * height <= (2 * y + 1) * width {
                Rgba([220, 40, 30, 255])
            } else {
                Rgba([30, 80, 220, 255])
            }
        });
        let t = Tuning {
            refinement_passes: 2,
            ..tuning(2)
        };
        let b = build(&Source::Image(pixels), &t);
        assert_eq!(b.parts.len(), 2);
        assert!(
            b.parts.iter().all(|p| p.shape == Shape::Wedge),
            "{:?}",
            b.parts
        );
        assert!(b.metrics.mean_color_error < 0.05, "{:?}", b.metrics);
        assert!(b.metrics.coverage > 0.98, "{:?}", b.metrics);
        assert!(b.metrics.sampled_overlap < 1e-6, "{:?}", b.metrics);
        assert!(b.volumes.iter().all(|v| v.footprint.len() == 3));
        // Probe between source samples and through the thickness as well.
        for y in 0..39 {
            for x in 0..41 {
                for z in [-0.2, 0., 0.2] {
                    let point = Vec3::new(
                        (x as f32 / 40. - 0.5) * t.size,
                        (y as f32 / 38. - 0.5) * t.size,
                        z,
                    );
                    assert!(
                        b.parts
                            .iter()
                            .filter(|p| p.contains_interior(point))
                            .count()
                            <= 1
                    );
                }
            }
        }
        assert!(
            b.connections
                .iter()
                .any(|c| c.touching && c.normal.x.abs() > 0.1 && c.normal.y.abs() > 0.1),
            "flip={flip}: {:?}; {:?}; {:?}",
            b.connections,
            b.volumes,
            b.parts
        );
        for extension in ["rbxm", "rbxmx"] {
            let path = dir.path().join(format!("diagonal.{extension}"));
            export::save(&b, &path, "diagonal.png").unwrap();
            let dom = import::read_dom(&fs::read(path).unwrap()).unwrap();
            let model = dom.get_by_ref(dom.root().children()[0]).unwrap();
            for (index, &id) in model.children().iter().enumerate() {
                let part = dom.get_by_ref(id).unwrap();
                assert_eq!(part.class.as_str(), "WedgePart");
                let Variant::Attributes(attrs) = &part.properties[&"Attributes".into()] else {
                    panic!()
                };
                let bytes: &[u8] = match attrs.get("PartifyVolumeFootprint").unwrap() {
                    Variant::String(value) => value.as_bytes(),
                    Variant::BinaryString(value) => value.as_ref(),
                    _ => panic!("expected footprint JSON"),
                };
                let polygon: Vec<Vec2> = serde_json::from_slice(bytes).unwrap();
                assert_eq!(polygon, b.volumes[index].footprint);
            }
        }
    }
}
#[test]
fn uniform_image_is_one_large_brick() {
    let b = build(
        &Source::Image(RgbaImage::from_pixel(24, 24, Rgba([80, 130, 220, 255]))),
        &tuning(12),
    );
    assert_eq!(b.parts.len(), 1);
    assert_eq!(b.parts[0].shape, Shape::Block);
}
#[test]
fn transparency_and_empty_images() {
    let mut pixels = RgbaImage::new(24, 24);
    for y in 0..24 {
        for x in 0..=y {
            pixels.put_pixel(x, y, Rgba([20, 160, 220, 255]));
        }
    }
    let b = build(&Source::Image(pixels), &tuning(1));
    assert_eq!(b.parts.len(), 1);
    assert_ne!(b.parts[0].shape, Shape::Block);
    assert!(
        pipeline::run(
            &Source::Image(RgbaImage::new(8, 8)),
            &tuning(1),
            &AtomicBool::new(false),
            |_| {}
        )
        .is_err()
    );
}
#[test]
fn primitive_geometry_and_rotations() {
    for shape in Shape::ALL {
        let tris = shape.triangles();
        assert!(!tris.is_empty());
        for tri in tris {
            for p in tri {
                assert!(shape.contains(p), "{shape:?}: {p:?}")
            }
        }
    }
    for r in partify::geometry::rotations() {
        assert!((r.determinant() - 1.).abs() < 1e-5)
    }
    assert_eq!(partify::geometry::rotations().len(), 24);
}
#[test]
fn cube_sphere_and_wedge_are_not_voxelized() {
    for shape in [Shape::Block, Shape::Ball, Shape::Wedge] {
        let b = build(&mesh(shape), &tuning(1));
        assert_eq!(b.parts.len(), 1);
        assert!(b.metrics.coverage > 0.8, "{shape:?}: {:?}", b.metrics);
        if shape == Shape::Block {
            assert_eq!(b.parts[0].shape, shape)
        }
    }
}
#[test]
fn supplied_mesh_templates_export_and_reimport_offline() {
    let dir = tempfile::tempdir().unwrap();
    let assets = Assets::new(dir.path().join("cache"), true, None).unwrap();
    for (shape, file) in [
        (Shape::CornerTriangle, "corner-triangle.rbxmx"),
        (
            Shape::InvertedCornerTriangle,
            "inverted-corner-triangle.rbxmx",
        ),
        (Shape::Quad, "quad.rbxmx"),
    ] {
        let source = import::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join(file),
            &assets,
        )
        .unwrap();
        let mut b = build(&source, &tuning(1));
        b.parts[0].shape = shape;
        let p = dir.path().join("mesh.rbxm");
        export::save(&b, &p, file).unwrap();
        let dom = import::read_dom(&fs::read(&p).unwrap()).unwrap();
        let m = dom.get_by_ref(dom.root().children()[0]).unwrap();
        let part = dom.get_by_ref(m.children()[0]).unwrap();
        assert_eq!(part.class.as_str(), "MeshPart");
        assert!(
            part.properties.contains_key(&"MeshId".into())
                || part.properties.contains_key(&"MeshContent".into())
        );
        import::load(&p, &assets).unwrap();
    }
}
#[test]
fn rbxm_and_rbxmx_preserve_world_transform_and_color() {
    let dir = tempfile::tempdir().unwrap();
    let assets = Assets::new(dir.path().join("cache"), true, None).unwrap();
    let mut dom = WeakDom::new(InstanceBuilder::new("DataModel"));
    dom.insert(
        dom.root_ref(),
        InstanceBuilder::new("Part")
            .with_property("Size", Vector3::new(2., 4., 6.))
            .with_property("Color", Color3::new(1., 0., 0.))
            .with_property(
                "CFrame",
                CFrame::new(
                    Vector3::new(10., 20., 30.),
                    Matrix3::new(
                        Vector3::new(0., -1., 0.),
                        Vector3::new(1., 0., 0.),
                        Vector3::new(0., 0., 1.),
                    ),
                ),
            ),
    );
    for ext in ["rbxm", "rbxmx"] {
        let mut bytes = Vec::new();
        if ext == "rbxm" {
            rbx_binary::to_writer(&mut bytes, &dom, dom.root().children()).unwrap()
        } else {
            rbx_xml::to_writer(&mut bytes, &dom, dom.root().children(), Default::default()).unwrap()
        }
        let p = dir.path().join(format!("input.{ext}"));
        fs::write(&p, bytes).unwrap();
        let Source::Mesh { triangles, .. } = import::load(&p, &assets).unwrap() else {
            panic!()
        };
        let mut bounds = partify::geometry::Bounds::empty();
        for t in triangles {
            assert!(t.colors.iter().all(|c| *c == Vec3::X));
            for p in t.p {
                bounds.add(p)
            }
        }
        assert!(bounds.center().distance(Vec3::new(10., 20., 30.)) < 1e-5);
        assert!(bounds.size().distance(Vec3::new(4., 2., 6.)) < 1e-5);
    }
}
#[test]
fn png_jpg_and_obj_materials() {
    let dir = tempfile::tempdir().unwrap();
    let assets = Assets::new(dir.path().join("cache"), true, None).unwrap();
    for ext in ["png", "jpg"] {
        let path = dir.path().join(format!("image.{ext}"));
        image::RgbImage::from_pixel(12, 8, image::Rgb([200, 80, 10]))
            .save(&path)
            .unwrap();
        assert!(matches!(
            import::load(&path, &assets).unwrap(),
            Source::Image(_)
        ));
    }
    fs::write(dir.path().join("test.mtl"), "newmtl red\nKd 1 0 0\n").unwrap();
    let path = dir.path().join("test.obj");
    fs::write(
        &path,
        "mtllib test.mtl\nv 0 0 0\nv 2 0 0\nv 0 2 0\nusemtl red\nf 1 2 3\n",
    )
    .unwrap();
    let Source::Mesh { triangles, .. } = import::load(&path, &assets).unwrap() else {
        panic!()
    };
    assert_eq!(triangles.len(), 1);
    assert!(triangles[0].colors[0].distance(Vec3::X) < 0.01);
}
#[test]
fn cancelled_and_invalid_inputs_fail() {
    assert!(pipeline::run(&image(), &tuning(1), &AtomicBool::new(true), |_| {}).is_err());
    let mut t = tuning(0);
    assert!(t.validate().is_err());
    t.budget = 1;
    t.spill_weight = f32::NAN;
    assert!(t.validate().is_err());
    assert!(import::read_dom(b"not a model").is_err());
}
#[test]
fn deterministic_geometry() {
    let a = build(&image(), &tuning(4));
    let b = build(&image(), &tuning(4));
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

#[test]
fn shared_faces_are_contacts_not_overlaps() {
    let b = build(&image(), &tuning(2));
    let c = &b.connections[0];
    assert!(b.parts[c.a].contains(c.center));
    assert!(b.parts[c.b].contains(c.center));
    assert!(!b.parts[c.a].contains_interior(c.center));
    assert!(!b.parts[c.b].contains_interior(c.center));
    assert_eq!(b.metrics.sampled_overlap, 0.);
}

#[test]
fn exact_corner_shapes_survive_frame_selection() {
    for shape in [
        Shape::CornerTriangle,
        Shape::InvertedCornerTriangle,
        Shape::CornerWedge,
    ] {
        let b = build(&mesh(shape), &tuning(1));
        assert_eq!(b.parts[0].shape, shape, "{:?}", b.metrics);
        assert!(b.metrics.coverage > 0.99);
    }
    let Source::Mesh {
        mut triangles,
        textures,
    } = mesh(Shape::Block)
    else {
        panic!()
    };
    let rotation = Mat3::from_rotation_x(0.5) * Mat3::from_rotation_y(0.7);
    for t in &mut triangles {
        for p in &mut t.p {
            *p = rotation * *p
        }
    }
    let b = build(
        &Source::Mesh {
            triangles,
            textures,
        },
        &tuning(1),
    );
    assert!(b.parts[0].size.max_element() / b.parts[0].size.min_element() < 1.01);
    assert!(b.metrics.coverage > 0.99);
}

#[test]
fn ascii_and_binary_fbx() {
    let dir = tempfile::tempdir().unwrap();
    let assets = Assets::new(dir.path().join("cache"), true, None).unwrap();
    for file in [
        "blender_279_default_6100_ascii.fbx",
        "blender_272_cube_7400_binary.fbx",
    ] {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(file);
        let source = import::load(&p, &assets).unwrap();
        let b = build(&source, &tuning(6));
        assert!(!b.parts.is_empty() && b.parts.len() <= 6);
        assert!(b.metrics.coverage > 0.85, "{file}: {:?}", b.metrics);
    }
}

#[test]
fn textured_obj_and_disconnected_regions() {
    let dir = tempfile::tempdir().unwrap();
    let assets = Assets::new(dir.path().join("cache"), true, None).unwrap();
    image::RgbImage::from_pixel(4, 4, image::Rgb([20, 180, 80]))
        .save(dir.path().join("texture.png"))
        .unwrap();
    fs::write(
        dir.path().join("test.mtl"),
        "newmtl painted\nKd 1 1 1\nmap_Kd texture.png\n",
    )
    .unwrap();
    let path = dir.path().join("test.obj");
    fs::write(&path,"mtllib test.mtl\nv 0 0 0\nv 2 0 0\nv 0 2 0\nvt 0 0\nvt 1 0\nvt 0 1\nusemtl painted\nf 1/1 2/2 3/3\n").unwrap();
    let source = import::load(&path, &assets).unwrap();
    let b = build(&source, &tuning(2));
    assert!(b.parts.iter().all(|p| p.color.y > 0.6 && p.color.x < 0.2));
    assert!(!b.warnings.is_empty());
    let mut image = RgbaImage::new(24, 24);
    for y in 0..24 {
        for x in (0..8).chain(16..24) {
            image.put_pixel(x, y, Rgba([200, 20, 30, 255]));
        }
    }
    let b = build(&Source::Image(image), &tuning(4));
    assert!(b.parts.len() >= 2);
    assert!(b.parts.iter().all(|p| !p.contains(Vec3::ZERO)));
}
#[test]
fn export_rotations_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = build(&image(), &tuning(1));
    b.parts[0].rotation = Mat3::from_rotation_y(0.73);
    let path = dir.path().join("rotation.rbxm");
    export::save(&b, &path, "image.png").unwrap();
    let assets = Assets::new(dir.path().join("cache"), true, None).unwrap();
    let Source::Mesh { triangles, .. } = import::load(&path, &assets).unwrap() else {
        panic!()
    };
    let expected = b.parts[0].triangles();
    for tri in triangles {
        for p in tri.p {
            assert!(expected.iter().flatten().any(|q| q.distance(p) < 1e-4))
        }
    }
}
