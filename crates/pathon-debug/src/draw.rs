use pathon_api::Camera;
use pathon_api::glam::{Mat4, Vec3};

use crate::color::DebugColor;
use crate::shapes::{aabb_corners, arrow_head, circle_points, cuboid_corners, cuboid_edges, frustum_corners};
use crate::vertices::{LINE_FLAG_DEPTH_TEST, LineInstance, WireVertex, pack_color};

const MAX_THICKNESS_PIXELS: f32 = 64.0;
const DEFAULT_MAX_LINES: usize = 1 << 18;
const DEFAULT_MAX_WIRE_TRIANGLES: usize = 1 << 16;
const SPHERE_SEGMENTS: u32 = 32;

/// How one primitive looks and how long it stays.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LineStyle {
    pub color: DebugColor,
    /// Width in screen pixels, whatever the distance or the render scale.
    pub thickness: f32,
    /// Hidden where `Slot::DEPTH` has something nearer. Ignored when no pack writes depth.
    pub depth_test: bool,
    /// Frames the primitive is drawn for; one means "this frame only".
    pub frames: u32,
}

impl LineStyle {
    pub const fn new(color: DebugColor) -> Self {
        Self {
            color,
            thickness: 1.0,
            depth_test: false,
            frames: 1,
        }
    }

    #[must_use]
    pub const fn thickness(self, pixels: f32) -> Self {
        Self {
            thickness: pixels,
            ..self
        }
    }

    #[must_use]
    pub const fn depth_tested(self) -> Self {
        Self {
            depth_test: true,
            ..self
        }
    }

    #[must_use]
    pub const fn persistent(self, frames: u32) -> Self {
        Self { frames, ..self }
    }

    fn thickness_pixels(self) -> f32 {
        if self.thickness.is_finite() {
            self.thickness.clamp(0.1, MAX_THICKNESS_PIXELS)
        } else {
            1.0
        }
    }
}

impl Default for LineStyle {
    fn default() -> Self {
        Self::new(DebugColor::WHITE)
    }
}

impl From<DebugColor> for LineStyle {
    fn from(color: DebugColor) -> Self {
        Self::new(color)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DebugLine {
    pub start: Vec3,
    pub end: Vec3,
    pub style: LineStyle,
}

#[derive(Debug)]
struct Retained<T> {
    value: T,
    frames_left: u32,
    // The frame that drew it last: a primitive counts down only after a frame really showed it.
    drawn_in: Option<u64>,
}

impl<T> Retained<T> {
    fn new(value: T, frames: u32) -> Self {
        Self {
            value,
            frames_left: frames.max(1),
            drawn_in: None,
        }
    }
}

fn count_down<T>(items: &mut Vec<Retained<T>>, frame: u64) {
    items.retain_mut(|item| {
        if item.drawn_in.is_some_and(|drawn| drawn != frame) {
            item.drawn_in = None;
            item.frames_left -= 1;
        }
        item.frames_left > 0
    });
}

/// Immediate-mode debug geometry in world space. It lives in `Shared` (`DebugPack` inserts it); the application fills
/// it between frames and the features of this pack draw and retire it. Nothing here touches the GPU or any I/O.
#[derive(Debug)]
pub struct DebugDraw {
    lines: Vec<Retained<DebugLine>>,
    wire_meshes: Vec<Retained<Vec<WireVertex>>>,
    wire_triangles: usize,
    max_lines: usize,
    max_wire_triangles: usize,
    dropped_lines: usize,
    dropped_wireframes: usize,
}

impl Default for DebugDraw {
    fn default() -> Self {
        Self::new()
    }
}

impl DebugDraw {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_LINES, DEFAULT_MAX_WIRE_TRIANGLES)
    }

