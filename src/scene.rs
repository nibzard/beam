// ABOUTME: Deterministic Beam choreography: a voxel workspace travels between two destinations.
// ABOUTME: Effect calculations are independent of terminal encoding, timing, and input.
use crate::raster::{Canvas, Point, Rgb};

/// The workspace leaves its destination at this progress.
pub const DEPART: f32 = 0.12;

/// Draw the timeline at `progress` (0 to 1). `seconds` turns the scattered cloud, so a long
/// transfer stays in motion while its progress holds still.
pub fn render(canvas: &mut Canvas, width: usize, height: usize, progress: f32, seconds: f32) {
    canvas.clear();
    let t = progress.clamp(0.0, 1.0);
    let travel = smooth((t - DEPART) / (1.0 - 2.0 * DEPART));
    let scatter = (std::f32::consts::PI * travel).sin().powi(2);
    let scale = (width as f32 * 0.025).min(height as f32 * 0.24);
    // Whole dots, so both destination frames rasterize the same.
    let left = (width as f32 * 0.4).round();
    let right = (width as f32 * 1.6).round();
    let cy = height as f32 * 2.0;
    let cx = left + (right - left) * travel;
    let yaw = 0.3 + scatter * 3.2;
    let pitch = -0.18 - scatter * 0.25;
    let color = mix([0, 198, 197], [169, 128, 238], travel);
    let project = |v: [f32; 3], center: f32| {
        let depth = 80.0 + v[2];
        Point {
            x: center + v[0] * scale * 80.0 / depth,
            y: cy + v[1] * scale * 80.0 / depth,
            depth,
        }
    };

    // Destination frames sway around the vertical axis, so their side faces show depth. They do
    // not tilt: a top face only a few dots high shows as steps or as a separate line in braille.
    // They do not move from their places; their geometry gives the movement a clear reference.
    // The frame that holds the formed workspace glows in its color.
    let sway = SWAY * (seconds * 1.2).sin();
    for (center, near) in [(left, 1.0 - travel), (right, travel)] {
        let frame = mix([110, 120, 130], color, 0.8 * near * (1.0 - scatter));
        let corners = corners([0.0; 3], [7.3, 5.5, 1.8]);
        let projected = crisp(
            corners.map(|v| project(rotate(v, sway, 0.0), center)),
            scale * 1.7,
        );
        for [a, b] in EDGES {
            canvas.line(projected[a], projected[b], frame);
        }
    }

    // A folder-shaped workspace: a tab and seven rows of blocks.
    for row in 0..8 {
        for column in 0..12 {
            if row == 0 && column >= 4 {
                continue;
            }
            let seed = (row * 12 + column) as u32;
            // Scatter offsets turn around the vertical axis. They shrink to zero on arrival.
            let offset = rotate(
                [
                    noise(seed * 3) * 12.0,
                    noise(seed * 3 + 1) * 14.0,
                    noise(seed * 3 + 2) * 9.0,
                ],
                seconds * 0.9,
                0.0,
            );
            let center = [
                column as f32 - 5.5 + offset[0] * scatter,
                row as f32 - 3.5 + offset[1] * scatter,
                offset[2] * scatter,
            ];
            let half = 0.46 * (1.0 - scatter * 0.65);
            let vertices = corners(center, [half, half, half]);
            let projected = vertices.map(|v| project(rotate(v, yaw, pitch), cx));
            let point = project(rotate(center, yaw, pitch), cx);
            // The cloud is taller than a short canvas. Near the top and bottom edges it fades out
            // and stops a few dots before them, so no edge cuts it.
            let margin = cy - (point.y - cy).abs() - EDGE;
            if margin <= 0.0 {
                continue;
            }
            let fade = (margin / (cy * 0.3)).min(1.0);
            // In the cloud, nearer blocks are brighter and farther blocks are dimmer.
            let shade = (1.0 + scatter * ((80.0 - point.depth) / 12.0).clamp(-0.6, 0.3))
                * (0.3 + 0.7 * fade);
            // A block smaller than a dot would cover a dot in some frames and none in others.
            // The particle below shows it instead.
            let faces = if half * scale < 0.5 {
                &[][..]
            } else {
                &FACES[..]
            };
            for &(indices, normal) in faces {
                let normal = rotate(normal, yaw, pitch);
                if normal[2] >= 0.0 {
                    continue;
                }
                let light = (0.45
                    + 0.65 * (-normal[0] * 0.4 - normal[1] * 0.5 - normal[2] * 0.7).max(0.0))
                .min(1.0);
                let lit =
                    color.map(|channel| (f32::from(channel) * light * shade).min(255.0) as u8);
                let [a, b, c, d] = indices.map(|index| projected[index]);
                canvas.triangle([a, b, c], lit);
                canvas.triangle([a, c, d], lit);
            }
            // Fine particles fill the gaps as the workspace disperses.
            if scatter > 0.2 {
                canvas.dot(
                    point.x.round() as i32,
                    point.y.round() as i32,
                    point.depth - 0.5,
                    color.map(|channel| (f32::from(channel) * shade).min(255.0) as u8),
                );
            }
        }
    }
}

