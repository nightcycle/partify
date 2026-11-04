use crate::{
    config::Tuning,
    footprint,
    geometry::{Bounds, Bvh, Shape, Triangle, halton, principal_frame, rotations},
    import::Source,
};
use anyhow::{Result, bail, ensure};
use glam::{Mat3, Vec2, Vec3};
use image::RgbaImage;
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug, Serialize)]
pub struct Sample {
    pub position: Vec3,
    pub color: Vec3,
    pub normal: Vec3,
    pub edge: f32,
    pub region: u32,
}
#[derive(Clone, Debug, Serialize)]
pub struct Part {
    pub shape: Shape,
    pub center: Vec3,
    pub size: Vec3,
    pub rotation: Mat3,
    pub color: Vec3,
    pub loss: f32,
    pub volume: usize,
}
impl Part {
    fn within_footprint(&self, polygon: &[Vec2], frame: Mat3) -> bool {
        if polygon.is_empty() {
            return true;
        }
        // Test support against each plane, using exact support for curved parts.
        // Bounding boxes may overlap across a diagonal; solid interiors may not.
        let vertices = if matches!(
            self.shape,
            Shape::Block | Shape::Quad | Shape::Ball | Shape::Cylinder
        ) {
            Vec::new()
        } else {
            self.shape.triangles()
        };
        footprint::edges(polygon).all(|(a, b)| {
            let edge = b - a;
            let normal = Vec2::new(edge.y, -edge.x).normalize_or_zero();
            let world_normal = frame * normal.extend(0.);
            let q = (self.rotation.transpose() * world_normal) * self.size;
            let support = match self.shape {
                Shape::Block | Shape::Quad => q.abs().element_sum() * 0.5,
                Shape::Ball => q.length() * 0.5,
                Shape::Cylinder => (q.x.abs() + Vec2::new(q.y, q.z).length()) * 0.5,
                _ => vertices
                    .iter()
                    .flatten()
                    .map(|p| q.dot(*p))
                    .fold(f32::NEG_INFINITY, f32::max),
            };
            world_normal.dot(self.center) + support <= normal.dot(a) + 1e-5
        })
    }
    pub fn contains(&self, p: Vec3) -> bool {
        self.shape
            .contains((self.rotation.transpose() * (p - self.center)) / self.size)
    }
    /// Shared faces are valid contacts, not positive-volume overlaps.
    pub fn contains_interior(&self, p: Vec3) -> bool {
        let p = (self.rotation.transpose() * (p - self.center)) / self.size;
        [Vec3::X, Vec3::Y, Vec3::Z].into_iter().all(|axis| {
            self.shape.contains(p + axis * 1e-4) && self.shape.contains(p - axis * 1e-4)
        })
    }
    pub fn triangles(&self) -> Vec<[Vec3; 3]> {
        self.shape
            .triangles()
            .into_iter()
            .map(|t| t.map(|p| self.center + self.rotation * (p * self.size)))
            .collect()
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Volume {
    pub bounds: Bounds,
    /// Counterclockwise XY polygon in the partition frame; empty for 3D boxes.
    pub footprint: Vec<Vec2>,
    pub color: Vec3,
    pub sample_count: usize,
}
#[derive(Clone, Debug, Serialize)]
pub struct Connection {
    pub a: usize,
    pub b: usize,
    pub center: Vec3,
    pub normal: Vec3,
    pub area: f32,
    pub touching: bool,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct Metrics {
    pub samples: usize,
    pub regions: usize,
    pub parts: usize,
    pub coverage: f32,
    pub mean_color_error: f32,
    pub sampled_overlap: f32,
    pub loss: f32,
}
#[derive(Clone, Serialize)]
pub struct Build {
    pub tuning: Tuning,
    pub samples: Vec<Sample>,
    pub volumes: Vec<Volume>,
    pub connections: Vec<Connection>,
    pub parts: Vec<Part>,
    pub frame: Mat3,
    pub metrics: Metrics,
    pub warnings: Vec<String>,
}
enum Target {
    Image {
        pixels: RgbaImage,
        bounds: Bounds,
        width: f32,
        height: f32,
        depth: f32,
        alpha: f32,
    },
    Mesh {
        triangles: Vec<Triangle>,
        textures: Vec<RgbaImage>,
        bvh: Bvh,
        shell: f32,
        solid: bool,
    },
}
impl Target {
    fn query(&self, p: Vec3) -> Option<Vec3> {
        match self {
            Self::Image {
                pixels,
                width,
                height,
                depth,
                alpha,
                ..
            } => {
                if p.z.abs() > *depth * 0.5001 {
                    return None;
                }
                let u = p.x / width + 0.5;
                let v = 0.5 - p.y / height;
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    return None;
                }
                let c = pixels.get_pixel(
                    (u * pixels.width() as f32) as u32,
                    (v * pixels.height() as f32) as u32,
                );
                (c[3] as f32 / 255. > *alpha)
                    .then(|| Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32) / 255.)
            }
            Self::Mesh {
                triangles,
                textures,
                bvh,
                shell,
                solid,
            } => {
                let (i, w, d) = bvh.nearest(triangles, p);
                let occupied = if *solid {
                    d <= 1e-8 || bvh.inside(triangles, p)
                } else {
                    d <= shell * shell * 0.25
                };
                if !occupied {
                    return None;
                }
                Some(triangle_color(&triangles[i], w, textures))
            }
        }
    }
}
fn triangle_color(t: &Triangle, w: Vec3, textures: &[RgbaImage]) -> Vec3 {
    let mut color = t.colors[0] * w.x + t.colors[1] * w.y + t.colors[2] * w.z;
    if let Some(id) = t.texture {
        let image = &textures[id];
        let uv = t.uv[0] * w.x + t.uv[1] * w.y + t.uv[2] * w.z;
        let x = (uv.x.rem_euclid(1.) * image.width() as f32) as u32;
        let y = (uv.y.rem_euclid(1.) * image.height() as f32) as u32;
        let c = image.get_pixel(x.min(image.width() - 1), y.min(image.height() - 1));
        color *= Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32) / 255.;
    }
    color.clamp(Vec3::ZERO, Vec3::ONE)
}
fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled")
    }
    Ok(())
}
fn image_pixel_order(x: u32, y: u32) -> u32 {
    // Morton order interleaves coordinate bits, keeping consecutive pixels
    // spatially local in both axes instead of grouping them into scanlines.
    fn spread(mut v: u32) -> u32 {
        v = (v | v << 8) & 0x00ff_00ff;
        v = (v | v << 4) & 0x0f0f_0f0f;
        v = (v | v << 2) & 0x3333_3333;
        (v | v << 1) & 0x5555_5555
    }
    spread(x) | (spread(y) << 1)
}
fn prepare(
    source: &Source,
    t: &Tuning,
    cancel: &AtomicBool,
) -> Result<(Target, Vec<Sample>, f32, Vec<String>)> {
    let mut samples = Vec::new();
    let mut warnings = Vec::new();
    match source {
        Source::Image(image) => {
            let image = image::DynamicImage::ImageRgba8(image.clone())
                .resize(
                    t.image_resolution,
                    t.image_resolution,
                    image::imageops::FilterType::Triangle,
                )
                .to_rgba8();
            let pixel_size = t.size / image.width().max(image.height()) as f32;
            let width = image.width() as f32 * pixel_size;
            let height = image.height() as f32 * pixel_size;
            let mut visible = Vec::new();
            let mut bounds = Bounds::empty();
            for (x, y, c) in image.enumerate_pixels() {
                if c[3] as f32 / 255. > t.alpha_threshold {
                    bounds.add(Vec3::new(
                        (x as f32 + 0.5) * pixel_size - width * 0.5,
                        height * 0.5 - (y as f32 + 0.5) * pixel_size,
                        0.,
                    ));
                    visible.push((x, y, *c))
                }
            }
            // Thinning row-major pixels locks onto a few columns whenever the
            // stride divides the image width. Spatial ordering distributes the
            // retained samples across both axes, including when the viewer
            // further reduces the number of points it draws.
            visible.sort_unstable_by_key(|&(x, y, _)| image_pixel_order(x, y));
            let stride = visible.len().div_ceil(t.sample_limit).max(1);
            for &(x, y, c) in visible.iter().step_by(stride) {
                let color = Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32) / 255.;
                let mut edge = 0.;
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= image.width() as i32 || ny >= image.height() as i32
                    {
                        edge = 1.;
                        continue;
                    }
                    let q = image.get_pixel(nx as u32, ny as u32);
                    let qc = Vec3::new(q[0] as f32, q[1] as f32, q[2] as f32) / 255.;
                    if q[3] as f32 / 255. <= t.alpha_threshold
                        || color.distance(qc) > t.color_threshold
                    {
                        edge = 1.
                    }
                }
                samples.push(Sample {
                    position: Vec3::new(
                        (x as f32 + 0.5) * pixel_size - width * 0.5,
                        height * 0.5 - (y as f32 + 0.5) * pixel_size,
                        0.,
                    ),
                    color,
                    normal: Vec3::Z,
                    edge,
                    region: 0,
                });
            }
            ensure!(
                !samples.is_empty(),
                "image contains no pixels above alpha_threshold"
            );
            let pad = Vec3::new(pixel_size * 0.5, pixel_size * 0.5, t.thickness * 0.5);
            bounds.min -= pad;
            bounds.max += pad;
            Ok((
                Target::Image {
                    pixels: image,
                    bounds,
                    width,
                    height,
                    depth: t.thickness,
                    alpha: t.alpha_threshold,
                },
                samples,
                pixel_size,
                warnings,
            ))
        }
        Source::Mesh {
            triangles,
            textures,
        } => {
            let mut b = Bounds::empty();
            for tri in triangles {
                for p in tri.p {
                    b.add(p)
                }
            }
            let scale = t.size / b.size().max_element();
            ensure!(scale.is_finite(), "degenerate mesh bounds");
            let mut triangles = triangles.clone();
            for tri in &mut triangles {
                for p in &mut tri.p {
                    *p = (*p - b.center()) * scale
                }
            }
            let bvh = Bvh::new(&triangles);
            let shell = t.shell_thickness;
            // Boundary-edge detection is geometric, so UV seams do not appear as holes.
            let key = |p: Vec3| {
                (
                    (p.x * 10000.).round() as i64,
                    (p.y * 10000.).round() as i64,
                    (p.z * 10000.).round() as i64,
                )
            };
            let mut edges = HashMap::new();
            for tri in &triangles {
                for j in 0..3 {
                    let a = key(tri.p[j]);
                    let b = key(tri.p[(j + 1) % 3]);
                    let pair = if a < b { (a, b) } else { (b, a) };
                    *edges.entry(pair).or_insert(0usize) += 1;
                }
            }
            let closed = edges.values().all(|&n| n == 2);
            let solid = t.solid && closed;
            if t.solid && !closed {
                warnings.push("Open or non-manifold geometry: using surface shells instead of ambiguous inside/outside filling.".into())
            }
            let mut areas = Vec::new();
            let mut total = 0.;
            for tri in &triangles {
                total += tri.area();
                areas.push(total)
            }
            let surface_count = if solid {
                t.sample_limit * 2 / 3
            } else {
                t.sample_limit
            };
            for i in 0..surface_count {
                if i % 512 == 0 {
                    cancelled(cancel)?
                }
                let n = i + 1 + t.seed as usize;
                let a = halton(n, 2) * total;
                let j = areas.partition_point(|v| *v < a).min(triangles.len() - 1);
                let u = halton(n, 3).sqrt();
                let v = halton(n, 5);
                let w = Vec3::new(1. - u, u * (1. - v), u * v);
                let tri = &triangles[j];
                let offset = if solid {
                    Vec3::ZERO
                } else {
                    tri.normal() * shell * (halton(n, 7) - 0.5)
                };
                samples.push(Sample {
                    position: tri.p[0] * w.x + tri.p[1] * w.y + tri.p[2] * w.z + offset,
                    color: triangle_color(tri, w, textures),
                    normal: tri.normal(),
                    edge: 0.,
                    region: 0,
                });
            }
            let extent = b.size() * scale;
            let target = Target::Mesh {
                triangles,
                textures: textures.clone(),
                bvh,
                shell,
                solid,
            };
            if solid {
                for i in 0..t.sample_limit * 10 {
                    if samples.len() >= t.sample_limit {
                        break;
                    }
                    if i % 512 == 0 {
                        cancelled(cancel)?
                    }
                    let n = i + 1 + t.seed as usize;
                    let p = (Vec3::new(halton(n, 2), halton(n, 3), halton(n, 5))
                        - Vec3::splat(0.5))
                        * extent;
                    if let Some(color) = target.query(p) {
                        samples.push(Sample {
                            position: p,
                            color,
                            normal: Vec3::ZERO,
                            edge: 0.,
                            region: 0,
                        })
                    }
                }
            }
            Ok((
                target,
                samples,
                (total / surface_count as f32).sqrt(),
                warnings,
            ))
        }
    }
}

