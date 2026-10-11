//! Post-filter for the observation date of features returned by the Vallaris hop.
//!
//! Same shape of problem as `area_filter`: the STAC `datetime` parameter filters
//! *item containers*, and a container only has to overlap the interval to match.
//! The second hop carries no datetime of its own, so a container spanning
//! 14-20 July satisfies a "1-15 July" query and drags the 16th-20th along with
//! it. Fire items happen to be one-per-day today (verified: a 3-day window
//! returned a single `acq_date`), but nothing in the API guarantees that, and
//! an absolute range is precisely where a user would notice the leak.
//!
//! Lenient in the same way: a feature is dropped only when it carries a date we
//! understand and that date falls outside the range. No date, unparseable date,
//! or a hazard whose features carry none at all (drought h3 cells) means keep.

use chrono::NaiveDate;
use serde_json::{Map, Value};

/// Observation-date properties, in the shapes seen on real responses.
/// Deliberately excludes `_createdAt`/`_updatedAt` (ingestion bookkeeping, not
/// when the event happened) and `th_date` (Buddhist Era, different arithmetic).
const DATE_FIELDS: &[&str] = &[
    "acq_date",
    "acq_datetime",
    "obs_date",
    "event_date",
    "flood_date",
    "start_date",
    "datetime",
    "date",
];

/// Values arrive as `2026-07-18T00:00:00`, `2026-07-18 00:00:00` or plain
/// `2026-07-18` — every one of them starts with an ISO date, so take that.
fn parse_date(raw: &str) -> Option<NaiveDate> {
    let head: String = raw.trim().chars().take(10).collect();
    NaiveDate::parse_from_str(&head, "%Y-%m-%d").ok()
}

fn feature_date(props: &Map<String, Value>) -> Option<NaiveDate> {
    DATE_FIELDS.iter().find_map(|k| match props.get(*k) {
        Some(Value::String(s)) => parse_date(s),
        _ => None,
    })
}

pub fn within_range(feature: &Value, from: NaiveDate, to: NaiveDate) -> bool {
    let Some(props) = feature.get("properties").and_then(Value::as_object) else {
        return true;
    };
    match feature_date(props) {
        Some(d) => d >= from && d <= to,
        None => true,
    }
}

/// Keeps only features observed within `from..=to` (inclusive), returning
/// `(kept, dropped_count)`.
pub fn filter_to_range(features: Vec<Value>, from: NaiveDate, to: NaiveDate) -> (Vec<Value>, usize) {
    let before = features.len();
    let kept: Vec<Value> = features
        .into_iter()
        .filter(|f| within_range(f, from, to))
        .collect();
    let dropped = before - kept.len();
    (kept, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn keeps_inside_drops_outside() {
        let (from, to) = (d(2026, 7, 1), d(2026, 7, 15));
        let inside = json!({"properties": {"acq_date": "2026-07-08T13:55:00"}});
        let after = json!({"properties": {"acq_date": "2026-07-18T00:00:00"}});
        let before = json!({"properties": {"acq_date": "2026-06-30T23:00:00"}});
        assert!(within_range(&inside, from, to));
        assert!(!within_range(&after, from, to));
        assert!(!within_range(&before, from, to));
    }

    #[test]
    fn boundary_days_are_inclusive() {
        let (from, to) = (d(2026, 7, 1), d(2026, 7, 15));
        assert!(within_range(&json!({"properties": {"acq_date": "2026-07-01"}}), from, to));
        assert!(within_range(
            // late on the final day — the classic off-by-one an end-of-day
            // timestamp would otherwise cut off
            &json!({"properties": {"acq_date": "2026-07-15T23:59:00"}}),
            from,
            to
        ));
    }

    #[test]
    fn undated_features_are_kept() {
        let (from, to) = (d(2026, 7, 1), d(2026, 7, 15));
        // drought h3 cells carry no observation date at all
        let cell = json!({"properties": {"dri_mean": 0, "pv_idn": "10"}});
        assert!(within_range(&cell, from, to));
        assert!(within_range(&json!({}), from, to));
    }

    #[test]
    fn ingestion_metadata_is_not_treated_as_the_event_date() {
        let (from, to) = (d(2026, 7, 1), d(2026, 7, 15));
        // _createdAt is outside the range but must not decide the verdict
        let f = json!({"properties": {"_createdAt": "2026-07-19T00:00:18.28Z"}});
        assert!(within_range(&f, from, to));
    }

    #[test]
    fn counts_what_it_drops() {
        let feats = vec![
            json!({"properties": {"acq_date": "2026-07-02"}}),
            json!({"properties": {"acq_date": "2026-07-20"}}),
            json!({"properties": {"acq_date": "2026-08-01"}}),
        ];
        let (kept, dropped) = filter_to_range(feats, d(2026, 7, 1), d(2026, 7, 15));
        assert_eq!(kept.len(), 1);
        assert_eq!(dropped, 2);
    }
}
