use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use futures::stream::{self, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};

use crate::analysis::stats;
use crate::error::AppError;
use crate::gazetteer::lookup::{lookup, LookupResult};
use crate::lang::{hazard_label, Lang};
use crate::gistda::{area_filter, stac, time_filter, vallaris};
use crate::llm::analysis_prompt::analysis_prompt;
use crate::models::{
    AdminLevel, DroughtProduct, ExtractedQuery, GazetteerEntry, Hazard, QueryRequest,
};
use crate::AppState;

/// Walks up `parent_area_idn` links to find the ancestor province's English name,
/// used to disambiguate candidates in the clarification response (README §6).
fn find_province_name(gazetteer: &[GazetteerEntry], entry: &GazetteerEntry) -> Option<String> {
    if entry.level == AdminLevel::Province {
        return Some(entry.name_en.clone());
    }
    let mut current = entry;
    loop {
        let parent_id = current.parent_area_idn?;
        let parent = gazetteer.iter().find(|e| e.area_idn == parent_id)?;
        if parent.level == AdminLevel::Province {
            return Some(parent.name_en.clone());
        }
        current = parent;
    }
}

fn candidate_json(gazetteer: &[GazetteerEntry], e: &GazetteerEntry) -> Value {
    json!({
        "area_idn": e.area_idn,
        "level": e.level,
        "name_th": e.name_th,
        "name_en": e.name_en,
        "province": find_province_name(gazetteer, e),
    })
}

/// What the question was missing, so the client can keep the conversation
/// going instead of making the user retype everything.
///
/// `Location` is the important one: the extractor usually understood the hazard
/// and the time window perfectly ("วันนี้มีน้ำท่วมไหม" parses to flood / 1 day)
/// and only lacks a place. Telling the client that lets the next message be
/// treated as the missing place and merged into the original question.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum Missing {
    Location,
    Hazard,
}

fn clarification(
    message: String,
    candidates: Vec<Value>,
    missing: Option<Missing>,
) -> Json<Value> {
    Json(json!({
        "needs_clarification": true,
        "message": message,
        "candidates": candidates,
        "missing": missing,
    }))
}

/// Computes a bbox by walking every GeoJSON geometry's `coordinates` tree and
/// taking the min/max of every [lon, lat] position found. Generic across
/// Point/Polygon/MultiPolygon since flood/fire/drought each use a different
/// geometry type (README §9 wants `fitBounds()` from real item geometry, not
/// the world-spanning placeholder bbox some STAC items carry).
fn compute_bbox(features: &[Value]) -> Option<[f64; 4]> {
    fn walk(v: &Value, acc: &mut (f64, f64, f64, f64, bool)) {
        match v {
            Value::Array(arr) => {
                if arr.len() >= 2 && arr.iter().all(|x| x.is_number()) {
                    if let (Some(lon), Some(lat)) = (arr[0].as_f64(), arr[1].as_f64()) {
                        acc.0 = acc.0.min(lon);
                        acc.1 = acc.1.min(lat);
                        acc.2 = acc.2.max(lon);
                        acc.3 = acc.3.max(lat);
                        acc.4 = true;
                    }
                    return;
                }
                for item in arr {
                    walk(item, acc);
                }
            }
            Value::Object(map) => {
                if let Some(coords) = map.get("coordinates") {
                    walk(coords, acc);
                }
            }
            _ => {}
        }
    }

    let mut acc = (f64::MAX, f64::MAX, f64::MIN, f64::MIN, false);
    for f in features {
        if let Some(geom) = f.get("geometry") {
            walk(geom, &mut acc);
        }
    }
    acc.4.then_some([acc.0, acc.1, acc.2, acc.3])
}

/// Longest range we will search. Guards against a model hallucinating a decade
/// and against a typo'd Buddhist Era year quietly asking for 543 years of data.
const MAX_RANGE_DAYS: i64 = 366;

/// Second-hop requests in flight at once. High enough that a year's worth of
/// items resolves in seconds rather than minutes, low enough to stay a
/// well-behaved client of a shared government gateway.
const ITEM_CONCURRENCY: usize = 16;

pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// Human-readable, used in the Thai summary so the user can see which
    /// period actually got searched — including when we ignored bad dates.
    pub label: String,
}