fn features(samples: &mut [Sample], radius: f32, t: &Tuning) -> usize {
    let mut grid: HashMap<(i32, i32, i32), Vec<usize>> = HashMap::new();
    let cell = radius.max(f32::EPSILON) * 2.;
    let key = |p: Vec3| {
        let a = (p / cell).floor().as_ivec3();
        (a.x, a.y, a.z)
    };
    for (i, s) in samples.iter().enumerate() {
        grid.entry(key(s.position)).or_default().push(i)
    }
    let levels = (t.color_levels - 1) as f32;
    let mut palette = HashMap::new();
    for i in 0..samples.len() {
        let c = (samples[i].color * levels).round().as_ivec3();
        let next = palette.len() as u32;
        samples[i].region = *palette.entry((c.x, c.y, c.z)).or_insert(next);
        let (x, y, z) = key(samples[i].position);
        let mut edge = samples[i].edge;
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(ids) = grid.get(&(x + dx, y + dy, z + dz)) {
                        for &j in ids.iter().step_by((ids.len() / 24).max(1)) {
                            if samples[i].position.distance_squared(samples[j].position)
                                > cell * cell
                            {
                                continue;
                            }
                            let normal_edge = samples[i].normal.length_squared() > 0.5
                                && samples[j].normal.length_squared() > 0.5
                                && samples[i].normal.dot(samples[j].normal)
                                    < t.edge_angle.to_radians().cos();
                            if normal_edge
                                || samples[i].color.distance(samples[j].color) > t.color_threshold
                            {
                                edge = 1.;
                                break;
                            }
                        }
                    }
                }
            }
        }
        samples[i].edge = edge;
    }
    palette.len()
}
#[derive(Clone)]
struct Leaf {
    ids: Vec<usize>,
    bounds: Bounds,
    footprint: Vec<Vec2>,
    faces: u8,
    part: Part,
    loss: f32,
    proposal: Option<Box<Proposal>>,
}
#[derive(Clone)]
struct Proposal {
    a: Leaf,
    b: Leaf,
    gain: f32,
}
struct Fitter<'a> {
    samples: &'a [Sample],
    local: Vec<Vec3>,
    target: &'a Target,
    t: &'a Tuning,
    frame: Mat3,
    flat: bool,
    pixel: f32,
    rotations: Vec<Mat3>,
    cancel: &'a AtomicBool,
}
impl Fitter<'_> {
    fn score(
        &self,
        part: &Part,
        ids: &[usize],
        cell: Bounds,
        faces: u8,
        footprint: &[Vec2],
        exact: bool,
    ) -> f32 {
        let count = if exact {
            ids.len()
        } else {
            ids.len().min(self.t.fit_samples)
        };
        let mut miss = 0.;
        let mut color = 0.;
        let mut weight = 0.;
        for j in 0..count {
            let start = j * ids.len() / count;
            let end = (j + 1) * ids.len() / count;
            let offset = (j
                .wrapping_mul(2654435761)
                .wrapping_add(self.t.seed as usize))
                % (end - start);
            let i = ids[start + offset];
            let s = &self.samples[i];
            let w = 1. + self.t.edge_weight * s.edge;
            weight += w;
            if !part.contains(s.position) {
                miss += w
            }
            color += s.color.distance_squared(part.color) * w;
        }
        let mut outside = 0.;
        let mut inside: f32 = 0.;
        let mut probe_color = 0.;
        for i in 0..self.t.probes {
            let n = i + 1 + self.t.seed as usize;
            let q = Vec3::new(halton(n, 2), halton(n, 3), halton(n, 5)) - Vec3::splat(0.5);
            if !part.shape.contains(q) {
                continue;
            }
            inside += 1.;
            let p = part.center + part.rotation * (part.size * q);
            if let Some(c) = self.target.query(p) {
                probe_color += c.distance_squared(part.color)
            } else {
                outside += 1.
            }
        }
        let mut contacts = 0.;
        let mut missed_contacts = 0.;
        for face in 0..6 {
            if faces & (1 << face) == 0 {
                continue;
            }
            let axis = face / 2;
            let a = (axis + 1) % 3;
            let b = (axis + 2) % 3;
            for u in [0.25, 0.5, 0.75] {
                for v in [0.25, 0.5, 0.75] {
                    let mut p = cell.center();
                    p[axis] = if face % 2 == 0 {
                        cell.min[axis] + 1e-5
                    } else {
                        cell.max[axis] - 1e-5
                    };
                    p[a] = cell.min[a] + u * cell.size()[a];
                    p[b] = cell.min[b] + v * cell.size()[b];
                    let p = self.frame * p;
                    if self.target.query(p).is_some() {
                        contacts += 1.;
                        if !part.contains(p) {
                            missed_contacts += 1.
                        }
                    }
                }
            }
        }
        // Axis faces are tracked above. Sloping polygon edges are also shared
        // connection surfaces; otherwise tight sample bounds leave visible gaps.
        for (a, b) in footprint::edges(footprint) {
            let edge = b - a;
            if edge.x.abs() <= 1e-5 || edge.y.abs() <= 1e-5 {
                continue;
            }
            for u in [0.25, 0.5, 0.75] {
                for v in [0.25, 0.5, 0.75] {
                    let p = self.frame * a.lerp(b, u).extend(cell.min.z + cell.size().z * v);
                    if self.target.query(p).is_some() {
                        contacts += 1.;
                        if !part.contains(p) {
                            missed_contacts += 1.;
                        }
                    }
                }
            }
        }
        let contact_loss = if contacts > 0. {
            missed_contacts / contacts
        } else {
            0.
        };
        self.t.coverage_weight * miss / weight.max(1.)
            + self.t.color_weight * (color / weight.max(1.) + probe_color / inside.max(1.) * 0.25)
            + self.t.spill_weight * outside / inside.max(1.)
            + self.t.connection_weight * contact_loss
    }
    fn fit(
        &self,
        ids: Vec<usize>,
        cell: Bounds,
        faces: u8,
        footprint: Vec<Vec2>,
    ) -> Result<Option<Leaf>> {
        cancelled(self.cancel)?;
        let mut bound = Bounds::empty();
        let mut color = Vec3::ZERO;
        for &i in &ids {
            bound.add(self.local[i]);
            color += self.samples[i].color
        }
        color /= ids.len() as f32;
        let pad = if self.flat {
            Vec3::new(self.pixel * 0.5, self.pixel * 0.5, self.t.thickness * 0.5)
        } else {
            Vec3::splat(self.t.min_size * 0.5)
        };
        bound.min = (bound.min - pad).max(cell.min);
        bound.max = (bound.max + pad).min(cell.max);
        // Only enlarge within the partition; exported shapes cannot cross sibling cells.
        let size = bound
            .size()
            .max(Vec3::splat(self.t.min_size))
            .min(cell.size());
        let center = bound
            .center()
            .clamp(cell.min + size * 0.5, cell.max - size * 0.5);
        let baseline = Part {
            shape: Shape::Block,
            center: self.frame * center,
            size,
            rotation: self.frame,
            color,
            loss: f32::INFINITY,
            volume: 0,
        };
        let mut candidates = vec![baseline.clone()];
        for (size, center) in [(size, center), (cell.size(), cell.center())] {
            for shape in Shape::ALL {
                if shape.mesh_id().is_some() && !self.t.allow_meshes {
                    continue;
                }
                // The quad is a zero-volume XZ surface; it only represents image faces.
                if shape == Shape::Quad && !self.flat {
                    continue;
                }
                for rotation in &self.rotations {
                    if matches!(shape, Shape::Block | Shape::Ball) && *rotation != Mat3::IDENTITY {
                        continue;
                    }
                    if shape == Shape::Quad
                        && (self.frame * *rotation * Vec3::Y).dot(Vec3::Z).abs() < 0.99
                    {
                        continue;
                    }
                    let rotated = rotation.transpose() * size;
                    let mut part = Part {
                        shape,
                        center: self.frame * center,
                        size: rotated.abs(),
                        rotation: self.frame * *rotation,
                        color,
                        loss: 0.,
                        volume: 0,
                    };
                    part.loss = self.score(&part, &ids, cell, faces, &footprint, false);
                    // Prefer simpler solid primitives in exact ties. Quad has no hidden thickness.
                    if shape == Shape::Quad {
                        part.loss += 0.0001
                    }
                    candidates.push(part);
                }
            }
        }
        candidates.retain(|p| p.within_footprint(&footprint, self.frame));
        if candidates.is_empty() {
            return Ok(None);
        }
        candidates.sort_by(|a, b| a.loss.total_cmp(&b.loss));
        candidates.truncate(8);
        if baseline.within_footprint(&footprint, self.frame) {
            candidates.push(baseline);
        }
        let mut best = candidates[0].clone();
        best.loss = f32::INFINITY;
        for mut part in candidates {
            part.loss = self.score(&part, &ids, cell, faces, &footprint, true);
            if part.shape == Shape::Quad {
                part.loss += 0.0001
            }
            if part.loss < best.loss {
                best = part
            }
        }
        let loss = best.loss * ids.len() as f32;
        Ok(Some(Leaf {
            ids,
            bounds: cell,
            footprint,
            faces,
            part: best,
            loss,
            proposal: None,
        }))
    }
    fn split(
        &self,
        leaf: &Leaf,
        normal: Vec3,
        offset: f32,
        axis: Option<usize>,
    ) -> Result<Option<Box<Proposal>>> {
        let (mut a_ids, mut b_ids, mut boundary) = (Vec::new(), Vec::new(), Vec::new());
        for &i in &leaf.ids {
            let distance = normal.dot(self.local[i]) - offset;
            if distance.abs() <= 1e-5 {
                boundary.push(i);
            } else if distance < 0. {
                a_ids.push(i);
            } else {
                b_ids.push(i);
            }
        }
        if a_ids.is_empty() || b_ids.is_empty() {
            return Ok(None);
        }
        // A sample on the shared face belongs to the side with its color.
        let mean = |ids: &[usize]| {
            ids.iter().map(|&i| self.samples[i].color).sum::<Vec3>() / ids.len() as f32
        };
        let (a_color, b_color) = (mean(&a_ids), mean(&b_ids));
        for i in boundary {
            if self.samples[i].color.distance_squared(a_color)
                <= self.samples[i].color.distance_squared(b_color)
            {
                a_ids.push(i);
            } else {
                b_ids.push(i);
            }
        }
        let (mut a_bounds, mut b_bounds) = (leaf.bounds, leaf.bounds);
        let (mut a_faces, mut b_faces) = (leaf.faces, leaf.faces);
        if let Some(axis) = axis {
            a_bounds.max[axis] = offset;
            b_bounds.min[axis] = offset;
            a_faces |= 1 << (axis * 2 + 1);
            b_faces |= 1 << (axis * 2);
        }
        let (a_polygon, b_polygon) = if self.flat {
            let a = footprint::clip(&leaf.footprint, normal.truncate(), offset);
            let b = footprint::clip(&leaf.footprint, -normal.truncate(), -offset);
            let (Some(a_box), Some(b_box)) = (
                footprint::bounds(&a, a_bounds),
                footprint::bounds(&b, b_bounds),
            ) else {
                return Ok(None);
            };
            a_bounds = a_box;
            b_bounds = b_box;
            if a_bounds.size().truncate().min_element() < self.t.min_size
                || b_bounds.size().truncate().min_element() < self.t.min_size
            {
                return Ok(None);
            }
            (a, b)
        } else {
            (Vec::new(), Vec::new())
        };
        let Some(a) = self.fit(a_ids, a_bounds, a_faces, a_polygon)? else {
            return Ok(None);
        };
        let Some(b) = self.fit(b_ids, b_bounds, b_faces, b_polygon)? else {
            return Ok(None);
        };
        let gain =
            leaf.loss - a.loss - b.loss - self.t.complexity_penalty * self.samples.len() as f32;
        Ok((gain > 0.).then(|| Box::new(Proposal { a, b, gain })))
    }
    fn propose(&self, leaf: &Leaf) -> Result<Option<Box<Proposal>>> {
        if leaf.ids.len() < 4 {
            return Ok(None);
        }
        let mut best: Option<Box<Proposal>> = None;
        for axis in 0..if self.flat { 2 } else { 3 } {
            if leaf.bounds.size()[axis] < self.t.min_size * 2. {
                continue;
            }
            let mut sorted = leaf.ids.clone();
            sorted.sort_by(|&a, &b| self.local[a][axis].total_cmp(&self.local[b][axis]));
            let mut cuts = Vec::new();
            for j in 1..=self.t.split_candidates {
                let q = j as f32 / (self.t.split_candidates + 1) as f32;
                let k = ((sorted.len() as f32 * q) as usize).clamp(1, sorted.len() - 1);
                cuts.push(k)
            }
            // Compare neighboring slabs, not arbitrary adjacent points in a sort.
            // This locates whole color boundaries (such as a roof/wall interface)
            // without letting the ordering of equal-coordinate samples dominate.
            let step = if self.flat {
                self.pixel * 0.25
            } else {
                leaf.bounds.size()[axis] / 64.
            };
            let mut groups = Vec::new();
            let mut start = 0;
            while start < sorted.len() {
                let mut end = start + 1;
                let coordinate = self.local[sorted[start]][axis];
                while end < sorted.len() && self.local[sorted[end]][axis] <= coordinate + step {
                    end += 1
                }
                let mean = sorted[start..end]
                    .iter()
                    .map(|&i| self.samples[i].color)
                    .sum::<Vec3>()
                    / (end - start) as f32;
                groups.push((start, end, mean));
                start = end;
            }
            let mut features = Vec::new();
            for pair in groups.windows(2) {
                let (start_a, end_a, color_a) = pair[0];
                let (start_b, end_b, color_b) = pair[1];
                let density = ((end_a - start_a) as f32 - (end_b - start_b) as f32).abs()
                    / (end_b - start_a) as f32;
                features.push((color_a.distance_squared(color_b) + 0.1 * density, start_b));
            }
            features.sort_by(|a, b| b.0.total_cmp(&a.0));
            cuts.extend(features.iter().take(self.t.split_candidates).map(|p| p.1));
            cuts.sort_unstable();
            cuts.dedup();
            for k in cuts {
                let cut = (self.local[sorted[k - 1]][axis] + self.local[sorted[k]][axis]) * 0.5;
                if cut - leaf.bounds.min[axis] < self.t.min_size
                    || leaf.bounds.max[axis] - cut < self.t.min_size
                {
                    continue;
                }
                if let Some(proposal) = self.split(leaf, Vec3::AXES[axis], cut, Some(axis))?
                    && best.as_ref().is_none_or(|p| proposal.gain > p.gain)
                {
                    best = Some(proposal);
                }
            }
        }
        if self.flat {
            // Right-triangle prisms share a sloped face. Their bounding boxes
            // overlap, so constrain the actual shapes to the clipped polygons.
            for sign in [-1., 1.] {
                let normal = Vec3::new(1. / leaf.bounds.size().x, sign / leaf.bounds.size().y, 0.)
                    .normalize();
                if let Some(proposal) =
                    self.split(leaf, normal, normal.dot(leaf.bounds.center()), None)?
                    && best.as_ref().is_none_or(|p| proposal.gain > p.gain)
                {
                    best = Some(proposal);
                }
            }
        }
        Ok(best)
    }
}

