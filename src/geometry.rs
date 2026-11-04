use glam::{Mat3, Vec2, Vec3};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}
impl Bounds {
    pub fn empty() -> Self {
        Self {
            min: Vec3::splat(f32::INFINITY),
            max: Vec3::splat(f32::NEG_INFINITY),
        }
    }
    pub fn add(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    pub fn size(self) -> Vec3 {
        self.max - self.min
    }
    pub fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    pub fn contains(self, p: Vec3) -> bool {
        p.cmpge(self.min - Vec3::splat(1e-5)).all() && p.cmple(self.max + Vec3::splat(1e-5)).all()
    }
    pub fn distance2(self, p: Vec3) -> f32 {
        (p - p.clamp(self.min, self.max)).length_squared()
    }
    pub fn intersects(self, b: Self) -> bool {
        self.min.cmple(b.max).all() && self.max.cmpge(b.min).all()
    }
    fn ray(self, p: Vec3, inv: Vec3) -> bool {
        let a = (self.min - p) * inv;
        let b = (self.max - p) * inv;
        a.min(b).max_element().max(0.) <= a.max(b).min_element()
    }
}

#[derive(Clone, Debug)]
pub struct Triangle {
    pub p: [Vec3; 3],
    pub colors: [Vec3; 3],
    pub uv: [Vec2; 3],
    pub texture: Option<usize>,
}
impl Triangle {
    pub fn normal(&self) -> Vec3 {
        (self.p[1] - self.p[0])
            .cross(self.p[2] - self.p[0])
            .normalize_or_zero()
    }
    pub fn area(&self) -> f32 {
        (self.p[1] - self.p[0])
            .cross(self.p[2] - self.p[0])
            .length()
            * 0.5
    }
    pub fn bounds(&self) -> Bounds {
        let mut b = Bounds::empty();
        for p in self.p {
            b.add(p)
        }
        b
    }
    pub fn ray(&self, origin: Vec3, dir: Vec3) -> Option<f32> {
        let e1 = self.p[1] - self.p[0];
        let e2 = self.p[2] - self.p[0];
        let h = dir.cross(e2);
        let det = e1.dot(h);
        if det.abs() < 1e-8 {
            return None;
        }
        let s = origin - self.p[0];
        let u = s.dot(h) / det;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = s.cross(e1);
        let v = dir.dot(q) / det;
        if v < 0. || u + v > 1. {
            return None;
        }
        let t = e2.dot(q) / det;
        (t > 1e-6).then_some(t)
    }
    /// Closest-point barycentrics, including edge and vertex Voronoi regions.
    pub fn closest(&self, p: Vec3) -> (Vec3, Vec3) {
        let [a, b, c] = self.p;
        let ab = b - a;
        let ac = c - a;
        let ap = p - a;
        let d1 = ab.dot(ap);
        let d2 = ac.dot(ap);
        if d1 <= 0. && d2 <= 0. {
            return (a, Vec3::X);
        }
        let bp = p - b;
        let d3 = ab.dot(bp);
        let d4 = ac.dot(bp);
        if d3 >= 0. && d4 <= d3 {
            return (b, Vec3::Y);
        }
        let vc = d1 * d4 - d3 * d2;
        if vc <= 0. && d1 >= 0. && d3 <= 0. {
            let v = d1 / (d1 - d3);
            return (a + v * ab, Vec3::new(1. - v, v, 0.));
        }
        let cp = p - c;
        let d5 = ab.dot(cp);
        let d6 = ac.dot(cp);
        if d6 >= 0. && d5 <= d6 {
            return (c, Vec3::Z);
        }
        let vb = d5 * d2 - d1 * d6;
        if vb <= 0. && d2 >= 0. && d6 <= 0. {
            let w = d2 / (d2 - d6);
            return (a + w * ac, Vec3::new(1. - w, 0., w));
        }
        let va = d3 * d6 - d5 * d4;
        if va <= 0. && d4 - d3 >= 0. && d5 - d6 >= 0. {
            let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
            return (b + w * (c - b), Vec3::new(0., 1. - w, w));
        }
        let inv = 1. / (va + vb + vc);
        let v = vb * inv;
        let w = vc * inv;
        (a + ab * v + ac * w, Vec3::new(1. - v - w, v, w))
    }
}

#[derive(Clone)]
enum Node {
    Leaf(Bounds, Vec<usize>),
    Branch(Bounds, Box<Node>, Box<Node>),
}
#[derive(Clone)]
pub struct Bvh {
    root: Node,
}
impl Bvh {
    pub fn new(tris: &[Triangle]) -> Self {
        fn build(t: &[Triangle], mut ids: Vec<usize>) -> Node {
            let mut b = Bounds::empty();
            for &i in &ids {
                for p in t[i].p {
                    b.add(p)
                }
            }
            if ids.len() <= 8 {
                return Node::Leaf(b, ids);
            }
            let s = b.size();
            let axis = if s.x >= s.y && s.x >= s.z {
                0
            } else if s.y >= s.z {
                1
            } else {
                2
            };
            ids.sort_by(|&a, &b| {
                t[a].bounds().center()[axis].total_cmp(&t[b].bounds().center()[axis])
            });
            let right = ids.split_off(ids.len() / 2);
            Node::Branch(b, Box::new(build(t, ids)), Box::new(build(t, right)))
        }
        Self {
            root: build(tris, (0..tris.len()).collect()),
        }
    }
    pub fn nearest(&self, tris: &[Triangle], p: Vec3) -> (usize, Vec3, f32) {
        fn visit(n: &Node, t: &[Triangle], p: Vec3, best: &mut (usize, Vec3, f32)) {
            let b = match n {
                Node::Leaf(b, _) | Node::Branch(b, _, _) => b,
            };
            if b.distance2(p) > best.2 {
                return;
            }
            match n {
                Node::Leaf(_, ids) => {
                    for &i in ids {
                        let (q, w) = t[i].closest(p);
                        let d = q.distance_squared(p);
                        if d < best.2 {
                            *best = (i, w, d)
                        }
                    }
                }
                Node::Branch(_, a, b) => {
                    visit(a, t, p, best);
                    visit(b, t, p, best)
                }
            }
        }
        let mut best = (0, Vec3::X, f32::INFINITY);
        visit(&self.root, tris, p, &mut best);
        best
    }
    pub fn inside(&self, tris: &[Triangle], p: Vec3) -> bool {
        let dir = Vec3::new(1., 0.37139, 0.52917).normalize();
        let inv = dir.recip();
        let mut hits = Vec::new();
        fn visit(n: &Node, t: &[Triangle], p: Vec3, d: Vec3, inv: Vec3, hits: &mut Vec<f32>) {
            let b = match n {
                Node::Leaf(b, _) | Node::Branch(b, _, _) => b,
            };
            if !b.ray(p, inv) {
                return;
            }
            match n {
                Node::Leaf(_, ids) => {
                    for &i in ids {
                        if let Some(h) = t[i].ray(p, d) {
                            hits.push(h)
                        }
                    }
                }
                Node::Branch(_, a, b) => {
                    visit(a, t, p, d, inv, hits);
                    visit(b, t, p, d, inv, hits)
                }
            }
        }
        visit(&self.root, tris, p, dir, inv, &mut hits);
        hits.sort_by(f32::total_cmp);
        hits.dedup_by(|a, b| (*a - *b).abs() < 1e-5);
        hits.len() % 2 == 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shape {
    Block,
    Ball,
    Cylinder,
    Wedge,
    CornerWedge,
    CornerTriangle,
    InvertedCornerTriangle,
    Quad,
}
impl Shape {
    pub const ALL: [Self; 8] = [
        Self::Block,
        Self::Ball,
        Self::Cylinder,
        Self::Wedge,
        Self::CornerWedge,
        Self::CornerTriangle,
        Self::InvertedCornerTriangle,
        Self::Quad,
    ];
    pub fn mesh_id(self) -> Option<&'static str> {
        match self {
            Self::CornerTriangle => Some("5623726092"),
            Self::InvertedCornerTriangle => Some("11180908923"),
            Self::Quad => Some("15721480240"),
            _ => None,
        }
    }
    pub fn class(self) -> &'static str {
        match self {
            Self::Wedge => "WedgePart",
            Self::CornerWedge => "CornerWedgePart",
            s if s.mesh_id().is_some() => "MeshPart",
            _ => "Part",
        }
    }
    /// Local coordinates are centered, with each bound at +/- 0.5.
    pub fn contains(self, p: Vec3) -> bool {
        if p.abs().max_element() > 0.50001 {
            return false;
        }
        match self {
            Self::Block | Self::Quad => true,
            Self::Ball => p.length_squared() <= 0.25001,
            Self::Cylinder => p.y * p.y + p.z * p.z <= 0.25001,
            Self::Wedge => p.y <= p.z + 1e-5,
            Self::CornerWedge => p.y <= p.z + 1e-5 && p.y <= -p.x + 1e-5,
            Self::CornerTriangle => p.x - p.y + p.z >= 0.5 - 1e-5,
            Self::InvertedCornerTriangle => p.x - p.y + p.z >= -0.5 - 1e-5,
        }
    }
    pub fn triangles(self) -> Vec<[Vec3; 3]> {
        if self == Self::Quad {
            let a = Vec3::new(-0.5, 0., -0.5);
            let b = Vec3::new(0.5, 0., -0.5);
            let c = Vec3::new(0.5, 0., 0.5);
            let d = Vec3::new(-0.5, 0., 0.5);
            return vec![[a, c, b], [a, d, c]];
        }
        if matches!(self, Self::Ball | Self::Cylinder) {
            let mut out = Vec::new();
            let n = 20;
            if self == Self::Cylinder {
                for i in 0..n {
                    let a = i as f32 * std::f32::consts::TAU / n as f32;
                    let b = (i + 1) as f32 * std::f32::consts::TAU / n as f32;
                    let p = Vec3::new(-0.5, a.cos() * 0.5, a.sin() * 0.5);
                    let q = Vec3::new(-0.5, b.cos() * 0.5, b.sin() * 0.5);
                    out.extend([
                        [p, q, p + Vec3::X],
                        [q, q + Vec3::X, p + Vec3::X],
                        [Vec3::NEG_X * 0.5, q, p],
                        [Vec3::X * 0.5, p + Vec3::X, q + Vec3::X],
                    ]);
                }
            } else {
                let point = |i: usize, j: usize| {
                    let a = i as f32 * std::f32::consts::TAU / n as f32;
                    let b = j as f32 * std::f32::consts::PI / 10.;
                    Vec3::new(a.cos() * b.sin(), b.cos(), a.sin() * b.sin()) * 0.5
                };
                for i in 0..n {
                    for j in 0..10 {
                        let a = point(i, j);
                        let b = point(i + 1, j);
                        let c = point(i + 1, j + 1);
                        let d = point(i, j + 1);
                        out.extend([[a, b, c], [a, c, d]]);
                    }
                }
            }
            return out;
        }
        let v: Vec<Vec3> = match self {
            Self::Wedge => vec![
                [-1., -1., -1.],
                [1., -1., -1.],
                [-1., -1., 1.],
                [1., -1., 1.],
                [-1., 1., 1.],
                [1., 1., 1.],
            ],
            Self::CornerWedge => vec![
                [-1., -1., -1.],
                [1., -1., -1.],
                [-1., -1., 1.],
                [1., -1., 1.],
                [-1., 1., 1.],
            ],
            Self::CornerTriangle => {
                vec![[1., -1., 1.], [-1., -1., 1.], [1., 1., 1.], [1., -1., -1.]]
            }
            Self::InvertedCornerTriangle => vec![
                [-1., -1., -1.],
                [1., -1., -1.],
                [-1., -1., 1.],
                [1., -1., 1.],
                [-1., 1., 1.],
                [1., 1., -1.],
                [1., 1., 1.],
            ],
            _ => vec![
                [-1., -1., -1.],
                [1., -1., -1.],
                [-1., -1., 1.],
                [1., -1., 1.],
                [-1., 1., -1.],
                [1., 1., -1.],
                [-1., 1., 1.],
                [1., 1., 1.],
            ],
        }
        .into_iter()
        .map(|p| Vec3::from_array(p) * 0.5)
        .collect();
        // Enumerate convex supporting planes, then triangulate each planar polygon once.
        let mut faces: Vec<(Vec3, f32, Vec<usize>)> = Vec::new();
        for i in 0..v.len() {
            for j in i + 1..v.len() {
                for k in j + 1..v.len() {
                    let mut n = (v[j] - v[i]).cross(v[k] - v[i]).normalize_or_zero();
                    if n == Vec3::ZERO {
                        continue;
                    }
                    let mut d = n.dot(v[i]);
                    let max = v
                        .iter()
                        .map(|p| n.dot(*p) - d)
                        .fold(f32::NEG_INFINITY, f32::max);
                    let min = v
                        .iter()
                        .map(|p| n.dot(*p) - d)
                        .fold(f32::INFINITY, f32::min);
                    if max > 1e-5 && min < -1e-5 {
                        continue;
                    }
                    if max > 1e-5 {
                        n = -n;
                        d = -d
                    }
                    if faces
                        .iter()
                        .any(|(m, e, _)| m.dot(n) > 0.9999 && (*e - d).abs() < 1e-5)
                    {
                        continue;
                    }
                    let ids = (0..v.len())
                        .filter(|&q| (n.dot(v[q]) - d).abs() < 1e-5)
                        .collect();
                    faces.push((n, d, ids));
                }
            }
        }
        let mut out = Vec::new();
        for (n, _, mut ids) in faces {
            let c = ids.iter().map(|&i| v[i]).sum::<Vec3>() / ids.len() as f32;
            let u = (v[ids[0]] - c).normalize();
            let w = n.cross(u);
            ids.sort_by(|&a, &b| {
                (v[a] - c)
                    .dot(w)
                    .atan2((v[a] - c).dot(u))
                    .total_cmp(&(v[b] - c).dot(w).atan2((v[b] - c).dot(u)))
            });
            for j in 1..ids.len() - 1 {
                out.push([v[ids[0]], v[ids[j]], v[ids[j + 1]]])
            }
        }
        out
    }
}

pub fn halton(mut index: usize, base: usize) -> f32 {
    let mut f = 1.;
    let mut r = 0.;
    while index > 0 {
        f /= base as f32;
        r += f * (index % base) as f32;
        index /= base
    }
    r
}
pub fn rotations() -> Vec<Mat3> {
    let axes = [
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        Vec3::NEG_X,
        Vec3::NEG_Y,
        Vec3::NEG_Z,
    ];
    let mut out = vec![Mat3::IDENTITY];
    for x in axes {
        for y in axes {
            if x.dot(y).abs() < 0.1 {
                let m = Mat3::from_cols(x, y, x.cross(y));
                if m != Mat3::IDENTITY {
                    out.push(m)
                }
            }
        }
    }
    out
}

/// Covariance principal frame; images retain their XY plane.
pub fn principal_frame(points: &[Vec3], flat: bool) -> Mat3 {
    if points.len() < 3 {
        return Mat3::IDENTITY;
    }
    let c = points.iter().sum::<Vec3>() / points.len() as f32;
    let mut cov = Mat3::ZERO;
    for p in points {
        let d = *p - c;
        cov += Mat3::from_cols(d * d.x, d * d.y, d * d.z)
    }
    let mut x = Vec3::new(1., 0.57, if flat { 0. } else { 0.31 }).normalize();
    for _ in 0..24 {
        x = (cov * x).normalize_or_zero()
    }
    if x.length_squared() < 0.9 {
        return Mat3::IDENTITY;
    }
    if flat {
        return Mat3::from_cols(x, Vec3::Z.cross(x), Vec3::Z);
    }
    let mut y = x.any_orthonormal_vector();
    for _ in 0..24 {
        let q = cov * y;
        y = (q - x * x.dot(q)).normalize_or_zero()
    }
    if y.length_squared() < 0.9 {
        y = x.any_orthonormal_vector()
    }
    let z = x.cross(y).normalize();
    Mat3::from_cols(x, z.cross(x), z)
}