/// Prefers an explicit `date_from`/`date_to` pair and falls back to the
/// relative window. The absolute path is validated rather than trusted: the
/// dates come from an LLM, so anything unparseable, reversed, or absurdly long
/// silently degrades to the relative window instead of erroring at the user.
fn resolve_time_range(
    extracted: &ExtractedQuery,
    now: DateTime<Utc>,
    lang: Lang,
) -> TimeRange {
    let days = extracted.time_window_days.clamp(1, MAX_RANGE_DAYS as u32) as i64;
    let relative = || TimeRange {
        start: now - Duration::days(days),
        end: now,
        label: match lang {
            Lang::Th => format!("{days} วันที่ผ่านมา"),
            Lang::En => format!("the last {days} days"),
        },
    };

    let (Some(from), Some(to)) = (&extracted.date_from, &extracted.date_to) else {
        return relative();
    };
    let (Ok(from), Ok(to)) = (
        NaiveDate::parse_from_str(from, "%Y-%m-%d"),
        NaiveDate::parse_from_str(to, "%Y-%m-%d"),
    ) else {
        tracing::warn!(?extracted.date_from, ?extracted.date_to, "unparseable dates, using relative window");
        return relative();
    };
    if to < from || (to - from).num_days() > MAX_RANGE_DAYS {
        tracing::warn!(%from, %to, "implausible date range, using relative window");
        return relative();
    }

    // Inclusive of the whole end day — "1-15 July" must contain the 15th, and a
    // bare date parses to 00:00, which would cut that day off entirely.
    let start = from.and_hms_opt(0, 0, 0).unwrap().and_utc();
    let end = to.and_hms_opt(23, 59, 59).unwrap().and_utc();
    TimeRange {
        start,
        // Nothing exists in the future; asking for it just widens the window.
        end: end.min(now),
        label: match lang {
            Lang::Th => format!("{from} ถึง {to}"),
            Lang::En => format!("{from} to {to}"),
        },
    }
}

/// Asks the model to explain the computed statistics. Only the aggregate is
/// sent — a year of hotspots is thousands of features, and shipping them would
/// be both expensive and an invitation for the model to miscount.
async fn narrate_stats(
    state: &AppState,
    computed: &stats::Stats,
    resolved: &GazetteerEntry,
    hazard: Hazard,
    period: &str,
    lang: Lang,
) -> Option<String> {
    // The keys are written in the answer's language too. A model handed Thai
    // field names tends to answer in Thai however the prompt is worded.
    let payload = match lang {
        Lang::Th => json!({
            "พื้นที่": resolved.name_th,
            "ระดับพื้นที่": resolved.level,
            "ภัยพิบัติ": hazard_label(hazard, lang),
            "ช่วงเวลา": period,
            "สถิติ": computed,
        }),
        Lang::En => json!({
            "area": resolved.name_en,
            "area_level": resolved.level,
            "hazard": hazard_label(hazard, lang),
            "period": period,
            "stats": computed,
        }),
    };
    let user = serde_json::to_string(&payload).ok()?;
    state.llm.narrate(analysis_prompt(lang), &user).await
}

