//! Rendering instruments and bar-backfill progress for a terminal.
//!
//! Search prints one line per hit, so the contract terms are compressed
//! into a short tag; `instrument` prints one hit in full, where every term
//! gets its own line. `bars` prints a preflight line, one line
//! per meaningful progress change, and a final outcome line.

use std::io::{self, Write};
use std::time::Duration;

use senken_core::UnixNanos;
use senken_loader::{JobOutcome, Phase, Requirement};
use senken_marketdata::{
    Contract, Instrument, InstrumentKind, InstrumentMatch, InstrumentPage, OptionRight, Settlement,
    format_scaled,
};

/// A short, fixed-width description of what an instrument *is*:
/// `spot`, `perp lin`, `fut inv`, `opt qnt`. Without it a perpetual and a
/// spot pair on the same venue render identically.
#[must_use]
pub(crate) fn kind_tag(instrument: &Instrument) -> String {
    let kind = match instrument.kind {
        InstrumentKind::Spot => "spot",
        InstrumentKind::Perpetual => "perp",
        InstrumentKind::Future => "fut",
        InstrumentKind::Option => "opt",
        _ => "?",
    };
    match instrument.contract.as_ref().map(|c| c.settlement) {
        None => kind.to_owned(),
        Some(settlement) => format!("{kind} {}", settlement_tag(settlement)),
    }
}

fn settlement_tag(settlement: Settlement) -> &'static str {
    match settlement {
        Settlement::Linear => "lin",
        Settlement::Inverse => "inv",
        Settlement::Quanto => "qnt",
        _ => "?",
    }
}

/// An expiry instant as RFC 3339 UTC.
fn expiry_text(expiry: UnixNanos) -> String {
    expiry.to_string()
}

/// One search hit, one line.
pub(crate) fn hit(out: &mut impl Write, hit: &InstrumentMatch) -> io::Result<()> {
    let i = &hit.instrument;
    writeln!(
        out,
        "{:<30} {:>7}/{:<7} {:<9} tick={:<12} step={:<12} {:?}",
        hit.id,
        i.base,
        i.quote,
        kind_tag(i),
        format_scaled(i.tick_size, i.price_scale),
        format_scaled(i.step_size, i.qty_scale),
        i.status,
    )
}

/// A page of hits, plus the summary line.
pub(crate) fn page(out: &mut impl Write, page: &InstrumentPage) -> io::Result<()> {
    for entry in &page.matches {
        hit(out, entry)?;
    }
    if !page.failures.is_empty() {
        // Flush so the stderr lines land after the rows, not before them.
        out.flush()?;
        for failure in &page.failures {
            eprintln!("{}: {:#}", failure.source_id, failure.error);
        }
    }
    let shown = page.matches.len();
    let last = page.offset + shown;
    writeln!(
        out,
        "{}–{} of {}{}",
        (page.offset + 1).min(last),
        last,
        page.total_matched,
        if page.has_more() { " (more)" } else { "" }
    )
}

/// One instrument in full: every term the model carries, one per line.
pub(crate) fn detail(out: &mut impl Write, hit: &InstrumentMatch) -> io::Result<()> {
    let i = &hit.instrument;
    writeln!(out, "{}", hit.id)?;
    writeln!(out, "  name          {}", i.name)?;
    writeln!(
        out,
        "  venue         {} ({})",
        hit.source_name,
        hit.source_id()
    )?;
    writeln!(out, "  venue symbol  {}", i.source_symbol)?;
    writeln!(out, "  pair          {} / {}", i.base, i.quote)?;
    writeln!(out, "  kind          {:?}", i.kind)?;
    writeln!(out, "  status        {:?}", i.status)?;
    writeln!(
        out,
        "  price         tick {} at {} dp",
        format_scaled(i.tick_size, i.price_scale),
        i.price_scale
    )?;
    writeln!(
        out,
        "  quantity      step {} at {} dp",
        format_scaled(i.step_size, i.qty_scale),
        i.qty_scale
    )?;

    if let Some(contract) = &i.contract {
        contract_detail(out, contract)?;
    }
    Ok(())
}

