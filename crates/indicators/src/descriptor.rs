//! Metadata describing the indicators this crate provides.

/// One built-in indicator's stable identifier and presentation contract.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IndicatorDescriptor {
    /// Name accepted by consumers that construct this indicator.
    pub id: &'static str,
    /// Human-readable name.
    pub title: &'static str,
    /// Compact label for chart chrome.
    pub short_title: &'static str,
    /// Legend template. Parameters are substituted by the caller.
    pub legend: &'static str,
    /// Configurable parameters.
    pub params: &'static [ParamSpec],
    /// Values emitted for each initialized bar.
    pub plots: &'static [PlotSpec],
    /// How emitted values are scaled.
    pub scale: ScaleHint,
    /// Volume unit the computation accepts.
    pub requires: VolumeRequirement,
    /// Where an instance may be displayed.
    pub placement: Placement,
    /// The convergence behaviour used to select history before a range.
    pub smoothing: Smoothing,
}

impl IndicatorDescriptor {
    /// Number of earlier bars needed before a requested range.
    #[must_use]
    pub fn warmup_bars(self, values: impl Fn(&str) -> Option<u64>) -> u64 {
        self.smoothing.warmup_bars(self.params, values)
    }
}

/// A configurable parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    /// Wire key.
    pub name: &'static str,
    /// Parameter shape.
    pub kind: ParamKind,
    /// Default value shown to a client.
    pub default: ParamDefault,
    /// Inclusive lower bound when applicable.
    pub min: Option<f64>,
}

/// Parameter value type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// A whole-number parameter such as a lookback period.
    Integer,
    /// A fractional indicator setting such as band width.
    Number,
}

/// A parameter default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamDefault {
    /// Whole-number default.
    Integer(u64),
    /// Fractional default.
    Number(f64),
}

/// One emitted plot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlotSpec {
    /// Wire key.
    pub field: &'static str,
    /// Display label.
    pub label: &'static str,
    /// Default rendering shape.
    pub shape: PlotShape,
    /// Default CSS colour.
    pub color: &'static str,
}

/// Chart rendering shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlotShape {
    /// Connected values.
    Line,
    /// Vertical columns.
    Histogram,
}

/// Scaling semantics for indicator output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScaleHint {
    /// Values use the instrument price scale.
    Price,
    /// Values are already dimensionless and bounded.
    Ratio {
        /// Inclusive lower bound.
        min: f64,
        /// Inclusive upper bound.
        max: f64,
    },
    /// Values use the instrument quantity scale.
    Volume,
    /// Values define their own scale.
    Own,
}

/// Volume unit an indicator can consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeRequirement {
    /// The calculation does not depend on volume units.
    Any,
    /// The calculation requires real traded quantity.
    Real,
}

/// Valid chart locations for an indicator instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Price-pane overlay only.
    Overlay,
    /// Separate pane only.
    SubPane,
    /// Either a price-pane overlay or a separate pane.
    Either,
}

/// Convergence model used to determine the warm-up prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Smoothing {
    /// A fixed rolling window.
    Windowed {
        /// Parameter naming the window, if there is one.
        period: Option<&'static str>,
    },
    /// A recursively smoothed calculation.
    Recursive {
        /// Parameter naming the smoothing period.
        period: &'static str,
        /// Whether alpha is Wilder's `1 / period` rather than EMA's form.
        wilder: bool,
    },
}

impl Smoothing {
    fn warmup_bars(self, params: &[ParamSpec], values: impl Fn(&str) -> Option<u64>) -> u64 {
        let value = |name| {
            values(name)
                .or_else(|| {
                    params
                        .iter()
                        .find(|param| param.name == name)
                        .and_then(|param| match param.default {
                            ParamDefault::Integer(value) => Some(value),
                            ParamDefault::Number(_) => None,
                        })
                })
                .unwrap_or(1)
        };
        match self {
            Self::Windowed { period } => period.map_or(0, value),
            Self::Recursive { period, wilder } => {
                let period = u32::try_from(value(period).max(1)).unwrap_or(u32::MAX);
                // Keep the seed's influence below one ten-thousandth, the
                // precision at which chart labels stop distinguishing it.
                let alpha = if wilder {
                    1.0 / f64::from(period)
                } else {
                    2.0 / f64::from(period.saturating_add(1))
                };
                let mut decay = 0_u64;
                let mut residual = 1.0;
                while residual > 1.0e-4 {
                    residual *= 1.0 - alpha;
                    decay = decay.saturating_add(1);
                }
                u64::from(period).saturating_add(decay)
            }
        }
    }
}

