use crate::geometry::{Bounds, Shape, Triangle};
use anyhow::{Context, Result, bail, ensure};
use glam::{Mat3, Mat4, Vec2, Vec3};
use image::RgbaImage;
use rbx_dom_weak::{WeakDom, types::Variant};
use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone)]
pub enum Source {
    Image(RgbaImage),
    Mesh {
        triangles: Vec<Triangle>,
        textures: Vec<RgbaImage>,
    },
}

pub struct Assets {
    pub cache: PathBuf,
    pub offline: bool,
    pub mappings: HashMap<String, PathBuf>,
}
impl Assets {
    pub fn new(cache: PathBuf, offline: bool, map: Option<&Path>) -> Result<Self> {
        let mut mappings: HashMap<String, PathBuf> = if let Some(p) = map {
            serde_json::from_slice(&fs::read(p)?)?
        } else {
            HashMap::new()
        };
        if let Some(p) = map {
            for value in mappings.values_mut() {
                *value = p.parent().unwrap_or(Path::new(".")).join(&*value)
            }
        }
        Ok(Self {
            cache,
            offline,
            mappings,
        })
    }
    pub fn load(&self, uri: &str) -> Result<Vec<u8>> {
        if let Some(p) = self.mappings.get(uri) {
            return fs::read(p).with_context(|| format!("asset mapping {}", p.display()));
        }
        let id = asset_id(uri)
            .with_context(|| format!("unsupported asset URI {uri}; use --asset-map"))?;
        let p = self.cache.join(format!("{id}.bin"));
        if p.is_file() {
            return Ok(fs::read(p)?);
        }
        ensure!(
            !self.offline,
            "asset {id} missing from cache {}; --offline prevents downloading it. Run once without --offline to populate the cache, or provide a local --asset-map",
            self.cache.display()
        );
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(45))
            .build()?;
        let response=client.get(format!("https://assetdelivery.roblox.com/v1/asset/?id={id}")).send()?.error_for_status()
            .with_context(||format!("Roblox asset {id} unavailable; private/restricted assets require a local --asset-map"))?;
        let mut bytes = Vec::new();
        response
            .take(128 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 128 * 1024 * 1024, "asset exceeds 128 MiB");
        fs::create_dir_all(&self.cache)?;
        fs::write(p, &bytes)?;
        Ok(bytes)
    }
}
pub fn asset_id(s: &str) -> Option<u64> {
    if let Some(s) = s.strip_prefix("rbxassetid://") {
        return s.parse().ok();
    }
    if let Ok(id) = s.parse() {
        return Some(id);
    }
    let url = reqwest::Url::parse(s).ok()?;
    if !matches!(
        url.host_str()?,
        "roblox.com" | "www.roblox.com" | "assetdelivery.roblox.com"
    ) {
        return None;
    }
    url.query_pairs()
        .find_map(|(k, v)| if k == "id" { v.parse().ok() } else { None })
}

