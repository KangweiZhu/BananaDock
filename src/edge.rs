//! Which screen edge the dock lives on, and the coordinates that follow from
//! it.
//!
//! Everything about the row is one-dimensional. Slots accumulate one after
//! another *along* the row, and the artwork stands away from the screen's edge
//! *across* it -- the panel's thickness, the icon's margin, the dot's offset
//! are all measured that way. Written like that, the same numbers describe a
//! dock on any of the four edges, and [`Frame`] is the only thing that has to
//! know which way round they go.
//!
//! The alternative -- x and y everywhere, with a `if vertical` at each use --
//! spreads the decision across every drawing call and every hit test, where
//! exactly one of them only has to be got wrong once.

use tiny_skia::Rect;

/// The edge of the screen the dock is docked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Edge {
    #[default]
    Bottom,
    Top,
    Left,
    Right,
}

impl Edge {
    /// Whether the row runs up and down rather than across.
    pub fn is_vertical(self) -> bool {
        matches!(self, Edge::Left | Edge::Right)
    }

    /// Parses the configuration's spelling, if it is one.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "bottom" => Some(Edge::Bottom),
            "top" => Some(Edge::Top),
            "left" => Some(Edge::Left),
            "right" => Some(Edge::Right),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Edge::Bottom => "bottom",
            Edge::Top => "top",
            Edge::Left => "left",
            Edge::Right => "right",
        }
    }
}

/// The row's coordinate system laid over a surface of a given size.
///
/// `along` runs down the row: left to right on a horizontal dock, top to
/// bottom on a vertical one. `across` measures *inwards from the screen's
/// edge*, so zero is the edge itself and larger numbers are further into the
/// screen -- which is the direction a magnified icon grows in, whichever edge
/// the dock is on.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    edge: Edge,
    /// The surface's size in logical pixels.
    surface: (f32, f32),
}

impl Frame {
    pub fn new(edge: Edge, surface: (f32, f32)) -> Self {
        Self { edge, surface }
    }

    /// How much room the row has to lay itself out in.
    pub fn length(&self) -> f32 {
        if self.edge.is_vertical() {
            self.surface.1
        } else {
            self.surface.0
        }
    }

    /// A rectangle given in the row's coordinates: `len` down the row from
    /// `along`, `thick` deep from `across`.
    pub fn rect(&self, along: f32, across: f32, len: f32, thick: f32) -> Option<Rect> {
        let (w, h) = self.surface;
        match self.edge {
            Edge::Bottom => Rect::from_xywh(along, h - across - thick, len, thick),
            Edge::Top => Rect::from_xywh(along, across, len, thick),
            Edge::Left => Rect::from_xywh(across, along, thick, len),
            Edge::Right => Rect::from_xywh(w - across - thick, along, thick, len),
        }
    }

    /// Where a point in the row's coordinates lands on the surface.
    pub fn point(&self, along: f32, across: f32) -> (f32, f32) {
        let (w, h) = self.surface;
        match self.edge {
            Edge::Bottom => (along, h - across),
            Edge::Top => (along, across),
            Edge::Left => (across, along),
            Edge::Right => (w - across, along),
        }
    }

    /// How far down the row a surface position is.
    pub fn along_of(&self, x: f32, y: f32) -> f32 {
        if self.edge.is_vertical() {
            y
        } else {
            x
        }
    }

    /// How far in from the screen's edge a surface position is.
    pub fn across_of(&self, x: f32, y: f32) -> f32 {
        let (w, h) = self.surface;
        match self.edge {
            Edge::Bottom => h - y,
            Edge::Top => y,
            Edge::Left => x,
            Edge::Right => w - x,
        }
    }

    /// A rectangle's near edge -- the side facing the screen's edge -- as an
    /// `across` measurement.
    pub fn near_of(&self, rect: Rect) -> f32 {
        match self.edge {
            Edge::Bottom => self.surface.1 - rect.bottom(),
            Edge::Top => rect.y(),
            Edge::Left => rect.x(),
            Edge::Right => self.surface.0 - rect.right(),
        }
    }