pub fn run(
    source: &Source,
    tuning: &Tuning,
    cancel: &AtomicBool,
    progress: impl Fn(&str),
) -> Result<Build> {
    tuning.validate()?;
    progress("Sampling source");
    let (target, mut samples, pixel, warnings) = prepare(source, tuning, cancel)?;
    progress("Detecting color regions, curves and edges");
    let regions = features(&mut samples, pixel, tuning);
    cancelled(cancel)?;
    let flat = matches!(source, Source::Image(_));
    let points: Vec<_> = samples.iter().map(|s| s.position).collect();
    let candidate = principal_frame(&points, flat);
    let make_fitter = |frame: Mat3| Fitter {
        samples: &samples,
        local: points.iter().map(|p| frame.transpose() * *p).collect(),
        target: &target,
        t: tuning,
        frame,
        flat,
        pixel,
        rotations: rotations(),
        cancel,
    };
    progress("Fitting adaptive volumes");
    let mut frames = vec![Mat3::IDENTITY];
    if !flat {
        frames.push(candidate);
    }
    if let Target::Mesh { triangles, .. } = &target
        && let Some(tri) = triangles
            .iter()
            .max_by(|a, b| a.area().total_cmp(&b.area()))
    {
        let edges = [
            tri.p[1] - tri.p[0],
            tri.p[2] - tri.p[1],
            tri.p[0] - tri.p[2],
        ];
        let x = edges
            .into_iter()
            .min_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))
            .unwrap()
            .normalize();
        let z = tri.normal();
        frames.push(Mat3::from_cols(x, z.cross(x).normalize(), z));
    }
    let mut best_root: Option<(Mat3, Leaf)> = None;
    for frame in frames {
        let mut bounds;
        match &target {
            Target::Image {
                bounds: source_bounds,
                ..
            } => {
                // Sampling must not move the silhouette's extremities or the
                // endpoints of a diagonal partition.
                bounds = *source_bounds;
            }
            Target::Mesh {
                triangles,
                shell,
                solid,
                ..
            } => {
                bounds = Bounds::empty();
                for t in triangles {
                    for p in t.p {
                        bounds.add(frame.transpose() * p)
                    }
                }
                if !solid {
                    bounds.min -= Vec3::splat(shell * 0.5);
                    bounds.max += Vec3::splat(shell * 0.5);
                }
                let center = bounds.center();
                let size = bounds.size().max(Vec3::splat(tuning.min_size));
                bounds = Bounds {
                    min: center - size * 0.5,
                    max: center + size * 0.5,
                };
            }
        }
        let polygon = if flat {
            footprint::rectangle(bounds)
        } else {
            Vec::new()
        };
        let Some(root) =
            make_fitter(frame).fit((0..samples.len()).collect(), bounds, 0, polygon)?
        else {
            continue;
        };
        if best_root
            .as_ref()
            .is_none_or(|(_, old)| root.loss < old.loss - 1e-5)
        {
            best_root = Some((frame, root));
        }
    }
    let (frame, mut root) = best_root.unwrap();
    let fitter = make_fitter(frame);
    if tuning.budget > 1 {
        root.proposal = fitter.propose(&root)?
    }
    let mut leaves = vec![root];
    while leaves.len() < tuning.budget {
        cancelled(cancel)?;
        let Some((index, _)) = leaves
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.proposal.as_ref().map(|p| (i, p.gain)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
        else {
            break;
        };
        let proposal = leaves[index].proposal.take().unwrap();
        let Proposal { mut a, mut b, .. } = *proposal;
        if leaves.len() + 1 < tuning.budget {
            a.proposal = fitter.propose(&a)?;
            b.proposal = fitter.propose(&b)?
        }
        leaves[index] = a;
        leaves.push(b);
        progress(&format!(
            "Fitted {} / {} parts",
            leaves.len(),
            tuning.budget
        ));
    }
    progress("Refining fit and connection surfaces");
    for _ in 0..tuning.refinement_passes {
        for leaf in &mut leaves {
            cancelled(cancel)?;
            let original = leaf.part.clone();
            let mut best = original.clone();
            // Coordinate descent shrinks geometry in its existing disjoint partition.
            for axis in 0..3 {
                if flat && (original.rotation.col(axis).dot(Vec3::Z)).abs() > 0.99 {
                    continue;
                }
                for shrink in [0.94, 0.98] {
                    for side in [-1., 0., 1.] {
                        let mut part = original.clone();
                        let d = part.size[axis] * (1. - shrink);
                        if part.size[axis] - d < tuning.min_size {
                            continue;
                        }
                        part.size[axis] -= d;
                        part.center += part.rotation.col(axis) * (d * side * 0.5);
                        if !part.within_footprint(&leaf.footprint, frame) {
                            continue;
                        }
                        part.loss = fitter.score(
                            &part,
                            &leaf.ids,
                            leaf.bounds,
                            leaf.faces,
                            &leaf.footprint,
                            true,
                        );
                        if part.loss < best.loss {
                            best = part
                        }
                    }
                }
            }
            leaf.loss = best.loss * leaf.ids.len() as f32;
            leaf.part = best;
        }
    }
    let mut parts = Vec::new();
    let mut volumes = Vec::new();
    for (i, l) in leaves.iter().enumerate() {
        let mut p = l.part.clone();
        p.volume = i;
        parts.push(p);
        volumes.push(Volume {
            bounds: l.bounds,
            footprint: l.footprint.clone(),
            color: l.part.color,
            sample_count: l.ids.len(),
        })
    }
    let connections = connections(&volumes, &parts, frame);
    let mut covered = 0;
    let mut color_error = 0.;
    let mut overlaps = 0;
    for sample in &samples {
        let mut hits = 0;
        let mut interior_hits = 0;
        let mut error = f32::INFINITY;
        for part in &parts {
            if part.contains(sample.position) {
                hits += 1;
                if part.contains_interior(sample.position) {
                    interior_hits += 1;
                }
                error = error.min(sample.color.distance_squared(part.color))
            }
        }
        if hits > 0 {
            covered += 1;
            color_error += error
        }
        if interior_hits > 1 {
            overlaps += 1
        }
    }
    let metrics = Metrics {
        samples: samples.len(),
        regions,
        parts: parts.len(),
        coverage: covered as f32 / samples.len() as f32,
        mean_color_error: (color_error / (covered.max(1) as f32)).sqrt(),
        sampled_overlap: overlaps as f32 / samples.len() as f32,
        loss: leaves.iter().map(|l| l.loss).sum::<f32>() / samples.len() as f32
            + tuning.overlap_weight * overlaps as f32 / samples.len() as f32,
    };
    drop(fitter);
    Ok(Build {
        tuning: tuning.clone(),
        samples,
        volumes,
        connections,
        parts,
        frame,
        metrics,
        warnings,
    })
}
fn connections(volumes: &[Volume], parts: &[Part], frame: Mat3) -> Vec<Connection> {
    let mut out = Vec::new();
    for a in 0..volumes.len() {
        for b in a + 1..volumes.len() {
            let x = volumes[a].bounds;
            let y = volumes[b].bounds;
            if !volumes[a].footprint.is_empty() && !volumes[b].footprint.is_empty() {
                if let Some((point, normal, length)) =
                    footprint::shared_edge(&volumes[a].footprint, &volumes[b].footprint)
                {
                    let lo = x.min.z.max(y.min.z);
                    let hi = x.max.z.min(y.max.z);
                    if hi > lo {
                        let center = frame * point.extend((lo + hi) * 0.5);
                        let normal = frame * normal.extend(0.);
                        out.push(Connection {
                            a,
                            b,
                            center,
                            normal,
                            area: length * (hi - lo),
                            touching: parts[a].contains(center - normal * 1e-4)
                                && parts[b].contains(center + normal * 1e-4),
                        });
                    }
                }
                continue;
            }
            for axis in 0..3 {
                let sign = if (x.max[axis] - y.min[axis]).abs() < 1e-4 {
                    1.
                } else if (y.max[axis] - x.min[axis]).abs() < 1e-4 {
                    -1.
                } else {
                    continue;
                };
                let u = (axis + 1) % 3;
                let v = (axis + 2) % 3;
                let lo = x.min.max(y.min);
                let hi = x.max.min(y.max);
                let area = (hi[u] - lo[u]) * (hi[v] - lo[v]);
                if hi[u] - lo[u] <= 1e-5 || hi[v] - lo[v] <= 1e-5 {
                    continue;
                }
                let mut c = (lo + hi) * 0.5;
                c[axis] = if sign > 0. { x.max[axis] } else { x.min[axis] };
                let n = frame.col(axis) * sign;
                let center = frame * c;
                let touching =
                    parts[a].contains(center - n * 1e-4) && parts[b].contains(center + n * 1e-4);
                out.push(Connection {
                    a,
                    b,
                    center,
                    normal: n,
                    area,
                    touching,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    use std::collections::HashSet;

    #[test]
    fn image_samples_cover_both_axes_across_resolutions_and_limits() {
        for (width, height, limit) in [
            (200, 200, 800),
            (200, 200, 1024),
            (200, 133, 1024),
            (133, 200, 1024),
            (199, 199, 1024),
            (201, 201, 1024),
            (256, 256, 12000),
            (512, 512, 100000),
        ] {
            let source = Source::Image(RgbaImage::from_pixel(
                width,
                height,
                Rgba([80, 130, 220, 255]),
            ));
            let tuning = Tuning {
                image_resolution: width.max(height),
                sample_limit: limit,
                ..Default::default()
            };
            let (_, samples, pixel, _) =
                prepare(&source, &tuning, &AtomicBool::new(false)).unwrap();
            assert!(samples.len() <= limit);
            let mut unique = HashSet::new();
            for s in &samples {
                assert!(unique.insert((s.position.x.to_bits(), s.position.y.to_bits())));
            }
            // Check both the fitting samples and the subset drawn by the viewer.
            for stride in [1, (samples.len() / 25000).max(1)] {
                let mut bins = [0usize; 64];
                for s in samples.iter().step_by(stride) {
                    let u = s.position.x / (width as f32 * pixel) + 0.5;
                    let v = 0.5 - s.position.y / (height as f32 * pixel);
                    bins[(v * 8.) as usize * 8 + (u * 8.) as usize] += 1;
                }
                let mean = bins.iter().sum::<usize>() as f32 / 64.;
                assert!(
                    bins.iter()
                        .all(|&count| count as f32 > mean * 0.45 && (count as f32) < mean * 1.65),
                    "uneven sampling at {width}x{height}, limit {limit}, display stride {stride}: {bins:?}"
                );
            }
        }
    }

    #[test]
    fn image_sampling_preserves_alpha_colors_and_determinism() {
        let pixels = RgbaImage::from_fn(200, 200, |x, y| {
            Rgba([
                x as u8,
                y as u8,
                100,
                if !(40..160).contains(&x) && (20..180).contains(&y) {
                    255
                } else {
                    0
                },
            ])
        });
        let tuning = Tuning {
            image_resolution: 200,
            sample_limit: 800,
            ..Default::default()
        };
        let source = Source::Image(pixels.clone());
        let (_, samples, pixel, _) = prepare(&source, &tuning, &AtomicBool::new(false)).unwrap();
        let (_, again, _, _) = prepare(&source, &tuning, &AtomicBool::new(false)).unwrap();
        assert_eq!(samples.len(), again.len());
        assert!(samples.len() <= tuning.sample_limit);
        assert!(samples.iter().any(|s| s.position.x < 0.));
        assert!(samples.iter().any(|s| s.position.x > 0.));
        for (sample, repeated) in samples.iter().zip(&again) {
            assert_eq!(sample.position, repeated.position);
            let x = ((sample.position.x + tuning.size * 0.5) / pixel) as u32;
            let y = ((tuning.size * 0.5 - sample.position.y) / pixel) as u32;
            let color = pixels.get_pixel(x, y);
            assert_eq!(color[3], 255);
            assert_eq!(
                sample.color,
                Vec3::new(color[0] as f32, color[1] as f32, color[2] as f32) / 255.
            );
        }
    }

    #[test]
    fn volume_min_size_does_not_change_features() {
        let source = Source::Image(RgbaImage::from_fn(200, 200, |x, _| {
            if x < 100 {
                Rgba([220, 40, 30, 255])
            } else {
                Rgba([30, 80, 220, 255])
            }
        }));
        let mut tuning = Tuning {
            image_resolution: 200,
            min_size: 0.001,
            ..Default::default()
        };
        let (_, mut samples, radius, _) =
            prepare(&source, &tuning, &AtomicBool::new(false)).unwrap();
        let mut larger_min_size = samples.clone();
        let regions = features(&mut samples, radius, &tuning);
        tuning.min_size = 0.5;
        assert_eq!(features(&mut larger_min_size, radius, &tuning), regions);
        assert!(
            samples
                .iter()
                .zip(&larger_min_size)
                .all(|(a, b)| a.edge == b.edge && a.region == b.region)
        );
    }
}