pub fn load(path: &Path, assets: &Assets) -> Result<Source> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" => {
            let reader = image::ImageReader::open(path)?.with_guessed_format()?;
            Ok(Source::Image(reader.decode()?.to_rgba8()))
        }
        "fbx" | "obj" => exchange(path),
        "rbxm" | "rbxmx" => roblox(path, assets),
        _ => bail!(
            "unsupported extension {ext:?}; expected rbxm, rbxmx, fbx, obj, png, jpg, or jpeg"
        ),
    }
    .with_context(|| format!("importing {}", path.display()))
}
fn checked_mesh(mut triangles: Vec<Triangle>, textures: Vec<RgbaImage>) -> Result<Source> {
    ensure!(
        triangles.len() <= 2_000_000,
        "model exceeds two million triangles"
    );
    ensure!(
        triangles.iter().all(|t| t.p.iter().all(|p| p.is_finite())
            && t.colors.iter().all(|c| c.is_finite())
            && t.uv.iter().all(|v| v.is_finite())),
        "non-finite mesh geometry or color"
    );
    triangles.retain(|t| t.area() > 1e-12);
    ensure!(
        !triangles.is_empty(),
        "no visible, non-degenerate geometry found"
    );
    Ok(Source::Mesh {
        triangles,
        textures,
    })
}
fn v3(v: ufbx::Vec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}
fn matrix(m: &ufbx::Matrix) -> Mat4 {
    Mat4::from_cols_array(&[
        m.m00 as f32,
        m.m10 as f32,
        m.m20 as f32,
        0.,
        m.m01 as f32,
        m.m11 as f32,
        m.m21 as f32,
        0.,
        m.m02 as f32,
        m.m12 as f32,
        m.m22 as f32,
        0.,
        m.m03 as f32,
        m.m13 as f32,
        m.m23 as f32,
        1.,
    ])
}
fn exchange(path: &Path) -> Result<Source> {
    let scene = ufbx::load_file(
        path.to_str().context("non-UTF8 input path")?,
        ufbx::LoadOpts {
            load_external_files: true,
            ignore_missing_external_files: true,
            generate_missing_normals: true,
            target_axes: ufbx::CoordinateAxes {
                right: ufbx::CoordinateAxis::PositiveX,
                up: ufbx::CoordinateAxis::PositiveY,
                front: ufbx::CoordinateAxis::PositiveZ,
            },
            ..Default::default()
        },
    )
    .map_err(|e| anyhow::anyhow!("OBJ/FBX: {e:?}"))?;
    let mut triangles = Vec::new();
    let mut textures = Vec::new();
    let mut tex_ids = HashMap::new();
    for node in &scene.nodes {
        let Some(mesh) = &node.mesh else { continue };
        let transform = matrix(&node.geometry_to_world);
        let mut indices = Vec::new();
        for (face_index, face) in mesh.faces.iter().enumerate() {
            let material = mesh
                .face_material
                .get(face_index)
                .and_then(|&i| node.materials.get(i as usize));
            let map = material.map(|m| {
                if m.pbr.base_color.has_value || m.pbr.base_color.texture.is_some() {
                    &m.pbr.base_color
                } else {
                    &m.fbx.diffuse_color
                }
            });
            let color = map.filter(|m| m.has_value).map_or(Vec3::splat(0.65), |m| {
                Vec3::new(
                    m.value_vec4.x as f32,
                    m.value_vec4.y as f32,
                    m.value_vec4.z as f32,
                )
            });
            let texture = if let Some(tex) = map.and_then(|m| m.texture.as_ref()) {
                let key = tex.element.element_id;
                if let Some(&id) = tex_ids.get(&key) {
                    Some(id)
                } else {
                    let bytes = if !tex.content.is_empty() {
                        tex.content.as_ref().to_vec()
                    } else {
                        let relative = tex.relative_filename.as_ref();
                        let candidate = path
                            .parent()
                            .unwrap_or(Path::new("."))
                            .join(relative.replace('\\', "/"));
                        let candidate = if candidate.is_file() {
                            candidate
                        } else {
                            PathBuf::from(tex.filename.as_ref())
                        };
                        fs::read(&candidate).with_context(|| {
                            format!("missing diffuse texture {}", candidate.display())
                        })?
                    };
                    let id = textures.len();
                    textures.push(
                        image::load_from_memory(&bytes)
                            .context("diffuse texture must be PNG or JPEG")?
                            .to_rgba8(),
                    );
                    tex_ids.insert(key, id);
                    Some(id)
                }
            } else {
                None
            };
            let count = ufbx::triangulate_face_vec(&mut indices, mesh, *face) as usize;
            for tri in indices[..count * 3].chunks_exact(3) {
                let mut t = Triangle {
                    p: [Vec3::ZERO; 3],
                    colors: [color; 3],
                    uv: [Vec2::ZERO; 3],
                    texture,
                };
                for (j, &index) in tri.iter().enumerate() {
                    t.p[j] = transform.transform_point3(v3(mesh.vertex_position[index as usize]));
                    if mesh.vertex_color.exists {
                        let c = mesh.vertex_color[index as usize];
                        t.colors[j] *= Vec3::new(c.x as f32, c.y as f32, c.z as f32)
                    }
                    if mesh.vertex_uv.exists {
                        let v = mesh.vertex_uv[index as usize];
                        t.uv[j] = Vec2::new(v.x as f32, 1. - v.y as f32)
                    }
                }
                triangles.push(t);
            }
        }
    }
    checked_mesh(triangles, textures)
}