pub async fn handle_query(
    State(state): State<AppState>,
    Json(req): Json<QueryRequest>,
) -> Result<Json<Value>, AppError> {
    // Read from the user's own words, before the LLM is involved: every string
    // below is written in this language, and nothing may mix the two.
    let lang = Lang::detect(&req.text);
    let extracted = state.llm.extract(&req.text).await?;

    // An unsupported hazard is a question we cannot answer at all, so it is
    // settled before anything else.
    let Some(hazard) = extracted.hazard else {
        return Ok(clarification(
            lang.pick(
                "ระบบนี้ครอบคลุมเฉพาะน้ำท่วม ไฟป่า และภัยแล้ง ยังไม่รองรับภัยพิบัติประเภทอื่น",
                "This system covers flood, fire and drought only — other hazards are not supported.",
            )
            .to_string(),
            vec![],
            Some(Missing::Hazard),
        ));
    };

    if extracted.needs_clarification {
        // Ask only for what is actually missing. Re-asking for the hazard when
        // the user already said "น้ำท่วม" reads as if nothing was understood,
        // and leaves them no idea what to type next.
        if extracted.location_text.trim().is_empty() {
            let hazard = hazard_label(hazard, lang);
            return Ok(clarification(
                match lang {
                    Lang::Th => format!(
                        "เข้าใจว่าต้องการข้อมูล{hazard} แต่ยังไม่ได้ระบุพื้นที่ — พิมพ์ชื่อจังหวัด อำเภอ หรือตำบลได้เลย เช่น \"ขอนแก่น\" หรือ \"อำเภอหาดใหญ่\""
                    ),
                    Lang::En => format!(
                        "Got it: {hazard}. Which area? Type a province, district or subdistrict — for example \"Khon Kaen\" or \"Amphoe Hat Yai\"."
                    ),
                },
                vec![],
                Some(Missing::Location),
            ));
        }
        return Ok(clarification(
            lang.pick(
                "รองรับเฉพาะน้ำท่วม ไฟป่า และภัยแล้ง กรุณาระบุประเภทภัยพิบัติให้ชัดเจนขึ้น",
                "Only flood, fire and drought are supported. Please state which one you mean.",
            )
            .to_string(),
            vec![],
            Some(Missing::Hazard),
        ));
    }

    // An explicit pick bypasses the name lookup entirely: there is nothing to
    // disambiguate once the exact area is known.
    let picked = req.area_idn.and_then(|id| {
        let found = state.gazetteer.iter().find(|e| e.area_idn == id).cloned();
        if found.is_none() {
            tracing::warn!(area_idn = id, "picked area not in gazetteer, falling back to lookup");
        }
        found
    });

    let resolved = match picked.map(LookupResult::Found).unwrap_or_else(|| {
        lookup(&state.gazetteer, &extracted.location_text)
    }) {
        LookupResult::Found(entry) => entry,
        LookupResult::Ambiguous(entries) => {
            let candidates: Vec<Value> = entries
                .iter()
                .take(10)
                .map(|e| candidate_json(&state.gazetteer, e))
                .collect();
            return Ok(clarification(
                match lang {
                    Lang::Th => format!(
                        "พบชื่อ \"{}\" หลายแห่ง กรุณาระบุให้ชัดเจนขึ้น",
                        extracted.location_text
                    ),
                    Lang::En => format!(
                        "\"{}\" matches several places. Please be more specific.",
                        extracted.location_text
                    ),
                },
                candidates,
                None,
            ));
        }
        LookupResult::NotFound => {
            return Ok(clarification(
                match lang {
                    Lang::Th => format!(
                        "ไม่พบสถานที่ \"{}\" กรุณาระบุชื่อสถานที่ใหม่",
                        extracted.location_text
                    ),
                    Lang::En => format!(
                        "Could not find a place called \"{}\". Try another name.",
                        extracted.location_text
                    ),
                },
                vec![],
                Some(Missing::Location),
            ));
        }
    };

    let Some(area_bbox) = resolved.bbox else {
        return Ok(clarification(
            match lang {
                Lang::Th => format!("ไม่พบขอบเขตพื้นที่ของ \"{}\"", extracted.location_text),
                Lang::En => format!("No boundary on file for \"{}\".", extracted.location_text),
            },
            vec![],
            Some(Missing::Location),
        ));
    };

    // Check the credential right before hitting GISTDA (README §2 step 6 / §4.3).
    // With an api_key this can only fail when it is unset; with a token it also
    // catches the ~1 hour expiry, so we fail fast instead of eating a bare 401.
    if !state.config.gistda_credential_valid() {
        return Err(AppError::TokenExpired);
    }
    let (auth_kind, auth_value) = state.config.gistda_auth();

    let now = Utc::now();
    let TimeRange { start, end, label } = resolve_time_range(&extracted, now, lang);
    let datetime_interval = format!(
        "{}/{}",
        start.format("%Y-%m-%dT%H:%M:%SZ"),
        end.format("%Y-%m-%dT%H:%M:%SZ"),
    );

    // A bare drought question means the drought index itself. This default is
    // applied here, not in the prompt, because it is a fixed rule rather than a
    // judgement: the smaller local model (gemma3:4b) resolved "โคราชแล้งไหม"
    // and "is Korat in drought?" correctly but left dataset null every single
    // time, which silently widened the search to soil moisture and NDWI.
    // Deterministic rules belong in code, where no model can forget them.
    let dataset = extracted.dataset.or(match hazard {
        Hazard::Drought => Some(DroughtProduct::DriPlus),
        _ => None,
    });

    let search_result = stac::search_all(
        &state.http,
        &state.config.gistda_gateway_host,
        hazard,
        area_bbox,
        &datetime_interval,
        (auth_kind.param(), auth_value),
    )
    .await?;

    let time_range_json = json!({
        "from": start.format("%Y-%m-%d").to_string(),
        "to": end.format("%Y-%m-%d").to_string(),
        "label": label,
    });

    let resolved_area = json!({
        "area_idn": resolved.area_idn,
        "level": resolved.level,
        "name_th": resolved.name_th,
        "name_en": resolved.name_en,
        // The boundary tileset only goes down to amphoe, so a tambon result
        // needs its parent's id for the client to have anything to outline.
        "parent_area_idn": resolved.parent_area_idn,
    });

    if search_result.matched == 0 || search_result.items.is_empty() {
        return Ok(Json(json!({
            "needs_clarification": false,
            "hazard": hazard,
            "location_text": extracted.location_text,
            "resolved_area": resolved_area,
            "time_window_days": extracted.time_window_days,
            "time_range": time_range_json,
            "summary": match lang {
                Lang::Th => format!(
                    "ไม่พบข้อมูล{}ใน{} ช่วง {}",
                    hazard_label(hazard, lang), resolved.name_th, label
                ),
                Lang::En => format!(
                    "No {} data found in {} during {}",
                    hazard_label(hazard, lang), resolved.name_en, label
                ),
            },
            "geojson": { "type": "FeatureCollection", "features": [] },
            "bbox": Value::Null,
            "dataset": dataset,
            "stats": stats::compute(&[], &resolved, hazard, lang),
            "analysis": Value::Null,
            "lang": lang,
        })));
    }

    // Double-hop per item: STAC search already happened above, now resolve
    // each item's asset.data.href into real geometry (README §2 step 8).
    // Raw satellite scenes (Sentinel-1 etc.) come back from the same search but
    // carry no `data` asset — nothing to resolve, so drop them before fanning out.
    // Owned (id, href) pairs rather than borrowed items: an async closure over
    // a reference cannot satisfy the higher-ranked lifetime `buffer_unordered`
    // demands, and cloning two strings per item is not worth fighting over.
    // When a drought product is selected, drop the other two here rather than
    // after fetching — they are separate collections behind separate hrefs, so
    // filtering now saves the whole second hop for them.
    let product_prefix = dataset.map(|d| d.item_prefix());
    let fetchable: Vec<(String, String)> = search_result
        .items
        .iter()
        .filter(|i| product_prefix.is_none_or(|p| i.id.starts_with(p)))
        .filter_map(|i| {
            i.assets
                .data
                .as_ref()
                .map(|a| (i.id.clone(), a.href.clone()))
        })
        .collect();

    // One request per item, in flight together. A year of hotspots is ~750
    // items; sequentially that is minutes of wall-clock, which is unusable
    // live. Bounded so we pipeline the gateway rather than flood it.
    let http = &state.http;
    let results: Vec<Result<Vec<Value>, (String, AppError)>> = stream::iter(fetchable)
        .map(|(id, href)| async move {
            vallaris::fetch_features(http, &href, area_bbox)
                .await
                .map_err(|e| (id, e))
        })
        .buffer_unordered(ITEM_CONCURRENCY)
        .collect()
        .await;

    let mut all_features: Vec<Value> = Vec::new();
    let mut failed_items = 0usize;
    for result in results {
        match result {
            Ok(features) => all_features.extend(features),
            Err((item_id, e)) => {
                failed_items += 1;
                tracing::warn!(error = %e, %item_id, "failed to fetch asset features, skipping item");
            }
        }
    }

    // Second pass: bbox is a rectangle, the province is not. Every hop so far
    // filtered by rectangle, so neighbouring provinces leak in at the corners —
    // drop features whose own admin attributes place them outside the area the
    // user actually named. Runs before compute_bbox so the map fits the real
    // result, not the overspill.
    let (all_features, dropped_area) = area_filter::filter_to_area(all_features, &resolved);
    if dropped_area > 0 {
        tracing::info!(
            dropped = dropped_area,
            kept = all_features.len(),
            area = %resolved.name_th,
            "dropped features outside the requested admin area"
        );
    }

    // Same story for time: STAC matched whole item containers that merely
    // overlap the interval, so trim to the days actually asked for.
    let (all_features, dropped_time) =
        time_filter::filter_to_range(all_features, start.date_naive(), end.date_naive());
    if dropped_time > 0 {
        tracing::info!(
            dropped = dropped_time,
            kept = all_features.len(),
            "dropped features outside the requested time range"
        );
    }
    let dropped = dropped_area + dropped_time;

    let bbox = compute_bbox(&all_features);
    // A wide date range can match more item containers than the server-side cap
    // returns, and the shortfall is invisible in the feature count alone — the
    // whole of July matched 65 items but yielded 50, so the map silently showed
    // part of the month. Say so rather than present a partial answer as total.
    let truncated = search_result.truncated || failed_items > 0;
    let truncation_note = match (lang, search_result.truncated, failed_items) {
        (Lang::Th, true, _) => format!(
            " (แสดงบางส่วน: พบ {} ชุดข้อมูล ดึงมา {} ชุด ลองแคบช่วงเวลาลง)",
            search_result.matched,
            search_result.items.len()
        ),
        (Lang::En, true, _) => format!(
            " (partial: {} datasets matched, {} retrieved — try a narrower period)",
            search_result.matched,
            search_result.items.len()
        ),
        (Lang::Th, false, n) if n > 0 => format!(" (ดึงข้อมูลไม่สำเร็จ {n} ชุด ผลอาจไม่ครบ)"),
        (Lang::En, false, n) if n > 0 => format!(" ({n} datasets failed to load — result may be incomplete)"),
        _ => String::new(),
    };

    // Flood leads with extent, not a record count: its features are polygon
    // fragments of one flooded surface, so "38,190 areas" describes the
    // tiling, not the flood. The figure comes from the same de-duplicated
    // computation the panel shows.
    // Computed before the summary because the flood sentence quotes its area
    // figure — one de-duplicated calculation, not two that could disagree.
    let computed = stats::compute(&all_features, &resolved, hazard, lang);

    let flood_area = (hazard == Hazard::Flood)
        .then(|| {
            computed
                .metrics
                .iter()
                .find(|m| m.label.contains("น้ำท่วม") || m.label.contains("Flooded"))
                .map(|m| m.value.clone())
        })
        .flatten();

    let summary = match (lang, all_features.is_empty()) {
        (Lang::Th, true) => format!(
            "ไม่พบข้อมูล{}ใน{} ช่วง {}",
            hazard_label(hazard, lang), resolved.name_th, label
        ),
        (Lang::Th, false) => match &flood_area {
            Some(a) => format!(
                "พบพื้นที่น้ำท่วม {} ใน{} ช่วง {}{}",
                a, resolved.name_th, label, truncation_note
            ),
            None => format!(
                "พบ {} พื้นที่{}ใน{} ช่วง {}{}",
                all_features.len(), hazard_label(hazard, lang), resolved.name_th, label, truncation_note
            ),
        },
        (Lang::En, true) => format!(
            "No {} data found in {} during {}",
            hazard_label(hazard, lang), resolved.name_en, label
        ),
        (Lang::En, false) => match &flood_area {
            Some(a) => format!(
                "Flooded area of {} in {} during {}{}",
                a, resolved.name_en, label, truncation_note
            ),
            None => format!(
                "Found {} {} areas in {} during {}{}",
                all_features.len(), hazard_label(hazard, lang), resolved.name_en, label, truncation_note
            ),
        },
    };

    let analysis = narrate_stats(&state, &computed, &resolved, hazard, &label, lang).await;

    Ok(Json(json!({
        "needs_clarification": false,
        "hazard": hazard,
        "location_text": extracted.location_text,
        "resolved_area": resolved_area,
        "time_window_days": extracted.time_window_days,
        "time_range": time_range_json,
        "dataset": dataset,
        "summary": summary,
        "geojson": { "type": "FeatureCollection", "features": all_features },
        "bbox": bbox,
        "filtered_out": dropped,
        "truncated": truncated,
        "stats": computed,
        "analysis": analysis,
        "lang": lang,
    })))
}
