//! Splitting joined speech balloons into lobes.
//!
//! A bubble mask often holds several balloons fused together: a stack of
//! overlapping boxes, or a balloon with hand-lettered moan bubbles attached.
//! Their seams run between two concave corners of the outline. The seam
//! search here is ported from upstream Koharu's `koharu-renderer/src/bubble.rs`
//! (mayocream/koharu, MIT OR Apache-2.0): candidate cuts join two reflex
//! vertices of the simplified outline; a cut is structural when its corners
//! are sharp and recessed from the convex hull, relative to its length.

type Point = (f32, f32);
type Polygon = Vec<Point>;

/// Lobe cells for `anchors` (text box centres) inside `polygon`: one polygon
/// per anchor, or `None` where no seam separates it from the others.
pub(crate) fn lobe_cells(
    polygon: &[Point],
    anchors: &[Point],
    tolerance: f32,
) -> Option<Vec<Polygon>> {
    let indexed = anchors.iter().copied().enumerate().collect::<Vec<_>>();
    let mut cells = decompose_lobes(polygon.to_vec(), indexed, tolerance)?;
    cells.sort_by_key(|(index, _)| *index);
    (cells.len() == anchors.len()).then(|| cells.into_iter().map(|(_, cell)| cell).collect())
}

/// A lobe needs at least this share of the outline's area to be cut off.
const TRIM_MIN_SHARE: f32 = 0.08;
/// A seam longer than the smaller side's size (square root of its area) is
/// part of the balloon's shape, not a join.
const TRIM_MAX_SEAM: f32 = 1.0;

/// Removes lobes holding no text from the one that holds `anchor_box`
/// (`left, top, right, bottom`): whenever a structural seam leaves the box
/// wholly on one side and a sizeable lobe joined by a short seam on the
/// other, the far lobe is cut away. Returns the text's own lobe.
pub(crate) fn trim_empty_lobes(
    polygon: &[Point],
    anchor_box: (f32, f32, f32, f32),
    tolerance: f32,
) -> Polygon {
    let mut current = polygon.to_vec();
    for _ in 0..4 {
        let total = polygon_area(&current).abs();
        let Some((_, reflex)) = reflex_vertices(&current, tolerance) else {
            break;
        };
        let mut best: Option<(f32, Polygon)> = None;
        for a in 0..reflex.len() {
            for b in a + 1..reflex.len() {
                let (first_vertex, first_support) = reflex[a];
                let (second_vertex, second_support) = reflex[b];
                if !valid_diagonal(&current, first_vertex, second_vertex, tolerance) {
                    continue;
                }
                let (first, second) = split_polygon(&current, first_vertex, second_vertex);
                if first.len() < 3 || second.len() < 3 {
                    continue;
                }
                let inside = |cell: &[Point]| box_inside(cell, anchor_box, tolerance);
                let (near, far) = match (inside(&first), inside(&second)) {
                    (true, false) => (first, second),
                    (false, true) => (second, first),
                    _ => continue,
                };
                let far_area = polygon_area(&far).abs();
                let near_area = polygon_area(&near).abs();
                if far_area < TRIM_MIN_SHARE * total {
                    continue;
                }
                let seam = distance_squared(current[first_vertex], current[second_vertex]).sqrt();
                if seam > TRIM_MAX_SEAM * far_area.min(near_area).sqrt() {
                    continue;
                }
                let support = first_support.min(second_support) / seam.max(f32::EPSILON);
                if best.as_ref().is_none_or(|(s, _)| support > *s) {
                    best = Some((support, near));
                }
            }
        }
        match best {
            Some((_, near)) => current = near,
            None => break,
        }
    }
    current
}

/// Whether most of the box lies inside `polygon` (sampled on a grid).
fn box_inside(
    polygon: &[Point],
    (left, top, right, bottom): (f32, f32, f32, f32),
    tolerance: f32,
) -> bool {
    const GRID: usize = 6;
    let epsilon = (tolerance * tolerance * 0.001).max(f32::EPSILON);
    let mut inside = 0;
    for i in 0..GRID {
        for j in 0..GRID {
            let x = left + (right - left) * (i as f32 + 0.5) / GRID as f32;
            let y = top + (bottom - top) * (j as f32 + 0.5) / GRID as f32;
            if point_in_polygon(polygon, (x, y), epsilon) {
                inside += 1;
            }
        }
    }
    inside as f32 >= 0.85 * (GRID * GRID) as f32
}