pub fn stage(progress: f32, down: bool) -> &'static str {
    match progress {
        p if p < DEPART => {
            if down {
                "Workspace in the sandbox"
            } else {
                "Workspace on your machine"
            }
        }
        p if p < 1.0 - DEPART => {
            if down {
                "Bringing the workspace home"
            } else {
                "Moving the workspace to the sandbox"
            }
        }
        _ => {
            if down {
                "Workspace home again"
            } else {
                "Workspace in the sandbox"
            }
        }
    }
}

const SWAY: f32 = 0.6;
/// Dots between the cloud and the top or bottom edge of the canvas.
const EDGE: f32 = 4.0;

/// A nearly horizontal or vertical box edge would show a step where it crosses a dot boundary.
/// Put such an edge on one dot row or column when its ends differ by less than `tolerance` dots;
/// the error is less than half of that. Each corner is
/// on one horizontal edge, which sets its row, and one vertical edge, which sets its column, so
/// the edges still meet at the corners.
fn crisp(mut points: [Point; 8], tolerance: f32) -> [Point; 8] {
    for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7)] {
        if (points[a].y - points[b].y).abs() < tolerance {
            let y = (points[a].y + points[b].y) / 2.0;
            (points[a].y, points[b].y) = (y, y);
        }
    }
    for (a, b) in [(0, 2), (1, 3), (4, 6), (5, 7)] {
        if (points[a].x - points[b].x).abs() < tolerance {
            let x = (points[a].x + points[b].x) / 2.0;
            (points[a].x, points[b].x) = (x, x);
        }
    }
    points
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn noise(seed: u32) -> f32 {
    let mut value = seed.wrapping_add(1).wrapping_mul(0x9e3779b9);
    value ^= value >> 16;
    value = value.wrapping_mul(0x85ebca6b);
    value ^= value >> 13;
    (value & 0xffff) as f32 / 32767.5 - 1.0
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    std::array::from_fn(|i| (f32::from(a[i]) * (1.0 - t) + f32::from(b[i]) * t) as u8)
}

fn rotate([x, y, z]: [f32; 3], yaw: f32, pitch: f32) -> [f32; 3] {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let rx = x * cy + z * sy;
    let rz = z * cy - x * sy;
    [rx, y * cp - rz * sp, y * sp + rz * cp]
}

fn corners(center: [f32; 3], half: [f32; 3]) -> [[f32; 3]; 8] {
    std::array::from_fn(|index| {
        std::array::from_fn(|axis| {
            center[axis] + half[axis] * if index & (1 << axis) == 0 { -1.0 } else { 1.0 }
        })
    })
}

