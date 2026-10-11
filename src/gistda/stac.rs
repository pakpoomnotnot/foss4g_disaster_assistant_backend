use futures::stream::{self, StreamExt};
use serde::Deserialize;

use crate::error::AppError;
use crate::models::Hazard;

/// Items per `/search` page. The endpoint caps a single response well below the
/// number a wide date range matches (a year over one province matched 756), so
/// completeness comes from paging, not from a bigger number here.
const PAGE_SIZE: u32 = 100;

/// Hard ceiling on items we will page through, as a runaway guard only — a
/// nationwide multi-year question would otherwise fan out into tens of
/// thousands of second-hop requests. Hitting it is reported, never silent.
const MAX_ITEMS: usize = 3000;

/// How many `/search` pages to request at once. Page 1 tells us `matched`, so
/// the rest are known up-front and can go out together instead of in a chain.
const PAGE_CONCURRENCY: usize = 6;

#[derive(Debug, Deserialize)]
pub struct StacSearchResponse {
    pub context: StacContext,
    #[serde(default)]
    pub features: Vec<StacItem>,
}

#[derive(Debug, Deserialize)]
pub struct StacContext {
    pub matched: u64,
}

#[derive(Debug, Deserialize)]
pub struct StacItem {
    pub id: String,
    pub assets: StacAssets,
}

#[derive(Debug, Deserialize)]
pub struct StacAssets {
    /// Optional on purpose. A `/search` hit is not always a derived product:
    /// some bboxes also match raw Sentinel-1 SAR scenes that carry only a
    /// `file` asset. Declaring `data` as required made serde fail the whole
    /// response, so one such scene in range turned an otherwise fine query
    /// into a 502 (hit live with the Khon Kaen bbox). Items without it simply
    /// have nothing for us to fetch and are skipped by the caller.
    #[serde(default)]
    pub data: Option<StacAsset>,
}

#[derive(Debug, Deserialize)]
pub struct StacAsset {
    pub href: String,
}

/// Every matching item, paged. `matched` is what the server says exists;
/// `items.len()` is what we actually hold, and the two differ only when
/// `MAX_ITEMS` cut it short — which `truncated` reports.
pub struct StacSearchOutcome {
    pub items: Vec<StacItem>,
    pub matched: u64,
    pub truncated: bool,
}

/// Pages `/search` until every matching item is collected.
///
/// A single page is not the answer: `limit` is capped server-side, so any
/// question wider than a few days used to come back quietly incomplete — and
/// incomplete in the worst possible way, since the items arrive oldest-first.
/// A "whole of 2025" query returned 50 of 756 items, all from January to May,
/// which looks like a plausible fire season rather than a truncated one.
pub async fn search_all(
    client: &reqwest::Client,
    gateway_host: &str,
    hazard: Hazard,
    bbox: [f64; 4],
    datetime_interval: &str,
    auth: (&str, &str),
) -> Result<StacSearchOutcome, AppError> {
    let first = search_page(client, gateway_host, hazard, bbox, datetime_interval, auth, 1).await?;
    let matched = first.context.matched;
    let mut items = first.features;

    let wanted = (matched as usize).min(MAX_ITEMS);
    if items.len() >= wanted || items.is_empty() {
        let truncated = (items.len() as u64) < matched;
        return Ok(StacSearchOutcome { items, matched, truncated });
    }

    // Ceil-divide the remainder into pages; page 1 is already in hand.
    let total_pages = wanted.div_ceil(PAGE_SIZE as usize);
    let pages: Vec<u32> = (2..=total_pages as u32).collect();

    let mut rest: Vec<Vec<StacItem>> = stream::iter(pages)
        .map(|page| async move {
            match search_page(client, gateway_host, hazard, bbox, datetime_interval, auth, page).await {
                Ok(r) => r.features,
                Err(e) => {
                    // One bad page must not sink an otherwise good answer; the
                    // shortfall still surfaces through `truncated` below.
                    tracing::warn!(error = %e, page, "STAC page failed, skipping");
                    Vec::new()
                }
            }
        })
        .buffered(PAGE_CONCURRENCY)
        .collect()
        .await;

    for page_items in rest.drain(..) {
        items.extend(page_items);
    }
    items.truncate(MAX_ITEMS);

    let truncated = (items.len() as u64) < matched;
    if truncated {
        tracing::warn!(matched, collected = items.len(), "STAC item set truncated");
    }
    Ok(StacSearchOutcome { items, matched, truncated })
}

/// GET /{hazard}/search?bbox=&datetime=&limit=&page= — the cross-collection
/// search endpoint confirmed against all 3 hazards (docs/stac-api-structure.md).
///
/// `auth` is the credential pair, e.g. `("api_key", "…")`. The gateway echoes
/// whichever scheme was used into the `asset.data` hrefs it returns, so the
/// second hop inherits it for free.
async fn search_page(
    client: &reqwest::Client,
    gateway_host: &str,
    hazard: Hazard,
    bbox: [f64; 4],
    datetime_interval: &str,
    auth: (&str, &str),
    page: u32,
) -> Result<StacSearchResponse, AppError> {
    let url = format!("{gateway_host}/resources/stac/{}/search", hazard.as_str());
    let bbox_str = format!("{},{},{},{}", bbox[0], bbox[1], bbox[2], bbox[3]);
    let limit_str = PAGE_SIZE.to_string();
    let page_str = page.to_string();

    let resp = client
        .get(&url)
        .query(&[
            ("bbox", bbox_str.as_str()),
            ("datetime", datetime_interval),
            ("limit", limit_str.as_str()),
            ("page", page_str.as_str()),
            auth,
        ])
        .send()
        .await
        .map_err(|e| AppError::Gistda(format!("STAC search request failed: {e}")))?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(AppError::TokenExpired);
    }
    if !resp.status().is_success() {
        return Err(AppError::Gistda(format!(
            "STAC search returned {}",
            resp.status()
        )));
    }

    resp.json::<StacSearchResponse>()
        .await
        .map_err(|e| AppError::Gistda(format!("failed to parse STAC search response: {e}")))
}