fn contract_detail(out: &mut impl Write, contract: &Contract) -> io::Result<()> {
    writeln!(
        out,
        "  settlement    {} in {}",
        match contract.settlement {
            Settlement::Linear => "linear",
            Settlement::Inverse => "inverse",
            Settlement::Quanto => "quanto",
            _ => "unknown",
        },
        contract.settle
    )?;
    writeln!(
        out,
        "  contract size {}",
        format_scaled(contract.contract_size, contract.size_scale)
    )?;
    match contract.expiry {
        Some(expiry) => writeln!(out, "  expiry        {}", expiry_text(expiry))?,
        None => writeln!(out, "  expiry        never (perpetual)")?,
    }
    if let Some(terms) = &contract.option {
        writeln!(
            out,
            "  option        {} struck at {}",
            match terms.right {
                OptionRight::Call => "call",
                OptionRight::Put => "put",
                _ => "unknown",
            },
            format_scaled(terms.strike, terms.strike_scale)
        )?;
    }
    Ok(())
}

/// A `plan()` preflight for a bar backfill: what is missing,
/// before anything is fetched.
pub(crate) fn bars_plan(out: &mut impl Write, requirement: &Requirement) -> io::Result<()> {
    if requirement.missing.is_empty() {
        return writeln!(out, "already covered — nothing to fetch");
    }
    write!(
        out,
        "missing {} chunk{} (~{} bars)",
        requirement.chunks,
        if requirement.chunks == 1 { "" } else { "s" },
        requirement.estimated_bars,
    )?;
    match requirement.estimate {
        Some(estimate) => writeln!(out, ", est. {}", human_duration(estimate)),
        None => writeln!(out),
    }
}

/// One line of backfill progress, taken from a [`JobSnapshot`]'s fields
/// rather than the snapshot itself — [`senken_loader::JobId`] has no public
/// constructor, so a whole fabricated `JobSnapshot` is not something a test
/// in this crate can build; every value this function actually renders is
/// passed explicitly instead. The loader reports chunks and bars, never a
/// percentage — percent is a presentation concern, so it is
/// computed here.
pub(crate) fn bars_progress(
    out: &mut impl Write,
    phase: Phase,
    chunks_done: u32,
    chunks_total: u32,
    bars_written: u64,
    last_error: Option<&str>,
) -> io::Result<()> {
    // `checked_div` folds in the "no chunks planned yet" case (division by
    // zero) and a hypothetical `chunks_done * 100` overflow into the same
    // `None` arm, both reported as a plain 100% rather than panicking.
    let percent = chunks_done
        .checked_mul(100)
        .and_then(|scaled| scaled.checked_div(chunks_total))
        .unwrap_or(100);
    write!(
        out,
        "{:<12} chunk {chunks_done}/{chunks_total}  bars {bars_written}  {percent:>3}%",
        phase_label(phase),
    )?;
    // A `429` being retried is `last_error`, not a failure — it
    // is shown as exactly that: still in progress, never as an error.
    match last_error {
        Some(error) => writeln!(out, "  retrying: {error}"),
        None => writeln!(out),
    }
}

fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Queued => "queued",
        Phase::Downloading => "downloading",
        Phase::Writing => "writing",
        Phase::Aggregating => "aggregating",
        Phase::Done => "done",
    }
}

/// The final line once a backfill job reaches a terminal state.
pub(crate) fn bars_outcome(out: &mut impl Write, outcome: &JobOutcome) -> io::Result<()> {
    match outcome {
        JobOutcome::Completed => writeln!(out, "completed"),
        JobOutcome::Cancelled => writeln!(out, "cancelled"),
        JobOutcome::Failed(error) => writeln!(out, "failed: {error}"),
    }
}

