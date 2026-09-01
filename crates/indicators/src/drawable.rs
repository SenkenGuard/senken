//! Display primitives emitted by an indicator.
//!
//! Indicators calculate values; charts draw geometry.  Keeping the latter in
//! this small vocabulary means a consumer does not need a special renderer
//! each time an indicator grows beyond a line series.

use std::collections::VecDeque;

/// A point in chart coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Unix nanoseconds on the horizontal axis.
    pub time: i64,
    /// A display or decision value on the vertical axis.
    pub value: f64,
}

/// A price coordinate used by a drawable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PriceCoord {
    /// A visual annotation chosen on a chart. It is not an order price.
    Annotation(f64),
    /// A price which can later be used for execution.
    ///
    /// ```compile_fail
    /// use senken_indicators::PriceCoord;
    ///
    /// let _ = PriceCoord::Executable(42.0);
    /// ```
    ///
    /// An executable coordinate cannot accidentally receive an annotation's
    /// floating-point value; it requires [`ScaledPrice`].
    Executable(ScaledPrice),
}

/// An exact, instrument-scaled price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScaledPrice {
    /// Integer value at [`Self::scale`].
    pub value: i64,
    /// Decimal scale of [`Self::value`].
    pub scale: u32,
}

/// How a series is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeriesShape {
    /// Connected values.
    Line,
    /// Vertical columns.
    Histogram,
    /// A line with filled area beneath it.
    Area,
}

/// How far a segment or level extends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extend {
    /// Only between its anchors.
    None,
    /// Beyond its second anchor.
    Forward,
    /// Before its first anchor.
    Backward,
    /// In both directions.
    Both,
}

/// Where a label is positioned relative to its anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelAnchor {
    /// Above the anchor.
    Above,
    /// Below the anchor.
    Below,
    /// Centered on the anchor.
    Center,
}

/// One thing a chart renderer can draw.
#[derive(Debug, Clone, PartialEq)]
pub enum Drawable {
    /// A computed series belonging to an indicator field.
    Series {
        /// Indicator field that owns these points.
        field: String,
        /// Rendering shape.
        shape: SeriesShape,
        /// Values in chronological order.
        points: Vec<Point>,
    },
    /// A line segment between two points.
    Segment {
        /// First endpoint.
        a: Point,
        /// Second endpoint.
        b: Point,
        /// Extension behaviour.
        extend: Extend,
    },
    /// A horizontal level.
    Level {
        /// Price coordinate.
        price: PriceCoord,
        /// Extension behaviour.
        extend: Extend,
    },
    /// A rectangular zone.
    Box {
        /// First corner.
        a: Point,
        /// Opposite corner.
        b: Point,
    },
    /// Text at one chart point.
    Label {
        /// Text position.
        at: Point,
        /// Text content.
        text: String,
        /// Position relative to the anchor.
        anchor: LabelAnchor,
    },
}

/// Whether a drawing tool creates visual annotations or executable prices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceRole {
    /// Visual coordinates only.
    Annotation,
    /// Exact scaled prices required.
    Executable,
}

/// The anchor interaction a tool needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAnchors {
    /// One anchor, constrained to price.
    Price,
    /// One anchor, constrained to time.
    Time,
    /// Two unrestricted anchors.
    TwoPoints,
}

/// Data defining a drawing tool; rendering is not tool-specific.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolDescriptor {
    /// Stable tool key.
    pub id: &'static str,
    /// Required anchors.
    pub anchors: ToolAnchors,
    /// Whether the tool's prices may be represented as annotations.
    pub price_role: PriceRole,
}

impl ToolDescriptor {
    /// Creates a descriptor for a visual-only tool.
    #[must_use]
    pub const fn annotation(id: &'static str, anchors: ToolAnchors) -> Self {
        Self {
            id,
            anchors,
            price_role: PriceRole::Annotation,
        }
    }
}

/// Bounded display output for one item.
///
/// Series are one drawable regardless of their number of points. Other
/// geometry is capped to prevent one faulty item from retaining unbounded
/// shared-server memory.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayList {
    max_objects: usize,
    drawables: VecDeque<Drawable>,
    discarded_objects: usize,
}

impl DisplayList {
    /// Creates an empty display list with a maximum non-series object count.
    #[must_use]
    pub fn new(max_objects: usize) -> Self {
        Self {
            max_objects,
            drawables: VecDeque::new(),
            discarded_objects: 0,
        }
    }

    /// Adds a drawable, discarding the oldest bounded object when needed.
    pub fn push(&mut self, drawable: Drawable) {
        if !matches!(drawable, Drawable::Series { .. }) && self.max_objects == 0 {
            self.discarded_objects = self.discarded_objects.saturating_add(1);
            return;
        }
        if !matches!(drawable, Drawable::Series { .. }) {
            while self.object_count() >= self.max_objects {
                if let Some(index) = self
                    .drawables
                    .iter()
                    .position(|entry| !matches!(entry, Drawable::Series { .. }))
                {
                    let _ = self.drawables.remove(index);
                    self.discarded_objects = self.discarded_objects.saturating_add(1);
                } else {
                    break;
                }
            }
        }
        self.drawables.push_back(drawable);
    }

    /// Retained drawables in insertion order.
    #[must_use]
    pub fn drawables(&self) -> impl ExactSizeIterator<Item = &Drawable> {
        self.drawables.iter()
    }

    /// Number of oldest objects discarded because of the limit.
    #[must_use]
    pub const fn discarded_objects(&self) -> usize {
        self.discarded_objects
    }

    fn object_count(&self) -> usize {
        self.drawables
            .iter()
            .filter(|entry| !matches!(entry, Drawable::Series { .. }))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::{DisplayList, Drawable, Extend, Point, PriceCoord, ToolAnchors, ToolDescriptor};

    fn level(value: f64) -> Drawable {
        Drawable::Level {
            price: PriceCoord::Annotation(value),
            extend: Extend::Both,
        }
    }

    #[test]
    fn a_display_list_discards_the_oldest_object_and_reports_it() {
        let mut display = DisplayList::new(2);
        display.push(level(1.0));
        display.push(Drawable::Box {
            a: Point {
                time: 1,
                value: 1.0,
            },
            b: Point {
                time: 2,
                value: 2.0,
            },
        });
        display.push(level(3.0));

        assert_eq!(display.discarded_objects(), 1);
        assert_eq!(display.drawables().count(), 2);
        assert!(matches!(
            display.drawables().next(),
            Some(Drawable::Box { .. })
        ));
    }

    #[test]
    fn annotation_tools_state_their_anchor_constraint() {
        let descriptor = ToolDescriptor::annotation("trend", ToolAnchors::TwoPoints);
        assert_eq!(descriptor.anchors, ToolAnchors::TwoPoints);
    }
}