const PERIOD_20: ParamSpec = ParamSpec {
    name: "period",
    kind: ParamKind::Integer,
    default: ParamDefault::Integer(20),
    min: Some(1.0),
};
const PERIOD_14: ParamSpec = ParamSpec {
    name: "period",
    kind: ParamKind::Integer,
    default: ParamDefault::Integer(14),
    min: Some(1.0),
};
const EMA_PARAMS: &[ParamSpec] = &[ParamSpec {
    name: "period",
    kind: ParamKind::Integer,
    default: ParamDefault::Integer(50),
    min: Some(1.0),
}];
const SMA_PARAMS: &[ParamSpec] = &[PERIOD_20];
const WMA_PARAMS: &[ParamSpec] = &[PERIOD_20];
const RSI_PARAMS: &[ParamSpec] = &[PERIOD_14];
const ATR_PARAMS: &[ParamSpec] = &[PERIOD_14];
const MACD_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "fast_period",
        kind: ParamKind::Integer,
        default: ParamDefault::Integer(12),
        min: Some(1.0),
    },
    ParamSpec {
        name: "slow_period",
        kind: ParamKind::Integer,
        default: ParamDefault::Integer(26),
        min: Some(1.0),
    },
    ParamSpec {
        name: "signal_period",
        kind: ParamKind::Integer,
        default: ParamDefault::Integer(9),
        min: Some(1.0),
    },
];
const STOCHASTIC_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "k_period",
        kind: ParamKind::Integer,
        default: ParamDefault::Integer(14),
        min: Some(1.0),
    },
    ParamSpec {
        name: "d_period",
        kind: ParamKind::Integer,
        default: ParamDefault::Integer(3),
        min: Some(1.0),
    },
];
const BOLLINGER_PARAMS: &[ParamSpec] = &[
    PERIOD_20,
    ParamSpec {
        name: "k",
        kind: ParamKind::Number,
        default: ParamDefault::Number(2.0),
        min: Some(0.0),
    },
];
const VALUE_LINE: &[PlotSpec] = &[PlotSpec {
    field: "value",
    label: "VALUE",
    shape: PlotShape::Line,
    color: "#f2f2ef",
}];
const VALUE_HISTOGRAM: &[PlotSpec] = &[PlotSpec {
    field: "value",
    label: "VOLUME",
    shape: PlotShape::Histogram,
    color: "#9aa0a6",
}];
const MACD_PLOTS: &[PlotSpec] = &[
    PlotSpec {
        field: "macd_line",
        label: "MACD",
        shape: PlotShape::Line,
        color: "#7aa7e8",
    },
    PlotSpec {
        field: "macd_signal",
        label: "SIGNAL",
        shape: PlotShape::Line,
        color: "#e8c87a",
    },
    PlotSpec {
        field: "macd_histogram",
        label: "HISTOGRAM",
        shape: PlotShape::Histogram,
        color: "#9aa0a6",
    },
];
const STOCHASTIC_PLOTS: &[PlotSpec] = &[
    PlotSpec {
        field: "stochastic_k",
        label: "%K",
        shape: PlotShape::Line,
        color: "#7aa7e8",
    },
    PlotSpec {
        field: "stochastic_d",
        label: "%D",
        shape: PlotShape::Line,
        color: "#e8c87a",
    },
];
const BOLLINGER_PLOTS: &[PlotSpec] = &[
    PlotSpec {
        field: "bollinger_upper",
        label: "UPPER",
        shape: PlotShape::Line,
        color: "#7de0a3",
    },
    PlotSpec {
        field: "bollinger_middle",
        label: "MIDDLE",
        shape: PlotShape::Line,
        color: "#f2f2ef",
    },
    PlotSpec {
        field: "bollinger_lower",
        label: "LOWER",
        shape: PlotShape::Line,
        color: "#e8836f",
    },
];