/// A rough, human `<minutes>m<seconds>s` (or plain `<seconds>s`) rendering
/// of an estimate. Not [`Duration`]'s own `Debug`, which is sub-second
/// precise — more precision than an estimate derived from measured
/// throughput deserves.
fn human_duration(estimate: Duration) -> String {
    let secs = estimate.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::{detail, expiry_text, hit, kind_tag};
    use senken_core::UnixNanos;
    use senken_marketdata::{
        Contract, Instrument, InstrumentId, InstrumentKind, InstrumentMatch, InstrumentStatus,
        OptionRight, Settlement,
    };
    use std::sync::Arc;

    fn matched(instrument: Instrument) -> InstrumentMatch {
        InstrumentMatch {
            id: InstrumentId::new("venue", &instrument.symbol).unwrap(),
            source_name: Arc::from("Venue"),
            instrument,
        }
    }

    fn spot() -> Instrument {
        Instrument::spot("BTCUSDT", "BTC-USDT", "BTC", "USDT")
            .with_status(InstrumentStatus::Trading)
            .with_price_increment((2, 1))
            .with_qty_increment((8, 1))
    }

    fn perpetual(settlement: Settlement, settle: &str) -> Instrument {
        Instrument::derivative(
            "BTCUSD",
            "BTC-USD-SWAP",
            "BTC",
            "USD",
            InstrumentKind::Perpetual,
            Contract::new(settle, settlement).with_contract_size(0, 100),
        )
        .with_status(InstrumentStatus::Trading)
        .with_price_increment((1, 1))
        .with_qty_increment((1, 1))
    }

    fn render(f: impl Fn(&mut Vec<u8>) -> std::io::Result<()>) -> String {
        let mut buffer = Vec::new();
        f(&mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }

    #[test]
    fn a_tag_separates_the_kinds_that_used_to_look_alike() {
        assert_eq!(kind_tag(&spot()), "spot");
        assert_eq!(kind_tag(&perpetual(Settlement::Linear, "USDT")), "perp lin");
        assert_eq!(kind_tag(&perpetual(Settlement::Inverse, "BTC")), "perp inv");
        assert_eq!(kind_tag(&perpetual(Settlement::Quanto, "XBT")), "perp qnt");
    }

    #[test]
    fn a_search_row_says_what_the_instrument_is() {
        let line = render(|out| hit(out, &matched(perpetual(Settlement::Inverse, "BTC"))));
        assert!(line.contains("perp inv"), "{line}");
        assert!(line.contains("BTC/USD"), "{line}");
        assert!(line.contains("tick=0.1"), "{line}");
    }

    #[test]
    fn a_spot_detail_has_no_contract_lines() {
        let text = render(|out| detail(out, &matched(spot())));
        assert!(text.contains("kind          Spot"), "{text}");
        assert!(!text.contains("settlement"), "spot settles nothing: {text}");
    }

    #[test]
    fn a_perpetual_detail_says_it_never_expires() {
        let text = render(|out| detail(out, &matched(perpetual(Settlement::Inverse, "BTC"))));
        assert!(text.contains("settlement    inverse in BTC"), "{text}");
        assert!(text.contains("contract size 100"), "{text}");
        assert!(text.contains("never (perpetual)"), "{text}");
    }

    #[test]
    fn an_option_detail_shows_its_strike_and_expiry() {
        let call = Instrument::derivative(
            "BTCUSD26083070000C",
            "BTC-USD-260830-70000-C",
            "BTC",
            "USD",
            InstrumentKind::Option,
            Contract::new("BTC", Settlement::Inverse)
                .with_expiry(UnixNanos::from_millis(1_788_076_800_000).unwrap())
                .with_option(OptionRight::Call, 0, 70_000),
        );

        let call = matched(call);
        let text = render(|out| detail(out, &call));
        assert!(text.contains("call struck at 70000"), "{text}");
        assert!(text.contains("expiry        2026-"), "{text}");
    }

    #[test]
    fn expiry_text_renders_the_full_unix_nanos_range_without_panicking() {
        // Unlike the old millisecond `i64` this replaced, every `UnixNanos`
        // value is already valid by construction — `from_millis`/`from_secs`
        // reject overflow at the call site — so rendering it can
        // never fail; there is no `?` case left to test for.
        assert!(expiry_text(UnixNanos::from_nanos(0)).starts_with("1970-"));
        assert!(!expiry_text(UnixNanos::from_nanos(i64::MAX)).is_empty());
        assert!(!expiry_text(UnixNanos::from_nanos(i64::MIN)).is_empty());
    }

    mod bars {
        use super::super::{bars_outcome, bars_plan, bars_progress, human_duration};
        use senken_core::{TimeRange, UnixNanos};
        use senken_loader::{JobOutcome, LoadError, Phase, Requirement};
        use senken_series::{BarSpec, BarUnit, Origin, SeriesKey};
        use std::time::Duration;

        fn range() -> TimeRange {
            TimeRange::new(UnixNanos::EPOCH, UnixNanos::from_secs(3_600).unwrap()).unwrap()
        }

        fn key() -> SeriesKey {
            SeriesKey::new(
                "okx-spot",
                "BTCUSDT",
                Origin::Venue,
                BarSpec::new(1, BarUnit::Minute),
            )
        }

        fn requirement(missing: Vec<TimeRange>, estimate: Option<Duration>) -> Requirement {
            let chunks = u32::try_from(missing.len()).unwrap();
            Requirement {
                key: key(),
                range: range(),
                covered: Vec::new(),
                missing,
                chunks,
                estimated_bars: 60,
                estimate,
            }
        }

        fn render(f: impl Fn(&mut Vec<u8>) -> std::io::Result<()>) -> String {
            let mut buffer = Vec::new();
            f(&mut buffer).unwrap();
            String::from_utf8(buffer).unwrap()
        }

        #[test]
        fn a_fully_covered_plan_says_nothing_to_fetch() {
            let text = render(|out| bars_plan(out, &requirement(Vec::new(), None)));
            assert_eq!(text, "already covered — nothing to fetch\n");
        }

        #[test]
        fn a_missing_plan_reports_chunks_and_bars_not_percent() {
            let text = render(|out| bars_plan(out, &requirement(vec![range()], None)));
            assert!(text.contains("missing 1 chunk"), "{text}");
            assert!(text.contains("~60 bars"), "{text}");
            assert!(
                !text.contains('%'),
                "a plan has no percent to report: {text}"
            );
        }

        #[test]
        fn a_plan_with_a_measured_estimate_shows_it() {
            let text = render(|out| {
                bars_plan(
                    out,
                    &requirement(vec![range()], Some(Duration::from_secs(90))),
                )
            });
            assert!(text.contains("est. 1m30s"), "{text}");
        }

        #[test]
        fn progress_computes_percent_from_chunk_counts() {
            let text = render(|out| bars_progress(out, Phase::Downloading, 3, 12, 42, None));
            assert!(text.contains("chunk 3/12"), "{text}");
            assert!(text.contains("bars 42"), "{text}");
            assert!(text.contains(" 25%"), "{text}");
        }

        #[test]
        fn a_retrying_chunk_is_shown_as_retrying_not_as_a_failure() {
            let text = render(|out| {
                bars_progress(
                    out,
                    Phase::Downloading,
                    1,
                    4,
                    10,
                    Some("429 too many requests"),
                )
            });
            assert!(text.contains("retrying: 429 too many requests"), "{text}");
            assert!(!text.contains("failed"), "a retry is not a failure: {text}");
        }

        #[test]
        fn an_empty_plan_reports_full_percent_rather_than_dividing_by_zero() {
            let text = render(|out| bars_progress(out, Phase::Done, 0, 0, 0, None));
            assert!(text.contains("100%"), "{text}");
        }

        #[test]
        fn outcomes_render_distinctly() {
            assert_eq!(
                render(|out| bars_outcome(out, &JobOutcome::Completed)),
                "completed\n"
            );
            assert_eq!(
                render(|out| bars_outcome(out, &JobOutcome::Cancelled)),
                "cancelled\n"
            );
            let text = render(|out| bars_outcome(out, &JobOutcome::Failed(LoadError::JobPanicked)));
            assert!(text.starts_with("failed: "), "{text}");
        }

        #[test]
        fn human_duration_switches_to_minutes_past_sixty_seconds() {
            assert_eq!(human_duration(Duration::from_secs(45)), "45s");
            assert_eq!(human_duration(Duration::from_secs(90)), "1m30s");
            assert_eq!(human_duration(Duration::from_secs(3_661)), "61m01s");
        }
    }
}
