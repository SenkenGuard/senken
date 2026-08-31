//! Bars over HTTP.
//!
//! Every handler here resolves an `instrument`/`spec` pair to a
//! `senken_series::SeriesKey` and a `senken_loader::SeriesLoader` — one per
//! registered bar source, held by `AppState::runtime` exactly the way
//! `senken-cli`'s own `bars` subcommand already resolves one — and calls
//! straight through to it. `plan()` and `ensure()` stay two different
//! endpoints: `GET /api/bars/plan` never starts work,
//! `POST /api/bars/ensure` never blocks on it. `GET /api/bars/range` is the
//! read path a chart actually renders from, going through
//! `SeriesLoader::resolve` so the memory cache, the store and aggregation
//! from a finer spec all apply exactly as built — and, since
//! `resolve` never fetches, asking for the same range twice costs nothing
//! once the first `ensure()` has written it (the "opening the
//! same range twice must issue zero venue requests").
//!
//! Every key here is built with `Origin::Derived`: a chart asks for a
//! timeframe, not for "whatever the venue itself calls this spec", so the
//! ladder is always given the chance to aggregate from a stored finer spec
//! instead of fetching the requested spec directly from the venue.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

use senken_core::{TimeRange, UnixNanos};
use senken_loader::{JobId, Priority, SeriesLoader};
use senken_marketdata::{InstrumentId, InstrumentMatch};
use senken_series::{Anchor, BarSpec, Clock, Origin, SeriesKey};

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    BarJobDto, BarRangeQuery, BarRangeResponse, BarsRequirementDto, EnsureBarsRequest,
    EnsureBarsResponse,
};

/// Resolves `instrument`/`spec` into a parsed [`InstrumentId`], [`BarSpec`]
/// and the instrument's catalog entry (needed for `ensure`'s
/// `price_scale`/`qty_scale`). `400` for anything that does not parse or
/// name a real, catalogued instrument.
pub(crate) async fn resolve_bar_target(
    state: &AppState,
    instrument: &str,
    spec: &str,
) -> Result<(InstrumentId, BarSpec, InstrumentMatch), HandlerError> {
    let id = InstrumentId::parse(instrument)
        .map_err(|source| HandlerError::BadRequest(source.to_string()))?;
    let spec: BarSpec = spec
        .parse()
        .map_err(|source: senken_series::ParseBarSpecError| {
            HandlerError::BadRequest(source.to_string())
        })?;
    let hit = state
        .runtime
        .marketdata()
        .instrument(&id)
        .await
        .map_err(|source| match source {
            // An unknown source or an id the catalog itself could not
            // parse is the caller's mistake, not this server's — the exact
            // same 400/500 split `IdentityError`'s own `HandlerError`
            // conversion draws between "malformed input" and "storage
            // failed".
            senken_marketdata::MarketDataError::UnknownSource(_)
            | senken_marketdata::MarketDataError::Id(_) => {
                HandlerError::BadRequest(source.to_string())
            }
            other => {
                tracing::error!(source = %other, "marketdata lookup failed while resolving a bars request");
                HandlerError::Internal
            }
        })?
        .ok_or_else(|| HandlerError::BadRequest(format!("no instrument `{id}`")))?;
    Ok((id, spec, hit))
}

/// The [`SeriesLoader`] registered for `id`'s source, or `400` if none is
/// (an unregistered source, or one whose plugin supports no fixed-duration
/// spec — see `senken_runtime::SeriesData::build`'s own docs).
pub(crate) fn loader_for(
    state: &AppState,
    id: &InstrumentId,
) -> Result<SeriesLoader, HandlerError> {
    state
        .runtime
        .series()
        .loader(id.source())
        .cloned()
        .ok_or_else(|| {
            HandlerError::BadRequest(format!("no bar source registered for `{}`", id.source()))
        })
}

pub(crate) fn parse_range(from: i64, to: i64) -> Result<TimeRange, HandlerError> {
    TimeRange::new(UnixNanos::from_nanos(from), UnixNanos::from_nanos(to))
        .ok_or_else(|| HandlerError::BadRequest("`from` must not be after `to`".to_owned()))
}

fn parse_priority(raw: Option<&str>) -> Result<Priority, HandlerError> {
    Ok(match raw {
        None | Some("visible") => Priority::Visible,
        Some("prefetch") => Priority::Prefetch,
        Some("background") => Priority::Background,
        Some(other) => {
            return Err(HandlerError::BadRequest(format!(
                "`{other}` is not `background`, `prefetch` or `visible`"
            )));
        }
    })
}