    /// A rectangle's start, measured down the row.
    pub fn along_start_of(&self, rect: Rect) -> f32 {
        if self.edge.is_vertical() {
            rect.y()
        } else {
            rect.x()
        }
    }

    /// A rectangle's extent down the row.
    pub fn along_len_of(&self, rect: Rect) -> f32 {
        if self.edge.is_vertical() {
            rect.height()
        } else {
            rect.width()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SURFACE: (f32, f32) = (1000.0, 400.0);

    /// The same row coordinates have to land on the right part of the surface
    /// for each edge, or every drawing call is wrong in a different way.
    #[test]
    fn a_rectangle_lands_against_its_own_edge() {
        // 20 down the row, 8 in from the edge, 100 long, 60 thick.
        let cases = [
            (Edge::Bottom, (20.0, 400.0 - 8.0 - 60.0, 100.0, 60.0)),
            (Edge::Top, (20.0, 8.0, 100.0, 60.0)),
            (Edge::Left, (8.0, 20.0, 60.0, 100.0)),
            (Edge::Right, (1000.0 - 8.0 - 60.0, 20.0, 60.0, 100.0)),
        ];

        for (edge, want) in cases {
            let f = Frame::new(edge, SURFACE);
            let r = f.rect(20.0, 8.0, 100.0, 60.0).unwrap();
            assert_eq!(
                (r.x(), r.y(), r.width(), r.height()),
                want,
                "{:?} put the rectangle in the wrong place",
                edge
            );
        }
    }

    /// A vertical dock lays its row out down the screen, so the room it has is
    /// the surface's height rather than its width.
    #[test]
    fn the_row_runs_along_the_edge_it_sits_on() {
        assert_eq!(Frame::new(Edge::Bottom, SURFACE).length(), 1000.0);
        assert_eq!(Frame::new(Edge::Top, SURFACE).length(), 1000.0);
        assert_eq!(Frame::new(Edge::Left, SURFACE).length(), 400.0);
        assert_eq!(Frame::new(Edge::Right, SURFACE).length(), 400.0);
    }

    /// Hit testing reads the pointer back through the same mapping, so a round
    /// trip has to come out where it went in.
    #[test]
    fn reading_a_point_back_gives_what_went_in() {
        for edge in [Edge::Bottom, Edge::Top, Edge::Left, Edge::Right] {
            let f = Frame::new(edge, SURFACE);
            let (x, y) = f.point(120.0, 30.0);
            assert!((f.along_of(x, y) - 120.0).abs() < 0.01, "{edge:?} along");
            assert!((f.across_of(x, y) - 30.0).abs() < 0.01, "{edge:?} across");
        }
    }

    /// The panel's own rectangle is handed back to the input and blur regions
    /// in surface coordinates, and read back to place things against it.
    #[test]
    fn a_rectangles_edges_read_back_in_row_terms() {
        for edge in [Edge::Bottom, Edge::Top, Edge::Left, Edge::Right] {
            let f = Frame::new(edge, SURFACE);
            let r = f.rect(50.0, 8.0, 200.0, 60.0).unwrap();
            assert!((f.near_of(r) - 8.0).abs() < 0.01, "{edge:?} near");
            assert!((f.along_start_of(r) - 50.0).abs() < 0.01, "{edge:?} start");
            assert!((f.along_len_of(r) - 200.0).abs() < 0.01, "{edge:?} length");
        }
    }

    #[test]
    fn edges_round_trip_through_their_names() {
        for edge in [Edge::Bottom, Edge::Top, Edge::Left, Edge::Right] {
            assert_eq!(Edge::parse(edge.name()), Some(edge));
        }
        assert_eq!(Edge::parse("BOTTOM"), Some(Edge::Bottom));
        assert_eq!(Edge::parse("sideways"), None);
    }
}