/// Built-in descriptors in their stable display order.
pub const DESCRIPTORS: &[IndicatorDescriptor] = &[
    IndicatorDescriptor {
        id: "Sma",
        title: "Simple Moving Average",
        short_title: "SMA",
        legend: "SMA {period}",
        params: SMA_PARAMS,
        plots: VALUE_LINE,
        scale: ScaleHint::Price,
        requires: VolumeRequirement::Any,
        placement: Placement::Overlay,
        smoothing: Smoothing::Windowed {
            period: Some("period"),
        },
    },
    IndicatorDescriptor {
        id: "Ema",
        title: "Exponential Moving Average",
        short_title: "EMA",
        legend: "EMA {period}",
        params: EMA_PARAMS,
        plots: VALUE_LINE,
        scale: ScaleHint::Price,
        requires: VolumeRequirement::Any,
        placement: Placement::Overlay,
        smoothing: Smoothing::Recursive {
            period: "period",
            wilder: false,
        },
    },
    IndicatorDescriptor {
        id: "Wma",
        title: "Weighted Moving Average",
        short_title: "WMA",
        legend: "WMA {period}",
        params: WMA_PARAMS,
        plots: VALUE_LINE,
        scale: ScaleHint::Price,
        requires: VolumeRequirement::Any,
        placement: Placement::Overlay,
        smoothing: Smoothing::Windowed {
            period: Some("period"),
        },
    },
    IndicatorDescriptor {
        id: "Rsi",
        title: "Relative Strength Index",
        short_title: "RSI",
        legend: "RSI {period}",
        params: RSI_PARAMS,
        plots: VALUE_LINE,
        scale: ScaleHint::Ratio {
            min: 0.0,
            max: 100.0,
        },
        requires: VolumeRequirement::Any,
        placement: Placement::SubPane,
        smoothing: Smoothing::Recursive {
            period: "period",
            wilder: true,
        },
    },
    IndicatorDescriptor {
        id: "Macd",
        title: "Moving Average Convergence Divergence",
        short_title: "MACD",
        legend: "MACD {fast_period},{slow_period},{signal_period}",
        params: MACD_PARAMS,
        plots: MACD_PLOTS,
        scale: ScaleHint::Price,
        requires: VolumeRequirement::Any,
        placement: Placement::SubPane,
        smoothing: Smoothing::Recursive {
            period: "slow_period",
            wilder: false,
        },
    },
    IndicatorDescriptor {
        id: "Stochastic",
        title: "Stochastic Oscillator",
        short_title: "STOCH",
        legend: "Stochastic {k_period},{d_period}",
        params: STOCHASTIC_PARAMS,
        plots: STOCHASTIC_PLOTS,
        scale: ScaleHint::Ratio {
            min: 0.0,
            max: 100.0,
        },
        requires: VolumeRequirement::Any,
        placement: Placement::SubPane,
        smoothing: Smoothing::Windowed {
            period: Some("k_period"),
        },
    },
    IndicatorDescriptor {
        id: "BollingerBands",
        title: "Bollinger Bands",
        short_title: "BB",
        legend: "BB {period},{k}",
        params: BOLLINGER_PARAMS,
        plots: BOLLINGER_PLOTS,
        scale: ScaleHint::Price,
        requires: VolumeRequirement::Any,
        placement: Placement::Overlay,
        smoothing: Smoothing::Windowed {
            period: Some("period"),
        },
    },
    IndicatorDescriptor {
        id: "Atr",
        title: "Average True Range",
        short_title: "ATR",
        legend: "ATR {period}",
        params: ATR_PARAMS,
        plots: VALUE_LINE,
        scale: ScaleHint::Price,
        requires: VolumeRequirement::Any,
        placement: Placement::SubPane,
        smoothing: Smoothing::Recursive {
            period: "period",
            wilder: true,
        },
    },
    IndicatorDescriptor {
        id: "Vwap",
        title: "Volume Weighted Average Price",
        short_title: "VWAP",
        legend: "VWAP",
        params: &[],
        plots: VALUE_LINE,
        scale: ScaleHint::Price,
        requires: VolumeRequirement::Real,
        placement: Placement::Overlay,
        smoothing: Smoothing::Windowed { period: None },
    },
    IndicatorDescriptor {
        id: "Volume",
        title: "Volume",
        short_title: "VOLUME",
        legend: "Volume",
        params: &[],
        plots: VALUE_HISTOGRAM,
        scale: ScaleHint::Volume,
        requires: VolumeRequirement::Real,
        placement: Placement::Either,
        smoothing: Smoothing::Windowed { period: None },
    },
];

/// Finds a descriptor by its case-insensitive identifier.
#[must_use]
pub fn descriptor(id: &str) -> Option<&'static IndicatorDescriptor> {
    DESCRIPTORS
        .iter()
        .find(|item| item.id.eq_ignore_ascii_case(id))
}

#[cfg(test)]
mod tests {
    use super::{DESCRIPTORS, ScaleHint, descriptor};

    #[test]
    fn descriptors_cover_the_ten_built_ins_once() {
        assert_eq!(DESCRIPTORS.len(), 10);
        assert!(DESCRIPTORS.iter().all(|item| descriptor(item.id).is_some()));
    }

    #[test]
    fn ratio_and_price_scales_are_declared_by_the_indicator() {
        assert!(matches!(
            descriptor("Rsi").unwrap().scale,
            ScaleHint::Ratio {
                min: 0.0,
                max: 100.0
            }
        ));
        assert!(matches!(descriptor("Ema").unwrap().scale, ScaleHint::Price));
    }

    #[test]
    fn recursive_wilder_warmup_is_deeper_than_ema_for_the_same_period() {
        let ema = descriptor("Ema").unwrap().warmup_bars(|_| Some(200));
        let rsi = descriptor("Rsi").unwrap().warmup_bars(|_| Some(200));
        assert!(rsi > ema);
    }
}