/// Encodes which loader minted `id` alongside the id itself, since
/// `senken_loader::JobId` is unique only within the loader that assigned it.
fn encode_job_ref(source: &str, id: JobId) -> String {
    format!("{source}:{id}")
}

/// The inverse of [`encode_job_ref`]. `400` for anything that does not
/// split into a known shape.
fn parse_job_ref(raw: &str) -> Result<(&str, JobId), HandlerError> {
    let (source, id) = raw
        .split_once(':')
        .ok_or_else(|| HandlerError::BadRequest("not a valid job reference".to_owned()))?;
    let id: JobId = id
        .parse()
        .map_err(|_| HandlerError::BadRequest("not a valid job reference".to_owned()))?;
    Ok((source, id))
}

/// `GET /api/bars/plan`: pure inspection — touches no
/// network, starts no work.
#[utoipa::path(
    get,
    path = "/api/bars/plan",
    params(
        ("instrument" = String, Query, description = "source:symbol, e.g. binance-spot:BTCUSDT"),
        ("spec" = String, Query, description = "bar timeframe, e.g. 1h"),
        ("from" = i64, Query, description = "inclusive start, unix nanoseconds"),
        ("to" = i64, Query, description = "exclusive end, unix nanoseconds"),
    ),
    responses(
        (status = 200, body = BarsRequirementDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn plan_bars(
    State(state): State<AppState>,
    Extension(_ctx): Authed,
    Query(query): Query<BarRangeQuery>,
) -> Result<Json<BarsRequirementDto>, HandlerError> {
    let (id, spec, _hit) = resolve_bar_target(&state, &query.instrument, &query.spec).await?;
    let loader = loader_for(&state, &id)?;
    let range = parse_range(query.from, query.to)?;
    let key = SeriesKey::new(id.source(), id.symbol(), Origin::Derived, spec);
    let requirement = loader.plan(&key, range, Anchor::UTC).map_err(|source| {
        tracing::error!(%source, "bars plan failed");
        HandlerError::Internal
    })?;
    Ok(Json(requirement.into()))
}

/// `GET /api/bars/range`: whatever is already resolvable
/// right now, through `SeriesLoader::resolve` — cache, then store, then
/// aggregation from a finer stored spec, and never a fetch. This is what a
/// chart actually renders bars from.
#[utoipa::path(
    get,
    path = "/api/bars/range",
    params(
        ("instrument" = String, Query, description = "source:symbol, e.g. binance-spot:BTCUSDT"),
        ("spec" = String, Query, description = "bar timeframe, e.g. 1h"),
        ("from" = i64, Query, description = "inclusive start, unix nanoseconds"),
        ("to" = i64, Query, description = "exclusive end, unix nanoseconds"),
    ),
    responses(
        (status = 200, body = BarRangeResponse),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn range_bars(
    State(state): State<AppState>,
    Extension(_ctx): Authed,
    Query(query): Query<BarRangeQuery>,
) -> Result<Json<BarRangeResponse>, HandlerError> {
    let (id, spec, hit) = resolve_bar_target(&state, &query.instrument, &query.spec).await?;
    let loader = loader_for(&state, &id)?;
    let range = parse_range(query.from, query.to)?;
    let key = SeriesKey::new(id.source(), id.symbol(), Origin::Derived, spec);
    let resolved = loader
        .resolve(&key, range, Anchor::UTC)
        .await
        .map_err(|source| {
            tracing::error!(%source, "bars resolve failed");
            HandlerError::Internal
        })?;
    let next_bar_open_at =
        senken_series::next_bucket_start(senken_loader::SystemClock.now(), spec, Anchor::UTC);
    Ok(Json(BarRangeResponse {
        bars: resolved.bars.iter().map(Into::into).collect(),
        missing: resolved.missing.into_iter().map(Into::into).collect(),
        price_scale: hit.instrument.price_scale,
        qty_scale: hit.instrument.qty_scale,
        next_bar_open_at: next_bar_open_at.as_nanos(),
    }))
}

/// `POST /api/bars/ensure`: enqueues whatever `plan()` would
/// report as missing and returns immediately with a job reference to poll
/// via `GET /api/bars/jobs/{job_id}` — never blocks on the fetch itself.
#[utoipa::path(
    post,
    path = "/api/bars/ensure",
    request_body = EnsureBarsRequest,
    responses(
        (status = 202, body = EnsureBarsResponse),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn ensure_bars(
    State(state): State<AppState>,
    Extension(_ctx): Authed,
    Json(body): Json<EnsureBarsRequest>,
) -> Result<(StatusCode, Json<EnsureBarsResponse>), HandlerError> {
    let (id, spec, hit) = resolve_bar_target(&state, &body.instrument, &body.spec).await?;
    let loader = loader_for(&state, &id)?;
    let range = parse_range(body.from, body.to)?;
    let priority = parse_priority(body.priority.as_deref())?;
    let key = SeriesKey::new(id.source(), id.symbol(), Origin::Derived, spec);
    let handle = loader.ensure(
        &key,
        range,
        Anchor::UTC,
        hit.instrument.price_scale,
        hit.instrument.qty_scale,
        priority,
    );
    let job_id = encode_job_ref(id.source(), handle.id());
    // Deliberately not awaited: `ensure` already returns a handle to a job
    // running independently, and blocking here on
    // `handle.wait()` would turn this endpoint back into the very
    // multi-minute-backfill-in-one-request shape `plan()`/`ensure()` being
    // separate calls exists to avoid.
    drop(handle);
    Ok((StatusCode::ACCEPTED, Json(EnsureBarsResponse { job_id })))
}

/// `GET /api/bars/jobs/{job_id}` — polls the job `POST /api/bars/ensure`
/// started.
#[utoipa::path(
    get,
    path = "/api/bars/jobs/{job_id}",
    responses(
        (status = 200, body = BarJobDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn bar_job_status(
    State(state): State<AppState>,
    Extension(_ctx): Authed,
    Path(job_ref): Path<String>,
) -> Result<Json<BarJobDto>, HandlerError> {
    let (source, job_id) = parse_job_ref(&job_ref)?;
    let loader = state
        .runtime
        .series()
        .loader(source)
        .cloned()
        .ok_or_else(|| {
            HandlerError::BadRequest(format!("no bar source registered for `{source}`"))
        })?;
    let snapshot = loader
        .job(job_id)
        .ok_or_else(|| HandlerError::BadRequest("no such job".to_owned()))?;
    Ok(Json(snapshot.into()))
}

#[cfg(test)]
pub(crate) mod test_support {
    //! A fake venue — a `MarketDataSource` plus a `BarSource` for one
    //! instrument — registered through a real `senken_runtime::Runtime`
    //! (never a network call), shared by this module's and
    //! `indicator_handlers`'s tests, both of which need bars to already be
    //! resolvable to exercise anything.

    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use async_trait::async_trait;
    use senken_core::TimeRange;
    use senken_marketdata::{Instrument, MarketDataSource, SourceError, SourceSymbol};
    use senken_plugin::{ActivationContext, BarSource, Plugin, PluginError, PluginManifest};
    use senken_runtime::Runtime;
    use senken_series::{Bar, BarSpec, BarUnit};

    pub(crate) const TEST_SOURCE: &str = "test-venue";
    pub(crate) const TEST_SYMBOL: &str = "BTCUSDT";

    pub(crate) fn test_instrument() -> String {
        format!("{TEST_SOURCE}:{TEST_SYMBOL}")
    }

    struct FakeMarketDataSource;

    #[async_trait]
    impl MarketDataSource for FakeMarketDataSource {
        fn id(&self) -> &str {
            TEST_SOURCE
        }

        fn name(&self) -> &'static str {
            "Test Venue"
        }

        async fn instruments(&self) -> Result<Vec<Instrument>, SourceError> {
            Ok(vec![Instrument::spot(
                TEST_SYMBOL,
                TEST_SYMBOL,
                "BTC",
                "USDT",
            )])
        }
    }

    /// One bar per minute in `range`, all identical — enough to prove
    /// coverage without caring about the actual OHLCV values.
    fn m1_bars_for(range: TimeRange) -> Vec<Bar> {
        let step = BarSpec::new(1, BarUnit::Minute)
            .duration_nanos()
            .expect("a 1-minute spec always has a fixed duration");
        let mut bars = Vec::new();
        let mut t = range.start().as_nanos();
        while t < range.end().as_nanos() {
            bars.push(Bar {
                ts_open: senken_core::UnixNanos::from_nanos(t),
                open: 100,
                high: 101,
                low: 99,
                close: 100,
                volume: 10,
                quote_volume: None,
                trade_count: None,
                taker_buy_volume: None,
            });
            t += step;
        }
        bars
    }

    /// Counts every `bars()` call — the observable proof that
    /// a second request for a range already fetched issues zero venue
    /// requests.
    pub(crate) struct FakeBarSource {
        pub(crate) calls: AtomicU32,
    }

    impl FakeBarSource {
        pub(crate) fn calls(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl BarSource for FakeBarSource {
        fn source_id(&self) -> &str {
            TEST_SOURCE
        }

        fn supported(&self) -> &[BarSpec] {
            static SPECS: std::sync::OnceLock<Vec<BarSpec>> = std::sync::OnceLock::new();
            SPECS.get_or_init(|| vec![BarSpec::new(1, BarUnit::Minute)])
        }

        fn max_rows(&self) -> usize {
            10_000
        }

        async fn bars(
            &self,
            _symbol: &SourceSymbol,
            _spec: BarSpec,
            range: TimeRange,
        ) -> Result<Vec<Bar>, SourceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(m1_bars_for(range))
        }
    }

    struct FakeVenuePlugin {
        bar_source: Arc<FakeBarSource>,
    }

    impl Plugin for FakeVenuePlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: TEST_SOURCE.to_owned(),
                name: "Test Venue".to_owned(),
                version: "0".to_owned(),
                description: String::new(),
                permissions: Vec::new(),
            }
        }

        fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
            context.register_marketdata_source(Arc::new(FakeMarketDataSource));
            context.register_bar_source(self.bar_source.clone());
            Ok(())
        }
    }

    /// Builds a [`Runtime`] with one fake venue registered, and a handle to
    /// its call counter.
    pub(crate) fn runtime_with_fake_venue(
        data_dir: &std::path::Path,
    ) -> (Runtime, Arc<FakeBarSource>) {
        let bar_source = Arc::new(FakeBarSource {
            calls: AtomicU32::new(0),
        });
        let runtime = Runtime::builder()
            .data_dir(data_dir)
            .plugin(FakeVenuePlugin {
                bar_source: bar_source.clone(),
            })
            .build()
            .expect("a single well-behaved plugin always activates");
        (runtime, bar_source)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use senken_identity::DEFAULT_ADMIN_EMAIL;

    use super::test_support::{runtime_with_fake_venue, test_instrument};
    use crate::test_support::{
        ADMIN_TEST_PASSWORD, body_json, get_auth, post_json, post_json_auth,
        serve_unfenced_test_server_with,
    };

    /// Logs into `addr` as the (already-unfenced) default admin and returns
    /// the session token — every test in this module authenticates as the
    /// seeded superadmin, since bars/indicators need no per-user ACL
    /// resource (the read-first note: market data is not
    /// ownership-scoped), only a valid, unfenced session.
    /// The countdown deadline must be a real future boundary of the
    /// requested spec: a client renders it directly as time-to-close, so
    /// "now", a past bucket, or an unaligned instant would each put a wrong
    /// clock on screen.
    #[tokio::test]
    async fn a_bar_range_reports_when_the_forming_bar_closes() {
        use senken_series::Clock;

        const FIFTEEN_MINUTES_NANOS: i64 = 15 * 60 * 1_000_000_000;

        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin_token = admin_token(addr).await;
        let instrument = test_instrument();

        let body = body_json(
            get_auth(
                format!(
                    "http://{addr}/api/bars/range?instrument={instrument}&spec=15m&from=0&to=1"
                ),
                &admin_token,
            )
            .await,
        )
        .await;

        let next = body["next_bar_open_at"].as_i64().expect("a deadline");
        let now = senken_loader::SystemClock.now().as_nanos();

        assert!(next > now, "the next bar must open in the future");
        assert!(
            next - now <= FIFTEEN_MINUTES_NANOS,
            "and no further away than one bar of the requested spec"
        );
        assert_eq!(
            next % FIFTEEN_MINUTES_NANOS,
            0,
            "a 15m boundary is a whole number of 15-minute steps from the epoch"
        );
    }

    async fn admin_token(addr: std::net::SocketAddr) -> String {
        let response = post_json(
            format!("http://{addr}/api/login"),
            serde_json::json!({ "email": DEFAULT_ADMIN_EMAIL, "password": ADMIN_TEST_PASSWORD }),
        )
        .await;
        body_json(response).await["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn requesting_the_same_range_twice_issues_zero_venue_requests_the_second_time() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin_token = admin_token(addr).await;

        let instrument = test_instrument();
        let range_from: i64 = 0;
        let range_to: i64 = 60 * 60 * 1_000_000_000; // one hour, in nanoseconds

        // --- first cycle: nothing cached yet, ensure() must fetch --------
        let ensure = post_json_auth(
            format!("http://{addr}/api/bars/ensure"),
            &admin_token,
            serde_json::json!({
                "instrument": instrument,
                "spec": "1m",
                "from": range_from,
                "to": range_to,
            }),
        )
        .await;
        assert_eq!(ensure.status(), reqwest::StatusCode::ACCEPTED);
        let job_id = body_json(ensure).await["job_id"]
            .as_str()
            .unwrap()
            .to_owned();

        wait_for_job_done(addr, &admin_token, &job_id).await;

        let first_range = body_json(
            get_auth(
                format!(
                    "http://{addr}/api/bars/range?instrument={instrument}&spec=1m&from={range_from}&to={range_to}"
                ),
                &admin_token,
            )
            .await,
        )
        .await;
        let first_bars = first_range["bars"].as_array().unwrap();
        assert!(
            !first_bars.is_empty(),
            "the first cycle must have written bars"
        );
        assert!(first_range["missing"].as_array().unwrap().is_empty());
        assert_eq!(
            first_range["price_scale"], 0,
            "the fake instrument's own default scale: a client cannot render \
             BarDto's raw integers as a price without it"
        );
        assert_eq!(first_range["qty_scale"], 0);

        // --- second cycle: the exact same range, requested again ---------
        let ensure_again = post_json_auth(
            format!("http://{addr}/api/bars/ensure"),
            &admin_token,
            serde_json::json!({
                "instrument": instrument,
                "spec": "1m",
                "from": range_from,
                "to": range_to,
            }),
        )
        .await;
        let job_id_again = body_json(ensure_again).await["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        wait_for_job_done(addr, &admin_token, &job_id_again).await;

        let second_range = body_json(
            get_auth(
                format!(
                    "http://{addr}/api/bars/range?instrument={instrument}&spec=1m&from={range_from}&to={range_to}"
                ),
                &admin_token,
            )
            .await,
        )
        .await;
        assert_eq!(
            second_range["bars"], first_range["bars"],
            "the same range must resolve to the same bars"
        );

        assert_eq!(
            bar_source.calls(),
            1,
            "opening the same range twice must issue zero venue requests the second time"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn plan_reports_the_whole_range_missing_and_starts_no_job() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin_token = admin_token(addr).await;

        let instrument = test_instrument();
        let response = get_auth(
            format!(
                "http://{addr}/api/bars/plan?instrument={instrument}&spec=1m&from=0&to=60000000000"
            ),
            &admin_token,
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["missing"].as_array().unwrap().len(), 1);
        assert_eq!(body["covered"].as_array().unwrap().len(), 0);
        assert_eq!(
            bar_source.calls(),
            0,
            "plan() must never fetch, over HTTP any more than in the loader itself"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_unregistered_source_is_a_400_not_a_500() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin_token = admin_token(addr).await;

        let response = get_auth(
            format!(
                "http://{addr}/api/bars/plan?instrument=nope:BTCUSDT&spec=1m&from=0&to=60000000000"
            ),
            &admin_token,
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_request_with_no_credentials_is_401() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();

        let instrument = test_instrument();
        let response = reqwest::get(format!(
            "http://{addr}/api/bars/range?instrument={instrument}&spec=1m&from=0&to=60000000000"
        ))
        .await
        .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

        handle.shutdown().await.unwrap();
    }

    /// Polls `GET /api/bars/jobs/{job_id}` until its phase is `"done"`.
    async fn wait_for_job_done(addr: std::net::SocketAddr, token: &str, job_id: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let response =
                    get_auth(format!("http://{addr}/api/bars/jobs/{job_id}"), token).await;
                let body = body_json(response).await;
                if body["phase"] == "done" {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("job did not finish before the test's safety timeout");
    }
}