/// Reflex (concave) vertices of the simplified outline with their support:
/// corner sharpness times recession from the convex hull.
#[allow(clippy::type_complexity)]
fn reflex_vertices(polygon: &[Point], tolerance: f32) -> Option<(Vec<usize>, Vec<(usize, f32)>)> {
    let simplified = simplify_closed_indices(polygon, tolerance);
    if simplified.len() < 4 {
        return None;
    }
    let orientation = polygon_area_from_indices(polygon, &simplified).signum();
    if orientation == 0.0 {
        return None;
    }
    let hull = convex_hull(
        &simplified
            .iter()
            .map(|&index| polygon[index])
            .collect::<Vec<_>>(),
    );
    let minimum_reflex_cross = tolerance * tolerance * 0.05;
    let reflex = (0..simplified.len())
        .filter_map(|index| {
            let previous = polygon[simplified[(index + simplified.len() - 1) % simplified.len()]];
            let current = polygon[simplified[index]];
            let next = polygon[simplified[(index + 1) % simplified.len()]];
            let cross = turn(previous, current, next) * orientation;
            if cross >= -minimum_reflex_cross {
                return None;
            }
            let edge_product =
                (distance_squared(previous, current) * distance_squared(current, next)).sqrt();
            (edge_product > f32::EPSILON).then(|| {
                let corner_strength = -cross / edge_product;
                let recession = distance_to_polygon_boundary(current, &hull);
                (simplified[index], corner_strength * recession)
            })
        })
        .collect::<Vec<_>>();
    Some((simplified, reflex))
}

struct LobeSplit {
    structural_support: f32,
    length_squared: f32,
    first: Polygon,
    second: Polygon,
    first_anchors: Vec<(usize, Point)>,
    second_anchors: Vec<(usize, Point)>,
}

fn decompose_lobes(
    polygon: Polygon,
    anchors: Vec<(usize, Point)>,
    tolerance: f32,
) -> Option<Vec<(usize, Polygon)>> {
    if anchors.len() == 1 {
        return Some(vec![(anchors[0].0, polygon)]);
    }
    let (_, reflex) = reflex_vertices(&polygon, tolerance)?;
    let mut candidates = Vec::new();
    for first_index in 0..reflex.len() {
        for second_index in first_index + 1..reflex.len() {
            let (first_vertex, first_support) = reflex[first_index];
            let (second_vertex, second_support) = reflex[second_index];
            if !valid_diagonal(&polygon, first_vertex, second_vertex, tolerance) {
                continue;
            }
            let (first, second) = split_polygon(&polygon, first_vertex, second_vertex);
            if first.len() < 3
                || second.len() < 3
                || polygon_area(&first).abs() <= tolerance * tolerance
                || polygon_area(&second).abs() <= tolerance * tolerance
            {
                continue;
            }
            let mut first_anchors = Vec::new();
            let mut second_anchors = Vec::new();
            let mut assigns_cleanly = true;
            let cross_epsilon = (tolerance * tolerance * 0.001).max(f32::EPSILON);
            for &(index, anchor) in &anchors {
                match (
                    point_in_polygon(&first, anchor, cross_epsilon),
                    point_in_polygon(&second, anchor, cross_epsilon),
                ) {
                    (true, false) => first_anchors.push((index, anchor)),
                    (false, true) => second_anchors.push((index, anchor)),
                    _ => {
                        assigns_cleanly = false;
                        break;
                    }
                }
            }
            if !assigns_cleanly || first_anchors.is_empty() || second_anchors.is_empty() {
                continue;
            }
            let length_squared = distance_squared(polygon[first_vertex], polygon[second_vertex]);
            if length_squared <= f32::EPSILON {
                continue;
            }
            candidates.push(LobeSplit {
                structural_support: first_support.min(second_support) / length_squared.sqrt(),
                length_squared,
                first,
                second,
                first_anchors,
                second_anchors,
            });
        }
    }
    candidates.sort_by(|left, right| {
        right
            .structural_support
            .total_cmp(&left.structural_support)
            .then_with(|| left.length_squared.total_cmp(&right.length_squared))
    });
    for candidate in candidates {
        let Some(mut first) = decompose_lobes(candidate.first, candidate.first_anchors, tolerance)
        else {
            continue;
        };
        let Some(second) = decompose_lobes(candidate.second, candidate.second_anchors, tolerance)
        else {
            continue;
        };
        first.extend(second);
        return Some(first);
    }
    None
}

