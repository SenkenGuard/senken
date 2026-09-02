//! A plugin that emits far more non-series display objects per bar than
//! any host would allow — used to prove the display-object cap this
//! workspace enforces on every dynamic indicator actually bites, and
//! rejects with a message, rather than a chart quietly losing some of its
//! objects.
use senken_plugin_api::{
    Bar, Drawable, Extend, Guest, GuestInstance, IndicatorDescriptor, LevelDrawable, OnBarResult,
    ParamValue, PriceCoord,
};

struct Overload;

impl Guest for Overload {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        IndicatorDescriptor {
            id: "DynOverload".into(),
            title: "Dynamic Overload".into(),
            short_title: "OVERLOAD".into(),
            legend: String::new(),
            params: vec![],
            plots: vec![],
        }
    }
}

struct Instance;

impl GuestInstance for Instance {
    fn new(_params: Vec<ParamValue>) -> Self {
        Instance
    }

    fn handle_bar(&self, bar: Bar) -> OnBarResult {
        // Fifty fresh levels every bar: a handful of bars is already well
        // past any sane per-indicator cap.
        let drawables = (0..50)
            .map(|i| {
                Drawable::Level(LevelDrawable {
                    price: PriceCoord::Annotation(bar.close.value as f64 + f64::from(i)),
                    extend: Extend::None,
                })
            })
            .collect();
        OnBarResult {
            plots: vec![],
            drawables,
        }
    }

    fn initialized(&self) -> bool {
        true
    }

    fn reset(&self) {}
}

senken_plugin_api::export!(Overload);
