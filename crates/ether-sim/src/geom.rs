//! Closest points between a segment and a triangle, and a ray against a
//! triangle: what a capsule needs to collide with a triangle soup.
//!
//! A capsule is a segment with a radius. It touches a triangle when the
//! distance between the segment and the triangle is below the radius. That
//! distance is reached either where the segment crosses the triangle (0), or
//! between an endpoint of the segment and the triangle, or between an edge of
//! the triangle and the segment; [`segment_triangle`] takes the least of those
//! six candidates.

use glam::Vec3;

/// The point of triangle `abc` closest to `p` (Ericson, *Real-Time Collision Detection* 5.1.5).
pub fn point_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return a + ab * v;
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return a + ac * w;
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return b + (c - b) * w;
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    a + ab * v + ac * w
}

/// Closest points of segments `p1q1` and `p2q2` (Ericson 5.1.9).
pub fn segment_segment(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> (Vec3, Vec3) {
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    const EPS: f32 = 1e-12;
    let (s, t);
    if a <= EPS && e <= EPS {
        return (p1, p2);
    }
    if a <= EPS {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= EPS {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s0 = if denom != 0.0 { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
            let mut t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t0 = 0.0;
                s0 = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t0 = 1.0;
                s0 = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = s0;
            t = t0;
        }
    }
    (p1 + d1 * s, p2 + d2 * t)
}

/// Where segment `pq` crosses triangle `abc`, as the parameter along the segment, either side.
pub fn segment_crosses(p: Vec3, q: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let t = ray_triangle(p, q - p, a, b, c)?;
    (t <= 1.0).then_some(t)
}

/// Möller–Trumbore, two-sided: the parameter `t >= 0` at which `origin + t·dir` meets the triangle.
pub fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let pv = dir.cross(e2);
    let det = e1.dot(pv);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let tv = origin - a;
    let u = tv.dot(pv) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qv = tv.cross(e1);
    let v = dir.dot(qv) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(qv) * inv;
    (t >= 0.0).then_some(t)
}

/// Closest points between segment `pq` and triangle `abc`: (point on the segment, point on the triangle). They
/// are equal where the segment crosses the triangle.
pub fn segment_triangle(p: Vec3, q: Vec3, tri: &[Vec3; 3]) -> (Vec3, Vec3) {
    let [a, b, c] = *tri;
    if let Some(t) = segment_crosses(p, q, a, b, c) {
        let x = p + (q - p) * t;
        return (x, x);
    }
    let mut best = (p, point_triangle(p, a, b, c));
    let mut best_d = best.0.distance_squared(best.1);
    let mut consider = |s: Vec3, t: Vec3| {
        let d = s.distance_squared(t);
        if d < best_d {
            best_d = d;
            best = (s, t);
        }
    };
    consider(q, point_triangle(q, a, b, c));
    for (e0, e1) in [(a, b), (b, c), (c, a)] {
        let (s, t) = segment_segment(p, q, e0, e1);
        consider(s, t);
    }
    best
}

/// The triangle's unit normal by its winding, or zero for a degenerate one.
pub fn face_normal(tri: &[Vec3; 3]) -> Vec3 {
    (tri[1] - tri[0]).cross(tri[2] - tri[0]).normalize_or_zero()
}

/// Axis-aligned bounds of a triangle.
#[inline]
pub fn tri_bounds(tri: &[Vec3; 3]) -> (Vec3, Vec3) {
    (tri[0].min(tri[1]).min(tri[2]), tri[0].max(tri[1]).max(tri[2]))
}

#[inline]
pub fn overlaps(amin: Vec3, amax: Vec3, bmin: Vec3, bmax: Vec3) -> bool {
    amin.x <= bmax.x && amax.x >= bmin.x && amin.y <= bmax.y && amax.y >= bmin.y && amin.z <= bmax.z && amax.z >= bmin.z
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec3;

    #[test]
    fn segment_over_a_triangle() {
        let tri = [vec3(0., 0., 0.), vec3(4., 0., 0.), vec3(0., 0., 4.)];
        let (s, t) = segment_triangle(vec3(1., 2., 1.), vec3(1., 5., 1.), &tri);
        assert!((s - vec3(1., 2., 1.)).length() < 1e-6 && (t - vec3(1., 0., 1.)).length() < 1e-6);
        let (s, t) = segment_triangle(vec3(1., -1., 1.), vec3(1., 1., 1.), &tri);
        assert!(s.distance(t) < 1e-6);
        // Beside the hypotenuse: closest to an edge.
        let (s, t) = segment_triangle(vec3(3., 1., 3.), vec3(3., -1., 3.), &tri);
        assert!((t - vec3(2., 0., 2.)).length() < 1e-5, "{t}");
        assert!((s - vec3(3., 0., 3.)).length() < 1e-5, "{s}");
    }

    #[test]
    fn ray_hits_both_sides() {
        let tri = [vec3(0., 0., 0.), vec3(4., 0., 0.), vec3(0., 0., 4.)];
        assert_eq!(ray_triangle(vec3(1., 3., 1.), vec3(0., -1., 0.), tri[0], tri[1], tri[2]), Some(3.0));
        assert_eq!(ray_triangle(vec3(1., -3., 1.), vec3(0., 1., 0.), tri[0], tri[1], tri[2]), Some(3.0));
        assert_eq!(ray_triangle(vec3(5., 3., 5.), vec3(0., -1., 0.), tri[0], tri[1], tri[2]), None);
    }
}