pub fn read_dom(bytes: &[u8]) -> Result<WeakDom> {
    if bytes.starts_with(b"<roblox!") {
        Ok(rbx_binary::from_reader(bytes)?)
    } else {
        Ok(rbx_xml::from_reader(
            bytes,
            rbx_xml::DecodeOptions::new()
                .property_behavior(rbx_xml::DecodePropertyBehavior::ReadUnknown),
        )?)
    }
}
fn property<'a>(i: &'a rbx_dom_weak::Instance, names: &[&str]) -> Option<&'a Variant> {
    names.iter().find_map(|&s| i.properties.get(&s.into()))
}
fn vector(i: &rbx_dom_weak::Instance, names: &[&str], fallback: Vec3) -> Vec3 {
    if let Some(Variant::Vector3(v)) = property(i, names) {
        Vec3::new(v.x, v.y, v.z)
    } else {
        fallback
    }
}
fn uri(v: Option<&Variant>) -> Option<String> {
    match v {
        Some(Variant::ContentId(v)) => Some(v.as_str().to_string()),
        Some(Variant::Content(v)) => v.as_uri().map(str::to_owned),
        _ => None,
    }
}
fn roblox(path: &Path, assets: &Assets) -> Result<Source> {
    let dom = read_dom(&fs::read(path)?)?;
    let mut triangles = Vec::new();
    let mut textures = Vec::new();
    let mut pending = dom.root().children().to_vec();
    while let Some(id) = pending.pop() {
        let i = dom.get_by_ref(id).unwrap();
        pending.extend(i.children());
        let class = i.class.as_str();
        if matches!(
            class,
            "UnionOperation" | "NegateOperation" | "PartOperation"
        ) {
            bail!(
                "CSG instance {:?}: export unions to OBJ/FBX or convert to MeshParts first",
                i.name
            )
        }
        if !matches!(
            class,
            "Part"
                | "WedgePart"
                | "CornerWedgePart"
                | "MeshPart"
                | "TrussPart"
                | "SpawnLocation"
                | "Seat"
                | "VehicleSeat"
        ) {
            continue;
        }
        if matches!(property(i,&["Transparency"]),Some(Variant::Float32(v)) if *v>=1.) {
            continue;
        }
        let size = vector(i, &["Size", "size"], Vec3::ONE);
        ensure!(
            size.is_finite() && size.min_element() > 0.,
            "invalid part size"
        );
        let (pos, rot) = if let Some(Variant::CFrame(cf)) = property(i, &["CFrame"]) {
            let r = cf.orientation;
            (
                Vec3::new(cf.position.x, cf.position.y, cf.position.z),
                Mat3::from_cols(
                    Vec3::new(r.x.x, r.y.x, r.z.x),
                    Vec3::new(r.x.y, r.y.y, r.z.y),
                    Vec3::new(r.x.z, r.y.z, r.z.z),
                ),
            )
        } else {
            (Vec3::ZERO, Mat3::IDENTITY)
        };
        let color = match property(i, &["Color", "Color3uint8"]) {
            Some(Variant::Color3(c)) => Vec3::new(c.r, c.g, c.b),
            Some(Variant::Color3uint8(c)) => Vec3::new(c.r as f32, c.g as f32, c.b as f32) / 255.,
            _ => Vec3::splat(0.65),
        };
        let special = i
            .children()
            .iter()
            .filter_map(|id| dom.get_by_ref(*id))
            .find(|c| c.class.as_str() == "SpecialMesh");
        let content = uri(property(i, &["MeshId", "MeshContent"]))
            .or_else(|| special.and_then(|s| uri(property(s, &["MeshId"]))));
        let mut mesh = if let Some(content) = content.filter(|s| !s.is_empty()) {
            if let Some(shape) = Shape::ALL
                .into_iter()
                .find(|s| s.mesh_id().and_then(|id| id.parse().ok()) == asset_id(&content))
            {
                primitive(shape, color)
            } else {
                let bytes = assets.load(&content)?;
                let mut ts = if bytes.starts_with(b"version ") {
                    file_mesh(&bytes)?
                } else {
                    let temp = ufbx::load_memory(
                        &bytes,
                        ufbx::LoadOpts {
                            filename: "mapped.obj".into(),
                            ..Default::default()
                        },
                    )
                    .map_err(|e| {
                        anyhow::anyhow!("mapped mesh must be Roblox FileMesh or OBJ: {e:?}")
                    })?;
                    let mut ts = Vec::new();
                    for node in &temp.nodes {
                        if let Some(m) = &node.mesh {
                            let mut inds = Vec::new();
                            for f in &m.faces {
                                let n = ufbx::triangulate_face_vec(&mut inds, m, *f) as usize;
                                for a in inds[..n * 3].chunks_exact(3) {
                                    ts.push(Triangle {
                                        p: [
                                            v3(m.vertex_position[a[0] as usize]),
                                            v3(m.vertex_position[a[1] as usize]),
                                            v3(m.vertex_position[a[2] as usize]),
                                        ],
                                        colors: [Vec3::ONE; 3],
                                        uv: [Vec2::ZERO; 3],
                                        texture: None,
                                    })
                                }
                            }
                        }
                    }
                    ts
                };
                let mut b = Bounds::empty();
                for t in &ts {
                    for p in t.p {
                        b.add(p)
                    }
                }
                let extent = b.size().max(Vec3::splat(1e-6));
                for t in &mut ts {
                    for j in 0..3 {
                        t.p[j] = if class == "MeshPart" {
                            (t.p[j] - b.center()) / extent
                        } else {
                            t.p[j]
                        };
                        t.colors[j] *= color
                    }
                }
                ts
            }
        } else {
            ensure!(
                class != "TrussPart",
                "TrussPart import requires conversion to a mesh"
            );
            let shape = match class {
                "WedgePart" => Shape::Wedge,
                "CornerWedgePart" => Shape::CornerWedge,
                _ => match property(i, &["Shape", "shape"]) {
                    Some(Variant::Enum(e)) if e.to_u32() == 0 => Shape::Ball,
                    Some(Variant::Enum(e)) if e.to_u32() == 2 => Shape::Cylinder,
                    Some(Variant::Enum(e)) if e.to_u32() == 3 => Shape::Wedge,
                    Some(Variant::Enum(e)) if e.to_u32() == 4 => Shape::CornerWedge,
                    _ => Shape::Block,
                },
            };
            if let Some(s) = special {
                bail!(
                    "SpecialMesh {:?} without a file mesh is not supported; export to OBJ/FBX",
                    s.name
                )
            }
            primitive(shape, color)
        };
        let mut texture_uri = uri(property(i, &["TextureID", "TextureId", "TextureContent"]))
            .or_else(|| special.and_then(|s| uri(property(s, &["TextureId"]))));
        for child in i.children().iter().filter_map(|id| dom.get_by_ref(*id)) {
            if child.class.as_str() == "SurfaceAppearance" {
                texture_uri = uri(property(child, &["ColorMap", "ColorMapContent"]))
            }
        }
        if let Some(uri) = texture_uri.filter(|s| !s.is_empty()) {
            let id = textures.len();
            textures.push(image::load_from_memory(&assets.load(&uri)?)?.to_rgba8());
            for t in &mut mesh {
                t.texture = Some(id)
            }
        }
        for t in &mut mesh {
            for p in &mut t.p {
                let q = if let Some(s) = special {
                    *p * vector(s, &["Scale"], Vec3::ONE) + vector(s, &["Offset"], Vec3::ZERO)
                } else {
                    *p * size
                };
                *p = pos + rot * q;
            }
        }
        triangles.extend(mesh);
    }
    checked_mesh(triangles, textures)
}
fn primitive(shape: Shape, color: Vec3) -> Vec<Triangle> {
    shape
        .triangles()
        .into_iter()
        .map(|p| Triangle {
            p,
            colors: [color; 3],
            uv: [Vec2::ZERO; 3],
            texture: None,
        })
        .collect()
}
pub(crate) fn file_mesh(bytes: &[u8]) -> Result<Vec<Triangle>> {
    use rbx_mesh::mesh::*;
    if bytes.starts_with(b"version 6.00\n") || bytes.starts_with(b"version 7.00\n") {
        return crate::mesh::chunked(bytes);
    }
    // v5 may omit FACS data; geometry shares v4's layout after its extended header.
    if bytes.starts_with(b"version 5.00\n") {
        ensure!(bytes.len() >= 45, "truncated v5 mesh header");
        let facs_size = u32::from_le_bytes(bytes[41..45].try_into()?) as usize;
        ensure!(facs_size <= bytes.len() - 45, "invalid v5 FACS length");
        let mut v4 = b"version 4.00\n".to_vec();
        v4.extend_from_slice(&24u16.to_le_bytes());
        v4.extend_from_slice(&bytes[15..37]);
        v4.extend_from_slice(&bytes[45..bytes.len() - facs_size]);
        return file_mesh(&v4);
    }
    let mesh = rbx_mesh::read_mesh_versioned(Cursor::new(bytes))
        .context("unsupported/corrupt Roblox FileMesh")?;
    let unpack = |v: Vertices2| match v {
        Vertices2::Full(v) => v,
        Vertices2::Truncated(v) => v
            .into_iter()
            .map(|v| Vertex2 {
                pos: v.pos,
                norm: v.norm,
                tex: v.tex,
                tangent: v.tangent,
                color: [255; 4],
            })
            .collect(),
    };
    let (v, f, lods) = match mesh {
        Mesh::V1(m) => {
            let v: Vec<_> = m
                .vertices
                .into_iter()
                .map(|v| Vertex2 {
                    pos: v.pos,
                    norm: v.norm,
                    tex: [v.tex[0], v.tex[1]],
                    tangent: [0; 4],
                    color: [255; 4],
                })
                .collect();
            let f = (0..v.len() / 3)
                .map(|i| {
                    Face2([
                        VertexId2((i * 3) as u32),
                        VertexId2((i * 3 + 1) as u32),
                        VertexId2((i * 3 + 2) as u32),
                    ])
                })
                .collect();
            (v, f, vec![])
        }
        Mesh::V2(m) => (unpack(m.vertices), m.faces, vec![]),
        Mesh::V3(m) => (unpack(m.vertices), m.faces, m.lods),
        Mesh::V4(m) => (m.vertices, m.faces, m.lods),
        Mesh::V5(m) => (m.vertices, m.faces, m.lods),
    };
    let end = lods.get(1).map_or(f.len(), |l| l.0 as usize);
    let mut out = Vec::new();
    for face in f.iter().take(if end == 0 { f.len() } else { end }) {
        let mut t = Triangle {
            p: [Vec3::ZERO; 3],
            colors: [Vec3::ONE; 3],
            uv: [Vec2::ZERO; 3],
            texture: None,
        };
        for j in 0..3 {
            let v = v
                .get(face.0[j].0 as usize)
                .context("mesh index out of bounds")?;
            t.p[j] = Vec3::from_array(v.pos);
            t.colors[j] = Vec3::new(v.color[0] as f32, v.color[1] as f32, v.color[2] as f32) / 255.;
            t.uv[j] = Vec2::from_array(v.tex)
        }
        out.push(t)
    }
    Ok(out)
}