    /// Primitives past the limits are dropped and reported once as U0002.
    pub fn with_capacity(max_lines: usize, max_wire_triangles: usize) -> Self {
        Self {
            lines: Vec::new(),
            wire_meshes: Vec::new(),
            wire_triangles: 0,
            max_lines,
            max_wire_triangles,
            dropped_lines: 0,
            dropped_wireframes: 0,
        }
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn lines(&self) -> impl ExactSizeIterator<Item = &DebugLine> {
        self.lines.iter().map(|line| &line.value)
    }

    pub fn wire_triangle_count(&self) -> usize {
        self.wire_triangles
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.wire_meshes.is_empty()
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.wire_meshes.clear();
        self.wire_triangles = 0;
    }

    pub fn line(&mut self, start: Vec3, end: Vec3, style: impl Into<LineStyle>) {
        let style = style.into();
        if !start.is_finite() || !end.is_finite() {
            return;
        }
        if self.lines.len() >= self.max_lines {
            self.dropped_lines += 1;
            return;
        }
        let style = LineStyle {
            color: style.color.sanitized(),
            thickness: style.thickness_pixels(),
            ..style
        };
        self.lines
            .push(Retained::new(DebugLine { start, end, style }, style.frames));
    }

    pub fn polyline(&mut self, points: &[Vec3], closed: bool, style: impl Into<LineStyle>) {
        let style = style.into();
        for pair in points.windows(2) {
            self.line(pair[0], pair[1], style);
        }
        if let (true, Some(first), Some(last)) = (closed, points.first(), points.last())
            && points.len() > 2
        {
            self.line(*last, *first, style);
        }
    }

    /// The 12 edges of an axis-aligned box.
    pub fn aabb(&mut self, min: Vec3, max: Vec3, style: impl Into<LineStyle>) {
        let style = style.into();
        for (start, end) in cuboid_edges(&aabb_corners(min, max)) {
            self.line(start, end, style);
        }
    }

    /// The unit cube `[-0.5, 0.5]^3` moved by `transform`: an oriented box for a model matrix.
    pub fn cuboid(&mut self, transform: Mat4, style: impl Into<LineStyle>) {
        let style = style.into();
        for (start, end) in cuboid_edges(&cuboid_corners(transform)) {
            self.line(start, end, style);
        }
    }

    pub fn circle(&mut self, center: Vec3, normal: Vec3, radius: f32, style: impl Into<LineStyle>) {
        self.polyline(&circle_points(center, normal, radius, SPHERE_SEGMENTS), true, style);
    }

    /// Three great circles, one per axis.
    pub fn sphere(&mut self, center: Vec3, radius: f32, style: impl Into<LineStyle>) {
        let style = style.into();
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            self.circle(center, axis, radius, style);
        }
    }

    pub fn arrow(&mut self, from: Vec3, to: Vec3, style: impl Into<LineStyle>) {
        let style = style.into();
        self.line(from, to, style);
        for (tip, barb) in arrow_head(from, to).into_iter().flatten() {
            self.line(tip, barb, style);
        }
    }

    /// Gizmo of a transform: X red, Y green, Z blue, each `size` metres long (scaled by the transform).
    pub fn axes(&mut self, transform: Mat4, size: f32, thickness: f32) {
        let origin = transform.transform_point3(Vec3::ZERO);
        for (axis, color) in [
            (Vec3::X, DebugColor::RED),
            (Vec3::Y, DebugColor::GREEN),
            (Vec3::Z, DebugColor::BLUE),
        ] {
            let tip = transform.transform_point3(axis * size);
            self.arrow(origin, tip, LineStyle::new(color).thickness(thickness));
        }
    }

    /// The frustum of `camera` from its near plane to `far` metres along the view axis.
    pub fn frustum(&mut self, camera: &Camera, far: f32, style: impl Into<LineStyle>) {
        let Some((near_corners, far_corners)) = frustum_corners(camera, far) else {
            return;
        };
        let style = style.into();
        self.polyline(&near_corners, true, style);
        self.polyline(&far_corners, true, style);
        for (near, far) in near_corners.into_iter().zip(far_corners) {
            self.line(near, far, style);
        }
    }

    /// Triangle edges of a mesh in world space after `transform`. Indices past the vertex list are skipped. Hardware
    /// line mode draws 1 px lines; the barycentric fallback honours `style.thickness` (see the crate page).
    pub fn wireframe(&mut self, positions: &[Vec3], indices: &[u32], transform: Mat4, style: impl Into<LineStyle>) {
        let style = style.into();
        let triangle_count = indices.len() / 3;
        if triangle_count == 0 {
            return;
        }
        if self.wire_triangles + triangle_count > self.max_wire_triangles {
            self.dropped_wireframes += 1;
            return;
        }
        let color = pack_color(style.color.sanitized().to_array());
        let thickness = style.thickness_pixels();
        let mut vertices = Vec::with_capacity(triangle_count * 3);
        for triangle in indices.chunks_exact(3) {
            let corners: Option<Vec<Vec3>> = triangle
                .iter()
                .map(|index| positions.get(*index as usize).copied())
                .collect();
            let Some(corners) = corners else { continue };
            for corner in corners {
                let position = transform.transform_point3(corner);
                if !position.is_finite() {
                    break;
                }
                vertices.push(WireVertex {
                    position: position.to_array(),
                    color,
                    thickness,
                });
            }
            vertices.truncate(vertices.len() / 3 * 3);
        }
        self.wire_triangles += vertices.len() / 3;
        self.wire_meshes.push(Retained::new(vertices, style.frames));
    }

