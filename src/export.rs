use crate::{config::Tuning, geometry::Shape, import::read_dom, pipeline::Build};
use anyhow::{Context, Result, ensure};
use glam::{Mat3, Vec3};
use rbx_dom_weak::{
    InstanceBuilder, WeakDom,
    types::{Attributes, CFrame, Color3, Enum, Matrix3, Variant, Vector3},
};
use std::{fs, io::Write, path::Path};

fn v3(v: Vec3) -> Vector3 {
    Vector3::new(v.x, v.y, v.z)
}
fn frame(center: Vec3, rotation: Mat3) -> CFrame {
    let r = rotation.transpose();
    CFrame::new(
        v3(center),
        Matrix3::new(v3(r.x_axis), v3(r.y_axis), v3(r.z_axis)),
    )
}
fn pascal(s: &str) -> String {
    s.split('_')
        .map(|p| {
            let mut c = p.chars();
            c.next().unwrap().to_uppercase().collect::<String>() + c.as_str()
        })
        .collect()
}
fn templates(shape: Shape) -> Option<&'static [u8]> {
    match shape {
        Shape::CornerTriangle => Some(include_bytes!("../assets/corner-triangle.rbxmx")),
        Shape::InvertedCornerTriangle => {
            Some(include_bytes!("../assets/inverted-corner-triangle.rbxmx"))
        }
        Shape::Quad => Some(include_bytes!("../assets/quad.rbxmx")),
        _ => None,
    }
}
pub fn model(build: &Build, source: &str) -> Result<WeakDom> {
    ensure!(
        build.parts.len() <= build.tuning.budget,
        "part budget exceeded"
    );
    let mut attrs = Attributes::new();
    attrs.insert(
        "PartifyVersion".into(),
        env!("CARGO_PKG_VERSION").to_string().into(),
    );
    attrs.insert("PartifySource".into(), source.to_string().into());
    attrs.insert(
        "PartifyTuning".into(),
        serde_json::to_string(&build.tuning)?.into(),
    );
    attrs.insert(
        "PartifyMetrics".into(),
        serde_json::to_string(&build.metrics)?.into(),
    );
    attrs.insert(
        "PartifyWarnings".into(),
        serde_json::to_string(&build.warnings)?.into(),
    );
    attrs.insert(
        "PartifyVolumeFrame".into(),
        frame(Vec3::ZERO, build.frame).into(),
    );
    for (key, value) in serde_json::to_value(&build.tuning)?.as_object().unwrap() {
        let value = match value {
            serde_json::Value::Bool(b) => Variant::Bool(*b),
            serde_json::Value::Number(n) => Variant::Float64(n.as_f64().unwrap()),
            v => Variant::String(v.to_string()),
        };
        attrs.insert(format!("Partify{}", pascal(key)), value);
    }
    let mut dom = WeakDom::new(InstanceBuilder::new("DataModel"));
    let root = dom.insert(
        dom.root_ref(),
        InstanceBuilder::new("Model")
            .with_name("Partify")
            .with_property("Attributes", attrs),
    );
    for (i, part) in build.parts.iter().enumerate() {
        ensure!(
            part.center.is_finite()
                && part.size.is_finite()
                && part.size.min_element() > 0.
                && part.rotation.is_finite(),
            "invalid fitted part"
        );
        let mut attrs = Attributes::new();
        attrs.insert("PartifyVolume".into(), (i as f64).into());
        attrs.insert("PartifyShape".into(), format!("{:?}", part.shape).into());
        attrs.insert("PartifyLoss".into(), (part.loss as f64).into());
        let links: Vec<_> = build
            .connections
            .iter()
            .filter(|c| c.a == i || c.b == i)
            .collect();
        attrs.insert(
            "PartifyConnections".into(),
            serde_json::to_string(&links)?.into(),
        );
        attrs.insert(
            "PartifyVolumeMin".into(),
            v3(build.volumes[i].bounds.min).into(),
        );
        attrs.insert(
            "PartifyVolumeMax".into(),
            v3(build.volumes[i].bounds.max).into(),
        );
        if !build.volumes[i].footprint.is_empty() {
            attrs.insert(
                "PartifyVolumeFootprint".into(),
                serde_json::to_string(&build.volumes[i].footprint)?.into(),
            );
        }
        let mut instance = InstanceBuilder::new(part.shape.class())
            .with_name(format!("{:04}_{:?}", i + 1, part.shape))
            .with_property("Size", v3(part.size))
            .with_property("CFrame", frame(part.center, part.rotation))
            .with_property(
                "Color",
                Color3::new(part.color.x, part.color.y, part.color.z),
            )
            .with_property("Anchored", true)
            .with_property("CanCollide", part.shape != Shape::Quad)
            .with_property("Material", Enum::from_u32(256))
            .with_property("Attributes", attrs);
        if let Some(bytes) = templates(part.shape) {
            let template = read_dom(bytes)?;
            let mesh = template
                .get_by_ref(template.root().children()[0])
                .context("template has no MeshPart")?;
            // Preserve the supplied asset/collision payload, never scripts or unrelated metadata.
            for name in [
                "MeshId",
                "MeshContent",
                "InitialSize",
                "MeshSize",
                "PhysicalConfigData",
                "PhysicsData",
                "RenderFidelity",
            ] {
                if let Some(value) = mesh.properties.get(&name.into()) {
                    instance = instance.with_property(name, value.clone());
                }
            }
            instance = instance.with_property("DoubleSided", true);
        } else if part.shape.class() == "Part" {
            instance = instance.with_property(
                "Shape",
                Enum::from_u32(match part.shape {
                    Shape::Ball => 0,
                    Shape::Cylinder => 2,
                    _ => 1,
                }),
            );
        }
        for surface in [
            "TopSurface",
            "BottomSurface",
            "LeftSurface",
            "RightSurface",
            "FrontSurface",
            "BackSurface",
        ] {
            instance = instance.with_property(
                surface,
                Enum::from_u32(if build.tuning.studs && surface == "TopSurface" {
                    3
                } else {
                    0
                }),
            );
        }
        dom.insert(root, instance);
    }
    Ok(dom)
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.flush()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
pub fn save(build: &Build, path: &Path, source: &str) -> Result<()> {
    let dom = model(build, source)?;
    let mut bytes = Vec::new();
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rbxm" => rbx_binary::to_writer(&mut bytes, &dom, dom.root().children())?,
        "rbxmx" => rbx_xml::to_writer(
            &mut bytes,
            &dom,
            dom.root().children(),
            rbx_xml::EncodeOptions::new(),
        )?,
        _ => anyhow::bail!("output must end in .rbxm or .rbxmx"),
    }
    atomic_write(path, &bytes).with_context(|| format!("writing {}", path.display()))
}
pub fn read_tuning(path: &Path) -> Result<Tuning> {
    let bytes = fs::read(path)?;
    if path.extension().is_some_and(|e| e == "json") {
        let t: Tuning = serde_json::from_slice(&bytes)?;
        t.validate()?;
        return Ok(t);
    }
    let dom = read_dom(&bytes)?;
    for &r in dom.root().children() {
        let i = dom.get_by_ref(r).unwrap();
        if let Some(Variant::Attributes(a)) = i.properties.get(&"Attributes".into()) {
            let json = match a.get("PartifyTuning") {
                Some(Variant::String(s)) => Some(s.as_bytes()),
                Some(Variant::BinaryString(s)) => Some(s.as_ref()),
                _ => None,
            };
            if let Some(s) = json {
                let t: Tuning = serde_json::from_slice(s)?;
                t.validate()?;
                return Ok(t);
            }
        }
    }
    anyhow::bail!("no PartifyTuning attribute found in {}", path.display())
}
