//! Convex image volumes in the partition frame, extruded along Z.
use crate::geometry::Bounds;
use glam::Vec2;

pub fn rectangle(bounds: Bounds) -> Vec<Vec2> {
    vec![
        bounds.min.truncate(),
        Vec2::new(bounds.max.x, bounds.min.y),
        bounds.max.truncate(),
        Vec2::new(bounds.min.x, bounds.max.y),
    ]
}

pub fn edges(points: &[Vec2]) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
    points
        .iter()
        .copied()
        .zip(points.iter().copied().cycle().skip(1))
}

pub fn clip(points: &[Vec2], normal: Vec2, offset: f32) -> Vec<Vec2> {
    let mut out = Vec::new();
    for (a, b) in edges(points) {
        let da = normal.dot(a) - offset;
        let db = normal.dot(b) - offset;
        if da <= 0. {
            out.push(a);
        }
        if (da < 0. && db > 0.) || (da > 0. && db < 0.) {
            out.push(a.lerp(b, da / (da - db)));
        }
    }
    out.dedup_by(|a, b| a.distance_squared(*b) < 1e-12);
    if out.len() > 1 && out[0].distance_squared(*out.last().unwrap()) < 1e-12 {
        out.pop();
    }
    out
}

pub fn bounds(points: &[Vec2], mut bounds: Bounds) -> Option<Bounds> {
    if points.len() < 3 {
        return None;
    }
    let min = points
        .iter()
        .copied()
        .fold(Vec2::splat(f32::INFINITY), Vec2::min);
    let max = points
        .iter()
        .copied()
        .fold(Vec2::splat(f32::NEG_INFINITY), Vec2::max);
    if (max - min).min_element() <= 1e-6 {
        return None;
    }
    bounds.min.x = min.x;
    bounds.min.y = min.y;
    bounds.max.x = max.x;
    bounds.max.y = max.y;
    Some(bounds)
}

pub fn shared_edge(a: &[Vec2], b: &[Vec2]) -> Option<(Vec2, Vec2, f32)> {
    for (a0, a1) in edges(a) {
        let length = a0.distance(a1);
        if length <= 1e-6 {
            continue;
        }
        let direction = (a1 - a0) / length;
        let normal = Vec2::new(direction.y, -direction.x);
        for (b0, b1) in edges(b) {
            if direction.dot((b1 - b0).normalize_or_zero()) > -0.99999
                || normal.dot(b0 - a0).abs() > 1e-4
                || normal.dot(b1 - a0).abs() > 1e-4
            {
                continue;
            }
            let t0 = direction.dot(b0 - a0);
            let t1 = direction.dot(b1 - a0);
            let lo = t0.min(t1).max(0.);
            let hi = t0.max(t1).min(length);
            if hi - lo > 1e-5 {
                return Some((a0 + direction * ((lo + hi) * 0.5), normal, hi - lo));
            }
        }
    }
    None
}
