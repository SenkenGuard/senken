use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_alerts::{AlertRecord, Comparator, Condition, IndicatorField};

use super::IndicatorSpecDto;

/// `(field, comparator, threshold)`, on the wire — mirrors
/// `senken_alerts::Condition` field-for-field. `field`/`comparator` reuse
/// that crate's own enums directly (already `Serialize`/`Deserialize`) but
/// are documented to `utoipa` as plain strings via
/// `#[schema(value_type = String)]`, the same technique [`super::GrantDto`] uses
/// for `senken_acl`'s enums and for the same reason: the orphan rule
/// forbids implementing `ToSchema` for a foreign type from here.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub(crate) struct ConditionDto {
    /// Which of the indicator's own numbers this condition reads.
    #[schema(value_type = String)]
    pub field: IndicatorField,
    /// How the field's value is compared against `threshold`.
    #[schema(value_type = String)]
    pub comparator: Comparator,
    /// The value compared against, in whatever unit `field` already
    /// reports in.
    pub threshold: f64,
}

impl From<Condition> for ConditionDto {
    fn from(condition: Condition) -> Self {
        Self {
            field: condition.field,
            comparator: condition.comparator,
            threshold: condition.threshold,
        }
    }
}

impl From<ConditionDto> for Condition {
    fn from(dto: ConditionDto) -> Self {
        Self {
            field: dto.field,
            comparator: dto.comparator,
            threshold: dto.threshold,
        }
    }
}

/// An alert row ("list, create, delete, and fired-state" — the last three fields plus `enabled` are exactly that state).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AlertDto {
    /// The alert's id.
    pub id: String,
    /// The account that owns this alert.
    pub owner_id: String,
    /// The instrument this alert leases from the subscription pool.
    pub instrument: String,
    /// The bar timeframe this alert's indicator evaluates over.
    pub timeframe: String,
    /// The indicator this alert evaluates.
    pub indicator: IndicatorSpecDto,
    /// The condition checked against the indicator's value each time a bar
    /// closes.
    pub condition: ConditionDto,
    /// Whether this alert is currently being evaluated.
    pub enabled: bool,
    /// Unix timestamp of creation.
    pub created_at: i64,
    /// Unix timestamp of the last change to this alert's own fields.
    pub updated_at: i64,
    /// When this alert last fired, if ever.
    pub last_fired_at: Option<i64>,
    /// The indicator value that triggered the most recent fire, if any.
    pub last_fired_value: Option<f64>,
    /// How many times this alert has ever fired.
    pub fire_count: u32,
}

impl From<AlertRecord> for AlertDto {
    fn from(record: AlertRecord) -> Self {
        Self {
            id: record.id.to_string(),
            owner_id: record.owner_id.to_string(),
            instrument: record.instrument.as_str().to_owned(),
            timeframe: record.timeframe.to_string(),
            indicator: record.indicator.into(),
            condition: record.condition.into(),
            enabled: record.enabled,
            created_at: record.created_at,
            updated_at: record.updated_at,
            last_fired_at: record.last_fired_at,
            last_fired_value: record.last_fired_value,
            fire_count: record.fire_count,
        }
    }
}

/// `GET /api/alerts` response body (scope reaches the query, including this `total`).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AlertsPage {
    /// The rows for this page.
    pub rows: Vec<AlertDto>,
    /// How many rows exist in total, under the same scope as `rows`.
    pub total: u64,
}

/// `POST /api/alerts` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateAlertRequest {
    /// The instrument to lease and evaluate, `source:symbol`.
    pub instrument: String,
    /// The bar timeframe to evaluate over, e.g. `"1h"`.
    pub timeframe: String,
    /// The indicator to evaluate.
    pub indicator: IndicatorSpecDto,
    /// The condition to check each time a bar closes.
    pub condition: ConditionDto,
}
