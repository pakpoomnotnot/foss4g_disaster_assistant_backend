use serde_json::Value;

use crate::error::AppError;

/// Features per request. The API's own default is **10** — small enough that it
/// was quietly halving ordinary results (an item reporting `numberMatched: 21`
/// handed back 10) without anything in the response looking wrong.
const PAGE_SIZE: usize = 1000;

/// Per-item ceiling, a runaway guard for a single enormous collection.
const MAX_PER_ITEM: usize = 20_000;

/// Fetches every feature behind `item.assets.data.href` (README §2 step 8 / §4.1).
///
/// IMPORTANT (found by live testing, not in any doc): the STAC `bbox` filter
/// only narrows which *item containers* match (e.g. "did anything happen
/// today, roughly around Thailand") — it does NOT filter the point/polygon
/// data inside. That data lives on a second system (Vallaris Dragonfly
/// Features API) which happens to accept its own `bbox` query param. Without
/// re-applying it here, this call returns the first page of a globally
/// unfiltered dataset (confirmed live: a "flood near Ayutthaya" query was
/// returning a wildfire point in Indonesia before this fix) instead of only
/// the queried area.
///
/// Pages via `offset` rather than trusting one big `limit`: the server honoured
/// 10000 in testing, but a silent cap below the page size would truncate
/// invisibly, whereas the offset loop stops only when the server stops handing
/// out new features.
pub async fn fetch_features(
    client: &reqwest::Client,
    href: &str,
    bbox: [f64; 4],
) -> Result<Vec<Value>, AppError> {
    let bbox_str = format!("{},{},{},{}", bbox[0], bbox[1], bbox[2], bbox[3]);
    let mut out: Vec<Value> = Vec::new();
    let mut offset = 0usize;

    loop {
        let (features, matched) = fetch_page(client, href, &bbox_str, offset).await?;
        let got = features.len();
        out.extend(features);

        // Stop on an empty page (nothing left), once we hold everything the
        // server says exists, or at the guard rail.
        let have_all = matched.is_some_and(|m| out.len() >= m);
        if got == 0 || have_all || out.len() >= MAX_PER_ITEM {
            if out.len() >= MAX_PER_ITEM && !have_all {
                tracing::warn!(href, collected = out.len(), ?matched, "per-item cap reached");
            }
            break;
        }
        offset += got;
    }

    Ok(out)
}

async fn fetch_page(
    client: &reqwest::Client,
    href: &str,
    bbox_str: &str,
    offset: usize,
) -> Result<(Vec<Value>, Option<usize>), AppError> {
    let limit_str = PAGE_SIZE.to_string();
    let offset_str = offset.to_string();
    let resp = client
        .get(href)
        .query(&[
            ("bbox", bbox_str),
            ("limit", limit_str.as_str()),
            ("offset", offset_str.as_str()),
        ])
        .send()
        .await
        .map_err(|e| AppError::Gistda(format!("asset fetch failed: {e}")))?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(AppError::TokenExpired);
    }
    if !resp.status().is_success() {
        return Err(AppError::Gistda(format!(
            "asset fetch returned {}",
            resp.status()
        )));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| AppError::Gistda(format!("failed to parse asset response: {e}")))?;

    // numberMatched: 0 is a valid empty result, not an error (README §10).
    let features = body
        .get("features")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();
    let matched = body
        .get("numberMatched")
        .and_then(Value::as_u64)
        .map(|m| m as usize);
    Ok((features, matched))
}
