//! [`LayoutPreset`] — the pane-grid arrangements the chart UI offers.

use std::fmt;
use std::str::FromStr;

/// Which pane-grid arrangement a layout uses.
///
/// Matches the picker `packages/web/src/routes/charts/+page.svelte` already
/// renders against mock data (see
/// `packages/web/src/lib/mock/charts.ts`'s `LayoutId`/`LAYOUTS`): one pane,
/// two panes split horizontally or vertically, three panes split either
/// way, or a 2x2 grid of four. This crate persists whichever one a layout
/// was saved with rather than inventing a different vocabulary for the same
/// concept.
///
/// A closed enum, not `#[non_exhaustive]`: this is *this* crate's own fixed
/// vocabulary (unlike `senken_acl::Scope`, which must stay open for a future
/// caller this crate does not control), so adding a seventh arrangement is a
/// deliberate edit here, not something a downstream crate could observe
/// growing out from under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutPreset {
    /// One pane filling the layout.
    One,
    /// Two panes, side by side.
    TwoHorizontal,
    /// Two panes, stacked.
    TwoVertical,
    /// Three panes, side by side.
    ThreeHorizontal,
    /// Three panes, stacked.
    ThreeVertical,
    /// A 2x2 grid of four panes.
    Four,
}

impl LayoutPreset {
    /// How many panes this preset holds — the length
    /// [`WorkspaceStore::replace_layout`](crate::WorkspaceStore::replace_layout)
    /// requires its `panes` argument to match.
    #[must_use]
    pub fn pane_count(self) -> usize {
        match self {
            Self::One => 1,
            Self::TwoHorizontal | Self::TwoVertical => 2,
            Self::ThreeHorizontal | Self::ThreeVertical => 3,
            Self::Four => 4,
        }
    }
}

impl fmt::Display for LayoutPreset {
    /// The token stored in `layouts.preset`, matching the mock UI's
    /// `LayoutId` string values exactly (`'1' | '2h' | '2v' | '3h' | '3v' |
    /// '4'`) so a future client speaking that vocabulary needs no
    /// translation layer.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::One => "1",
            Self::TwoHorizontal => "2h",
            Self::TwoVertical => "2v",
            Self::ThreeHorizontal => "3h",
            Self::ThreeVertical => "3v",
            Self::Four => "4",
        })
    }
}

/// [`LayoutPreset::from_str`] rejects anything other than the six documented
/// tokens.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a known layout preset")]
pub struct ParseLayoutPresetError(pub(crate) String);

impl FromStr for LayoutPreset {
    type Err = ParseLayoutPresetError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "1" => Self::One,
            "2h" => Self::TwoHorizontal,
            "2v" => Self::TwoVertical,
            "3h" => Self::ThreeHorizontal,
            "3v" => Self::ThreeVertical,
            "4" => Self::Four,
            other => return Err(ParseLayoutPresetError(other.to_owned())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::LayoutPreset;

    #[test]
    fn every_preset_round_trips_through_display_and_from_str() {
        for preset in [
            LayoutPreset::One,
            LayoutPreset::TwoHorizontal,
            LayoutPreset::TwoVertical,
            LayoutPreset::ThreeHorizontal,
            LayoutPreset::ThreeVertical,
            LayoutPreset::Four,
        ] {
            let rendered = preset.to_string();
            assert_eq!(rendered.parse::<LayoutPreset>().unwrap(), preset);
        }
    }

    #[test]
    fn display_matches_the_mock_uis_layout_id_tokens() {
        assert_eq!(LayoutPreset::One.to_string(), "1");
        assert_eq!(LayoutPreset::TwoHorizontal.to_string(), "2h");
        assert_eq!(LayoutPreset::TwoVertical.to_string(), "2v");
        assert_eq!(LayoutPreset::ThreeHorizontal.to_string(), "3h");
        assert_eq!(LayoutPreset::ThreeVertical.to_string(), "3v");
        assert_eq!(LayoutPreset::Four.to_string(), "4");
    }

    #[test]
    fn pane_count_matches_each_presets_grid() {
        assert_eq!(LayoutPreset::One.pane_count(), 1);
        assert_eq!(LayoutPreset::TwoHorizontal.pane_count(), 2);
        assert_eq!(LayoutPreset::TwoVertical.pane_count(), 2);
        assert_eq!(LayoutPreset::ThreeHorizontal.pane_count(), 3);
        assert_eq!(LayoutPreset::ThreeVertical.pane_count(), 3);
        assert_eq!(LayoutPreset::Four.pane_count(), 4);
    }

    #[test]
    fn from_str_rejects_an_unknown_token() {
        assert!("5".parse::<LayoutPreset>().is_err());
        assert!("".parse::<LayoutPreset>().is_err());
    }
}
