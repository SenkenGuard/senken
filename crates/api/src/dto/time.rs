//! The typed "when" boundary: every endpoint that accepts a point in time
//! from a caller accepts a [`TimeInputDto`], never a bare civil datetime.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_core::{CivilDateTime, IanaZone, TimeError, UnixNanos};

/// The one shape this API accepts for "when": an absolute instant, or a
/// civil (wall-clock) datetime paired with the IANA zone id it should be
/// read in. There is no third shape.
///
/// A JSON payload for the `civil` variant that omits `zone` fails to
/// deserialize naming the missing field, instead of silently falling back to
/// whatever zone the server process happens to be running in — the same
/// zone that, by coincidence, matches every developer's own machine and
/// nothing else's. See [`IanaZone`] for what happens when `zone` is present
/// but not a zone the bundled database recognises, and
/// [`senken_core::instant_from_civil`] for the documented answer to what an
/// hour that a DST transition skips or repeats means.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TimeInputDto {
    /// An absolute point in time, unambiguous by construction. Unix
    /// nanoseconds — the same wire representation [`UnixNanos`] uses
    /// everywhere else in this API.
    Instant {
        /// Nanoseconds since the Unix epoch, UTC.
        at: i64,
    },
    /// A wall-clock date and time, meaningless without `zone`.
    Civil {
        /// ISO-8601 civil datetime with no offset, e.g.
        /// `"2026-09-01T09:00:00"`.
        #[schema(value_type = String, example = "2026-09-01T09:00:00")]
        datetime: CivilDateTime,
        /// The IANA zone id `datetime` should be read in, e.g.
        /// `"America/New_York"`. Required: this is the field a caller
        /// cannot omit, on purpose.
        #[schema(value_type = String, example = "America/New_York")]
        zone: IanaZone,
    },
}

impl TryFrom<TimeInputDto> for UnixNanos {
    type Error = TimeError;

    /// The one place a [`TimeInputDto`] is resolved to the instant it
    /// names — conversion happens exactly once, here, the same way the
    /// display direction converts exactly once at its own render boundary.
    fn try_from(value: TimeInputDto) -> Result<Self, Self::Error> {
        match value {
            TimeInputDto::Instant { at } => Ok(Self::from_nanos(at)),
            TimeInputDto::Civil { datetime, zone } => {
                senken_core::instant_from_civil(datetime, &zone)
            }
        }
    }
}

/// `GET /api/me/zone` response body, and `PUT /api/me/zone`'s own response
/// echoing the value it just stored.
///
/// `zone` is nullable — not because the wire format is ambiguous, but
/// because "this account has not chosen a display zone yet" is a real,
/// distinct state (see `senken_identity::IdentityStore::get_zone`), and a
/// caller must be able to tell it apart from any actual zone rather than
/// having one silently substituted here. The browser's own detected zone is
/// this app's proposed default for that state, applied client-side — see
/// `packages/web/src/lib/time/zone.ts` — never invented at this boundary.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub(crate) struct UserZoneResponse {
    /// The caller's stored display zone, e.g. `"America/New_York"`, or
    /// `null` if none has been chosen yet.
    #[schema(value_type = Option<String>, example = "America/New_York")]
    pub zone: Option<IanaZone>,
}

/// `PUT /api/me/zone` request body.
///
/// `zone` is a plain `String` here, not an [`IanaZone`], deliberately unlike
/// [`TimeInputDto::Civil`]'s own `zone` field: an [`IanaZone`] rejects an
/// unrecognised id while axum is still deserialising the body, which — with
/// no custom rejection handler in this crate — answers `422` with axum's own
/// plain-text rejection body rather than this crate's uniform
/// [`crate::dto::ErrorBody`]. `identity_handlers::set_own_zone` instead
/// validates `zone` against [`IanaZone::new`] itself, once the body has
/// parsed, so an unrecognised id gets the same `400` + [`ErrorBody`](crate::dto::ErrorBody)
/// shape every other malformed request in this crate already does.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub(crate) struct SetZoneRequest {
    /// The IANA zone id to store as the caller's display zone, e.g.
    /// `"Europe/London"`.
    #[schema(example = "Europe/London")]
    pub zone: String,
}

#[cfg(test)]
mod tests {
    use super::{SetZoneRequest, TimeInputDto, UnixNanos, UserZoneResponse};

    #[test]
    fn set_zone_request_accepts_any_string_and_leaves_zone_validation_to_the_handler() {
        // Deliberately not an `IanaZone` field (see this struct's own doc
        // comment) — deserialising an unrecognised id must still succeed
        // here so `identity_handlers::set_own_zone` is the one place that
        // rejects it, with this crate's own `400` + `ErrorBody` shape.
        let dto: SetZoneRequest = serde_json::from_str(r#"{"zone":"Not/AZone"}"#).unwrap();
        assert_eq!(dto.zone, "Not/AZone");
    }

    #[test]
    fn set_zone_request_requires_the_zone_field() {
        let err = serde_json::from_str::<SetZoneRequest>("{}")
            .expect_err("a payload with no `zone` at all must not deserialize");
        assert!(
            err.to_string().contains("zone"),
            "expected the missing-field error to name `zone`, got: {err}"
        );
    }

    #[test]
    fn user_zone_response_serialises_an_absent_zone_as_json_null() {
        let response = UserZoneResponse { zone: None };
        assert_eq!(
            serde_json::to_string(&response).unwrap(),
            r#"{"zone":null}"#,
            "the caller must be able to tell \"not chosen yet\" apart from any real zone"
        );
    }

    #[test]
    fn instant_variant_round_trips_through_json() {
        let dto: TimeInputDto = serde_json::from_str(r#"{"kind":"instant","at":1000000000}"#)
            .expect("a well-formed instant payload must deserialize");
        let instant = UnixNanos::try_from(dto).expect("an instant conversion never fails");
        assert_eq!(instant, UnixNanos::from_nanos(1_000_000_000));
    }

    #[test]
    fn civil_variant_requires_zone_to_deserialize() {
        let err = serde_json::from_str::<TimeInputDto>(
            r#"{"kind":"civil","datetime":"2026-09-01T09:00:00"}"#,
        )
        .expect_err("a civil payload with no zone must not deserialize");
        assert!(
            err.to_string().contains("zone"),
            "expected the missing-field error to name `zone`, got: {err}"
        );
    }

    #[test]
    fn civil_variant_with_an_unknown_zone_also_fails_to_deserialize() {
        let err = serde_json::from_str::<TimeInputDto>(
            r#"{"kind":"civil","datetime":"2026-09-01T09:00:00","zone":"Not/AZone"}"#,
        )
        .expect_err("an unknown zone id must not deserialize into `IanaZone`");
        assert!(err.to_string().contains("unknown IANA time zone id"));
    }

    #[test]
    fn the_same_civil_datetime_resolves_differently_by_zone() {
        let jakarta: TimeInputDto = serde_json::from_str(
            r#"{"kind":"civil","datetime":"2026-09-01T09:00:00","zone":"Asia/Jakarta"}"#,
        )
        .unwrap();
        let london: TimeInputDto = serde_json::from_str(
            r#"{"kind":"civil","datetime":"2026-09-01T09:00:00","zone":"Europe/London"}"#,
        )
        .unwrap();
        let jakarta_instant = UnixNanos::try_from(jakarta).unwrap();
        let london_instant = UnixNanos::try_from(london).unwrap();
        assert_ne!(jakarta_instant, london_instant);
    }
}