const EDGES: [[usize; 2]; 12] = [
    [0, 1],
    [0, 2],
    [0, 4],
    [1, 3],
    [1, 5],
    [2, 3],
    [2, 6],
    [3, 7],
    [4, 5],
    [4, 6],
    [5, 7],
    [6, 7],
];
const FACES: [([usize; 4], [f32; 3]); 6] = [
    ([0, 2, 6, 4], [-1.0, 0.0, 0.0]),
    ([1, 5, 7, 3], [1.0, 0.0, 0.0]),
    ([0, 4, 5, 1], [0.0, -1.0, 0.0]),
    ([2, 3, 7, 6], [0.0, 1.0, 0.0]),
    ([0, 1, 3, 2], [0.0, 0.0, -1.0]),
    ([4, 6, 7, 5], [0.0, 0.0, 1.0]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearly_level_box_edges_stay_on_one_dot_row_or_column_and_still_meet() {
        // A box seen almost straight on: each edge is a little off level or plumb.
        let mut points = corners([10.0, 10.0, 0.0], [8.0, 6.0, 1.0]).map(|[x, y, z]| Point {
            x,
            y,
            depth: 80.0 + z,
        });
        for (index, point) in points.iter_mut().enumerate() {
            point.x += 0.3 * index as f32 / 7.0;
            point.y += 0.9 * (index & 1) as f32;
        }
        let snapped = crisp(points, 2.0);
        for [a, b] in EDGES {
            let (a, b) = (snapped[a], snapped[b]);
            assert!(a.x == b.x || a.y == b.y || a.depth != b.depth);
        }
        // Real slopes stay as they are.
        points[1].y += 5.0;
        assert_ne!(crisp(points, 2.0)[0].y, crisp(points, 2.0)[1].y);
    }

    #[test]
    fn the_scattered_cloud_stays_inside_the_strip() {
        let mut canvas = Canvas::new(72, 9);
        for step in 0..40 {
            render(&mut canvas, 72, 9, 0.5, step as f32 * 0.4);
            let cells = canvas.resolve();
            let dots = |cell: &crate::raster::Cell| (cell.glyph as u32).saturating_sub(0x2800);
            // The three outer dot rows at each edge stay empty. In a braille cell, bits 1 and 8
            // are the top dot row, 2 and 16 the second, 4 and 32 the third, 64 and 128 the bottom.
            for cell in &cells[..72] {
                assert_eq!(dots(cell) & 0x3f, 0, "{step}: {cell:?}");
            }
            for cell in &cells[72 * 8..] {
                assert_eq!(dots(cell) & 0xf6, 0, "{step}: {cell:?}");
            }
        }
    }

    #[test]
    fn workspace_moves_between_destinations_and_frames_are_deterministic() {
        let mut canvas = Canvas::new(80, 18);
        let mut snapshots = Vec::new();
        for t in [0.0, 0.5, 1.0] {
            render(&mut canvas, 80, 18, t, 0.0);
            let cells = canvas.resolve().to_vec();
            assert!(cells.iter().filter(|c| c.glyph != ' ').count() > 50);
            render(&mut canvas, 80, 18, t, 0.0);
            assert_eq!(cells, canvas.resolve());
            snapshots.push(cells);
        }
        assert_ne!(snapshots[0], snapshots[1]);
        assert_ne!(snapshots[0], snapshots[2]);
        // The bright workspace, excluding the gray destination frames, changes sides.
        for (cells, right) in [(&snapshots[0], false), (&snapshots[2], true)] {
            let positions: Vec<_> = cells
                .iter()
                .enumerate()
                .filter(|(_, c)| c.glyph != ' ' && c.color[2] > c.color[0] + 10)
                .map(|(i, _)| i % 80)
                .collect();
            assert!(!positions.is_empty());
            let center = positions.iter().sum::<usize>() / positions.len();
            assert_eq!(center > 40, right);
        }
    }
}