    /// Lines dropped for lack of capacity since the last call.
    pub(crate) fn take_dropped_lines(&mut self) -> usize {
        std::mem::take(&mut self.dropped_lines)
    }

    pub(crate) fn take_dropped_wireframes(&mut self) -> usize {
        std::mem::take(&mut self.dropped_wireframes)
    }

    pub(crate) fn collect_lines(&mut self, frame: u64, out: &mut Vec<LineInstance>) {
        count_down(&mut self.lines, frame);
        for line in &mut self.lines {
            line.drawn_in = Some(frame);
            let DebugLine { start, end, style } = line.value;
            out.push(LineInstance {
                start: start.to_array(),
                thickness: style.thickness,
                end: end.to_array(),
                flags: if style.depth_test { LINE_FLAG_DEPTH_TEST } else { 0 },
                color: style.color.to_array(),
            });
        }
    }

    pub(crate) fn collect_wire_vertices(&mut self, frame: u64, out: &mut Vec<WireVertex>) {
        count_down(&mut self.wire_meshes, frame);
        self.wire_triangles = self.wire_meshes.iter().map(|mesh| mesh.value.len() / 3).sum();
        for mesh in &mut self.wire_meshes {
            mesh.drawn_in = Some(frame);
            out.extend_from_slice(&mesh.value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collected(draw: &mut DebugDraw, frame: u64) -> usize {
        let mut out = Vec::new();
        draw.collect_lines(frame, &mut out);
        out.len()
    }

    #[test]
    fn a_line_is_drawn_for_one_frame_by_default() {
        let mut draw = DebugDraw::new();
        draw.line(Vec3::ZERO, Vec3::X, DebugColor::RED);

        assert_eq!(collected(&mut draw, 0), 1);
        assert_eq!(collected(&mut draw, 0), 1, "a second record attempt of the same frame draws it again");
        assert_eq!(collected(&mut draw, 1), 0);
        assert!(draw.is_empty());
    }

    #[test]
    fn persistent_lines_survive_exactly_their_frame_count() {
        let mut draw = DebugDraw::new();
        draw.line(Vec3::ZERO, Vec3::X, LineStyle::new(DebugColor::RED).persistent(3));

        let seen: Vec<_> = (0..5).map(|frame| collected(&mut draw, frame)).collect();

        assert_eq!(seen, [1, 1, 1, 0, 0]);
    }

    #[test]
    fn a_line_added_after_a_frame_is_not_aged_by_it() {
        let mut draw = DebugDraw::new();
        draw.line(Vec3::ZERO, Vec3::X, DebugColor::RED);
        assert_eq!(collected(&mut draw, 7), 1);

        draw.line(Vec3::ZERO, Vec3::Y, DebugColor::GREEN);

        assert_eq!(collected(&mut draw, 8), 1, "only the new line is left");
    }

    #[test]
    fn lines_past_the_capacity_are_dropped_and_counted() {
        let mut draw = DebugDraw::with_capacity(2, 8);
        for _ in 0..5 {
            draw.line(Vec3::ZERO, Vec3::X, DebugColor::WHITE);
        }

        assert_eq!(draw.line_count(), 2);
        assert_eq!(draw.take_dropped_lines(), 3);
        assert_eq!(draw.take_dropped_lines(), 0);
    }

    #[test]
    fn non_finite_lines_and_styles_are_repaired() {
        let mut draw = DebugDraw::new();
        draw.line(Vec3::ZERO, Vec3::new(f32::NAN, 0.0, 0.0), DebugColor::WHITE);
        draw.line(
            Vec3::ZERO,
            Vec3::X,
            LineStyle::new(DebugColor::new(f32::NAN, 5.0, 0.0, 1.0)).thickness(f32::INFINITY),
        );

        let line = draw.lines().next().expect("the finite line stays");
        assert_eq!(draw.line_count(), 1);
        assert_eq!(line.style.thickness, 1.0);
        assert_eq!(line.style.color.to_array(), [0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn primitives_add_the_expected_number_of_lines() {
        let mut draw = DebugDraw::new();
        let count = |draw: &DebugDraw| draw.line_count();

        draw.aabb(Vec3::ZERO, Vec3::ONE, DebugColor::WHITE);
        assert_eq!(count(&draw), 12);
        draw.clear();
        draw.cuboid(Mat4::IDENTITY, DebugColor::WHITE);
        assert_eq!(count(&draw), 12);
        draw.clear();
        draw.circle(Vec3::ZERO, Vec3::Y, 1.0, DebugColor::WHITE);
        assert_eq!(count(&draw), SPHERE_SEGMENTS as usize);
        draw.clear();
        draw.sphere(Vec3::ZERO, 1.0, DebugColor::WHITE);
        assert_eq!(count(&draw), 3 * SPHERE_SEGMENTS as usize);
        draw.clear();
        draw.arrow(Vec3::ZERO, Vec3::X, DebugColor::WHITE);
        assert_eq!(count(&draw), 5);
        draw.clear();
        draw.axes(Mat4::IDENTITY, 1.0, 2.0);
        assert_eq!(count(&draw), 15);
        draw.clear();
        draw.frustum(&Camera::default(), 10.0, DebugColor::WHITE);
        assert_eq!(count(&draw), 12);
    }

    #[test]
    fn axes_are_red_green_and_blue_along_x_y_and_z() {
        let mut draw = DebugDraw::new();
        draw.axes(Mat4::from_translation(Vec3::new(0.0, 5.0, 0.0)), 2.0, 3.0);

        let shafts: Vec<_> = draw.lines().take(1).chain(draw.lines().skip(5).take(1)).chain(draw.lines().skip(10).take(1)).collect();

        assert_eq!(shafts[0].end, Vec3::new(2.0, 5.0, 0.0));
        assert_eq!(shafts[0].style.color, DebugColor::RED);
        assert_eq!(shafts[1].end, Vec3::new(0.0, 7.0, 0.0));
        assert_eq!(shafts[1].style.color, DebugColor::GREEN);
        assert_eq!(shafts[2].end, Vec3::new(0.0, 5.0, 2.0));
        assert_eq!(shafts[2].style.color, DebugColor::BLUE);
        assert_eq!(shafts[2].style.thickness, 3.0);
    }

    #[test]
    fn wireframe_expands_to_a_triangle_soup_and_skips_bad_indices() {
        let mut draw = DebugDraw::new();
        let positions = [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::ONE];

        draw.wireframe(&positions, &[0, 1, 2, 1, 3, 9, 1, 2, 3], Mat4::from_translation(Vec3::Z), DebugColor::CYAN);

        assert_eq!(draw.wire_triangle_count(), 2);
        let mut vertices = Vec::new();
        draw.collect_wire_vertices(0, &mut vertices);
        assert_eq!(vertices.len(), 6);
        assert_eq!(vertices[0].position, [0.0, 0.0, 1.0]);
        assert_eq!(vertices[0].color, [0, 255, 255, 255]);
    }

    #[test]
    fn wireframes_expire_like_lines() {
        let mut draw = DebugDraw::new();
        draw.wireframe(&[Vec3::ZERO, Vec3::X, Vec3::Y], &[0, 1, 2], Mat4::IDENTITY, DebugColor::WHITE);
        let mut vertices = Vec::new();

        draw.collect_wire_vertices(0, &mut vertices);
        draw.collect_wire_vertices(1, &mut vertices);

        assert_eq!(vertices.len(), 3);
        assert_eq!(draw.wire_triangle_count(), 0);
    }

    #[test]
    fn a_wireframe_past_the_capacity_is_dropped_whole() {
        let mut draw = DebugDraw::with_capacity(8, 1);
        draw.wireframe(&[Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::ONE], &[0, 1, 2, 1, 2, 3], Mat4::IDENTITY, DebugColor::WHITE);

        assert_eq!(draw.wire_triangle_count(), 0);
        assert_eq!(draw.take_dropped_wireframes(), 1);
    }
}