fn convex_hull(points: &[Point]) -> Polygon {
    let mut points = points.to_vec();
    points.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.total_cmp(&right.1))
    });
    points.dedup();
    if points.len() <= 2 {
        return points;
    }
    let mut lower: Polygon = Vec::new();
    for &point in &points {
        while lower.len() >= 2 && turn(lower[lower.len() - 2], lower[lower.len() - 1], point) <= 0.0
        {
            lower.pop();
        }
        lower.push(point);
    }
    let mut upper: Polygon = Vec::new();
    for &point in points.iter().rev() {
        while upper.len() >= 2 && turn(upper[upper.len() - 2], upper[upper.len() - 1], point) <= 0.0
        {
            upper.pop();
        }
        upper.push(point);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn distance_to_polygon_boundary(point: Point, polygon: &[Point]) -> f32 {
    if polygon.len() < 2 {
        return 0.0;
    }
    (0..polygon.len())
        .map(|index| {
            point_segment_distance(point, polygon[index], polygon[(index + 1) % polygon.len()])
        })
        .fold(f32::INFINITY, f32::min)
}

fn simplify_closed_indices(polygon: &[Point], tolerance: f32) -> Vec<usize> {
    let split = (1..polygon.len())
        .max_by(|&left, &right| {
            distance_squared(polygon[0], polygon[left])
                .total_cmp(&distance_squared(polygon[0], polygon[right]))
        })
        .unwrap_or(0);
    if split == 0 {
        return (0..polygon.len()).collect();
    }
    let first_chain = (0..=split).collect::<Vec<_>>();
    let second_chain = (split..polygon.len())
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut first = simplify_chain_indices(polygon, &first_chain, tolerance);
    let mut second = simplify_chain_indices(polygon, &second_chain, tolerance);
    first.pop();
    second.pop();
    first.extend(second);
    if first.len() >= 3 {
        first
    } else {
        (0..polygon.len()).collect()
    }
}

fn simplify_chain_indices(polygon: &[Point], chain: &[usize], tolerance: f32) -> Vec<usize> {
    if chain.len() <= 2 {
        return chain.to_vec();
    }
    let start = polygon[chain[0]];
    let end = polygon[*chain.last().unwrap()];
    let mut farthest = None;
    for (position, &index) in chain.iter().enumerate().skip(1).take(chain.len() - 2) {
        let distance = point_segment_distance(polygon[index], start, end);
        if farthest.is_none_or(|(_, best)| distance > best) {
            farthest = Some((position, distance));
        }
    }
    let Some((position, distance)) = farthest else {
        return vec![chain[0], *chain.last().unwrap()];
    };
    if distance <= tolerance {
        return vec![chain[0], *chain.last().unwrap()];
    }
    let mut first = simplify_chain_indices(polygon, &chain[..=position], tolerance);
    let second = simplify_chain_indices(polygon, &chain[position..], tolerance);
    first.pop();
    first.extend(second);
    first
}

fn valid_diagonal(polygon: &[Point], first: usize, second: usize, tolerance: f32) -> bool {
    let len = polygon.len();
    if first == second || (first + 1) % len == second || (second + 1) % len == first {
        return false;
    }
    let start = polygon[first];
    let end = polygon[second];
    let epsilon = (tolerance * tolerance * 0.001).max(f32::EPSILON);
    for edge in 0..len {
        let next = (edge + 1) % len;
        if edge == first || edge == second || next == first || next == second {
            continue;
        }
        if segments_intersect(start, end, polygon[edge], polygon[next], epsilon) {
            return false;
        }
    }
    [0.2, 0.5, 0.8].into_iter().all(|fraction| {
        point_in_polygon(
            polygon,
            (
                start.0 + (end.0 - start.0) * fraction,
                start.1 + (end.1 - start.1) * fraction,
            ),
            epsilon,
        )
    })
}

fn split_polygon(polygon: &[Point], first: usize, second: usize) -> (Polygon, Polygon) {
    let (first, second) = if first <= second {
        (first, second)
    } else {
        (second, first)
    };
    let first_part = polygon[first..=second].to_vec();
    let second_part = polygon[second..]
        .iter()
        .chain(&polygon[..=first])
        .copied()
        .collect();
    (first_part, second_part)
}

fn polygon_area(polygon: &[Point]) -> f32 {
    if polygon.len() < 3 {
        return 0.0;
    }
    (0..polygon.len())
        .map(|index| {
            let first = polygon[index];
            let second = polygon[(index + 1) % polygon.len()];
            first.0 * second.1 - second.0 * first.1
        })
        .sum::<f32>()
        * 0.5
}

fn polygon_area_from_indices(polygon: &[Point], indices: &[usize]) -> f32 {
    (0..indices.len())
        .map(|index| {
            let first = polygon[indices[index]];
            let second = polygon[indices[(index + 1) % indices.len()]];
            first.0 * second.1 - second.0 * first.1
        })
        .sum::<f32>()
        * 0.5
}

pub(crate) fn point_in_polygon(polygon: &[Point], point: Point, epsilon: f32) -> bool {
    let mut inside = false;
    for index in 0..polygon.len() {
        let first = polygon[index];
        let second = polygon[(index + 1) % polygon.len()];
        if point_on_segment(first, second, point, epsilon) {
            return true;
        }
        if (first.1 > point.1) != (second.1 > point.1) {
            let crossing =
                (second.0 - first.0) * (point.1 - first.1) / (second.1 - first.1) + first.0;
            if point.0 < crossing {
                inside = !inside;
            }
        }
    }
    inside
}

fn segments_intersect(
    first_start: Point,
    first_end: Point,
    second_start: Point,
    second_end: Point,
    epsilon: f32,
) -> bool {
    let first_side_start = orientation(first_start, first_end, second_start);
    let first_side_end = orientation(first_start, first_end, second_end);
    let second_side_start = orientation(second_start, second_end, first_start);
    let second_side_end = orientation(second_start, second_end, first_end);
    let crosses = ((first_side_start > epsilon && first_side_end < -epsilon)
        || (first_side_start < -epsilon && first_side_end > epsilon))
        && ((second_side_start > epsilon && second_side_end < -epsilon)
            || (second_side_start < -epsilon && second_side_end > epsilon));
    crosses
        || point_on_segment(first_start, first_end, second_start, epsilon)
        || point_on_segment(first_start, first_end, second_end, epsilon)
        || point_on_segment(second_start, second_end, first_start, epsilon)
        || point_on_segment(second_start, second_end, first_end, epsilon)
}

fn point_on_segment(start: Point, end: Point, point: Point, epsilon: f32) -> bool {
    orientation(start, end, point).abs() <= epsilon
        && point.0 >= start.0.min(end.0) - epsilon
        && point.0 <= start.0.max(end.0) + epsilon
        && point.1 >= start.1.min(end.1) - epsilon
        && point.1 <= start.1.max(end.1) + epsilon
}

fn orientation(first: Point, second: Point, third: Point) -> f32 {
    (second.0 - first.0) * (third.1 - first.1) - (second.1 - first.1) * (third.0 - first.0)
}

fn turn(previous: Point, current: Point, next: Point) -> f32 {
    (current.0 - previous.0) * (next.1 - current.1)
        - (current.1 - previous.1) * (next.0 - current.0)
}

fn point_segment_distance(point: Point, start: Point, end: Point) -> f32 {
    let segment = (end.0 - start.0, end.1 - start.1);
    let length_squared = segment.0 * segment.0 + segment.1 * segment.1;
    if length_squared <= f32::EPSILON {
        return distance_squared(point, start).sqrt();
    }
    let fraction = (((point.0 - start.0) * segment.0 + (point.1 - start.1) * segment.1)
        / length_squared)
        .clamp(0.0, 1.0);
    distance_squared(
        point,
        (
            start.0 + segment.0 * fraction,
            start.1 + segment.1 * fraction,
        ),
    )
    .sqrt()
}

fn distance_squared(first: Point, second: Point) -> f32 {
    (first.0 - second.0).powi(2) + (first.1 - second.1).powi(2)
}
