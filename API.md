# askme-disaster Backend — API Reference

This is the contract between the backend and the frontend. It describes what the code in `src/handlers/` does today. If the backend behaves differently from this document, treat that as a bug.

For how the backend works inside (LLM prompts, gazetteer, GISTDA auth), see [README.md](README.md).

---

## 1. Base URL

Development: `http://localhost:8080`. The port is set by the backend's `PORT` variable.

**CORS:** fully permissive (`Access-Control-Allow-Origin: *`) for development, so the frontend needs no extra setup. Restrict it before production.

**Content-Type:** every request and response is `application/json`.

---

## 2. `POST /api/query`

Takes a natural-language question in Thai or English and returns GeoJSON, statistics and a summary.

### Request

```json
{ "text": "flooding in Ayutthaya this week", "area_idn": null }
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `text` | string | yes | The user's question |
| `area_idn` | number \| null | no | Set this when the user picks a candidate from a clarification response (2.1). The backend then uses that area directly instead of resolving the place name again. |

### Responses

There are three successful shapes, all returned with HTTP 200. Errors are described in 2.4.

**Always check `needs_clarification` first.** If it is `true`, the response has no `geojson` or `bbox`. Show `message` and `candidates` to the user instead of rendering the map.

#### 2.1 Clarification needed (`needs_clarification: true`)

This happens when:

- (a) the LLM could not find a hazard or a location in the sentence
- (b) the place name is not in the gazetteer
- (c) the place name matches more than one area

```json
{
  "needs_clarification": true,
  "message": "Found several places named \"Bang Phli\". Please be more specific.",
  "missing": null,
  "candidates": [
    {
      "area_idn": 1103,
      "level": "amphoe",
      "name_th": "บางพลี",
      "name_en": "Bang Phli",
      "province": "Samut Prakarn"
    },
    {
      "area_idn": 140402,
      "level": "tambon",
      "name_th": "บางพลี",
      "name_en": "BANG PHLI",
      "province": "Phra Nakhon Si Ayutthaya"
    }
  ]
}
```

This is real data. "Bang Phli" is both a district in Samut Prakan and a tambon in Ayutthaya. `message` is written in the language of the question.

| Field | Type | Notes |
|---|---|---|
| `message` | string | Text to show the user |
| `missing` | `"location"` \| `"hazard"` \| `null` | Which part of the question was missing. `"location"` means everything else was understood, so the user's next message can be treated as the place and combined with the original question. |
| `candidates` | Candidate[] | Only filled in case (c), with at most 10 entries. Empty (`[]`) in cases (a) and (b). |

**Candidate object:**

| Field | Type | Notes |
|---|---|---|
| `area_idn` | number | Send it back as `area_idn` in the next request when the user picks this candidate |
| `level` | `"province"` \| `"amphoe"` \| `"tambon"` | |
| `name_th` | string | |
| `name_en` | string | |
| `province` | string \| null | English name of the province the candidate belongs to. `null` when the candidate is itself a province. |

**Suggested UX:** show the candidates as clickable chips. When the user clicks one, resend the original `text` with that `area_idn`.

#### 2.2 Data found (`needs_clarification: false`, with features)

```json
{
  "needs_clarification": false,
  "hazard": "flood",
  "location_text": "Ayutthaya",
  "resolved_area": {
    "area_idn": 14,
    "level": "province",
    "name_th": "พระนครศรีอยุธยา",
    "name_en": "Phra Nakhon Si Ayutthaya",
    "parent_area_idn": null
  },
  "time_window_days": 7,
  "time_range": { "from": "2026-10-04", "to": "2026-10-11", "label": "the last 7 days" },
  "dataset": null,
  "summary": "Found 3 flooded areas in Phra Nakhon Si Ayutthaya during the last 7 days",
  "geojson": { "type": "FeatureCollection", "features": [] },
  "bbox": [100.212706, 14.111092, 100.82323, 14.673195],
  "filtered_out": 0,
  "truncated": false,
  "stats": { "...": "see below" },
  "analysis": "Flooding over the last 7 days is concentrated in ...",
  "lang": "en"
}
```

(`features` is shortened here. The structure of each feature is in section 4.)

#### 2.3 No data for this query (`needs_clarification: false`, no features)

This is **not an error**. The shape is the same as 2.2, except:

```json
{
  "summary": "No flood data found in Phra Nakhon Si Ayutthaya during the last 7 days",
  "geojson": { "type": "FeatureCollection", "features": [] },
  "bbox": null,
  "analysis": null
}
```

`stats.total` is `0`. When the STAC search itself matched nothing, `filtered_out` and `truncated` are omitted, so treat them as optional. Show the `summary` and do not zoom the map, because `bbox` is `null`.

#### Result fields (2.2 and 2.3)

| Field | Type | Notes |
|---|---|---|
| `needs_clarification` | `false` | |
| `hazard` | `"flood"` \| `"fire"` \| `"drought"` | |
| `location_text` | string | Place text as extracted by the LLM, not normalised |
| `resolved_area` | object | `{ area_idn, level, name_th, name_en, parent_area_idn }` |
| `time_window_days` | number | 1 to 365. If the user named explicit dates, use `time_range` instead. |
| `time_range` | `{ from, to, label }` | The period that was actually searched (`YYYY-MM-DD`). `label` is a readable description. It can differ from the question when the dates the LLM extracted were invalid and the backend fell back to a relative window. |
| `dataset` | `"dri_plus"` \| `"smap"` \| `"ndwi"` \| `null` | Drought product that was searched. It is always set for drought (a plain "ภัยแล้ง" ("drought") means DRI+) and `null` for flood and fire. |
| `summary` | string | One-sentence summary, ready to display |
| `geojson` | FeatureCollection | See section 4. The properties **differ a lot between hazards**. |
| `bbox` | `[minLon, minLat, maxLon, maxLat]` \| `null` | Computed from the returned feature geometries, not from the queried area. Pass it straight to `map.fitBounds()`. `null` when there are no features. |
| `filtered_out` | number | Features dropped because they were outside the requested area or date range |
| `truncated` | boolean | `true` when some upstream items could not be fetched, or the 3,000-item STAC cap was reached, so the result is known to be incomplete |
| `stats` | Stats | Statistics computed by the backend (see below) |
| `analysis` | string \| null | Short written analysis of `stats` by the LLM. `null` when the LLM was unreachable. The statistics are still valid, so a missing analysis never blocks the result. |
| `lang` | `"th"` \| `"en"` | Language of every string the backend wrote. Match the UI labels to it. |

**Stats object:**

| Field | Type | Notes |
|---|---|---|
| `total` | number | Number of features |
| `sub_level` | AdminLevel \| null | The level below the queried area (province to amphoe to tambon). `null` when a tambon was queried. |
| `sub_level_label` | string \| null | Readable name of that level |
| `breakdown` | Bucket[] | Counts per sub-area. The top 8 are listed and the rest are grouped into one `other` bucket. |
| `sub_areas_affected` | number | Number of sub-areas with at least one feature |
| `categories` | `{ title, buckets: Bucket[] }[]` | Hazard-specific groupings, for example land use for fire, severity class for drought and flooded farmland for flood |
| `timeline` | Bucket[] | Counts per date, in chronological order. Only filled for hazards whose features carry a date. |
| `metrics` | `{ label, value }[]` | Extra figures as display-ready strings |

**Bucket:** `{ name: string, count: number, share: number, other: boolean }`. `share` is a percentage of the total with one decimal. `other: true` marks the grouped remainder row, which is not a real place. Leave it out when scaling bar charts.

#### 2.4 Error responses (non-200)

```json
{ "error": "GISTDA credential missing or expired — set GISTDA_API_KEY in .env" }
```

| HTTP status | When | `error` message |
|---|---|---|
| 503 | LLM unreachable, timed out, rate-limited or returned non-2xx | `"LLM unavailable or timed out"` |
| 500 | LLM returned JSON that does not match the schema, even after one retry | `"failed to parse LLM response"` |
| 503 | GISTDA credential not set, or (token fallback only) expired | `"GISTDA credential missing or expired — set GISTDA_API_KEY in .env"` |
| 502 | GISTDA STAC search or Vallaris fetch failed or timed out | `"upstream GISTDA/Vallaris error"` |
| 500 | Any other unexpected internal error | `"internal server error"` |

**Suggested frontend handling:** show one generic "Something went wrong, please try again" message for all of these. The `error` text is written for developers, not end users. The credential 503 in particular is a backend configuration problem that users cannot fix.

---

## 3. `GET /api/health`

Health check with no request body.

```json
{
  "status": "ok",
  "gazetteer_loaded": true,
  "gazetteer_entries": 8429,
  "gistda_token_valid": true,
  "gistda_auth": "api_key",
  "llm_reachable": true,
  "llm_model": "google/gemma-4-26b-a4b-it:free"
}
```

- HTTP 200 with `status: "ok"` when every check passes.
- HTTP 503 with `status: "degraded"` when any check fails.
- `llm_reachable` only checks that the LLM's `/models` endpoint answers. A model that is temporarily rate-limited can still report `true`.
- There is no need to poll it often. Call it at startup, or use it for a status banner.

---

## 4. `geojson.features` structure: **differs by `hazard`, read before rendering**

The backend passes feature properties through from GISTDA/Vallaris without normalising them. It only computes the response-level `bbox` and `stats`. The shape of `properties` therefore depends on the `hazard`.

### `hazard: "flood"`: flooded-area polygons

```json
{
  "type": "Feature",
  "geometry": { "type": "Polygon", "coordinates": [] },
  "properties": {}
}
```

Properties vary between flood products and were often almost empty in the samples we tested. Do not assume any fixed fields.

### `hazard: "fire"`: satellite hotspot points (VIIRS/MODIS)

```json
{
  "type": "Feature",
  "geometry": { "type": "Point", "coordinates": [100.5, 14.3] },
  "properties": {
    "acq_date": "2025-01-31",
    "acq_time": "1734",
    "instrument": "VIIRS",
    "satellite": "N",
    "confidence": "nominal",
    "frp": 2.95,
    "changwat": "...",
    "amphoe": "...",
    "tambol": "...",
    "ct_en": "Thailand",
    "ct_tn": "ประเทศไทย"
  }
}
```

Notes for the frontend:

- `confidence` (`"low"` \| `"nominal"` \| `"high"`) and `frp` (Fire Radiative Power) are useful for styling by severity.
- `changwat`, `amphoe` and `tambol` are Thai admin names, reverse-geocoded by the provider. They are `null` for points outside Thailand, so never assume they are set.

### `hazard: "drought"`: tambon-level polygons

```json
{
  "type": "Feature",
  "geometry": { "type": "MultiPolygon", "coordinates": [] },
  "properties": {
    "pv_en": "Bangkok", "pv_tn": "กรุงเทพมหานคร", "pv_idn": "10",
    "ap_en": "BANG KHEN", "ap_tn": "บางเขน", "ap_idn": "1005",
    "tb_en": "THA RAENG", "tb_tn": "ท่าแร้ง", "tb_idn": "100508",
    "re_royin": "Central",
    "h3_address": "8764a48ebffffff",
    "dri_mean": null
  }
}
```

Notes for the frontend:

- Every polygon carries province, district and tambon names (`pv_*`, `ap_*`, `tb_*`), which are ready for tooltips and popups.
- The value fields depend on `dataset` (DRI+, SMAP or NDWI) and can be `null`. Handle `null` when building a choropleth.

**In short:** always switch on the response-level `hazard` (and `dataset` for drought) before reading feature properties. The schemas really are different.

---

## 5. Enum reference

```ts
type Hazard = "flood" | "fire" | "drought";
type DroughtProduct = "dri_plus" | "smap" | "ndwi";
type AdminLevel = "province" | "amphoe" | "tambon";
type Lang = "th" | "en";
```

---

## 6. curl examples

```bash
curl -s -X POST http://localhost:8080/api/query \
  -H "Content-Type: application/json" \
  -d '{"text": "is Korat in drought?"}'
```

```bash
curl -s http://localhost:8080/api/health
```

---

## 7. Not in v1

- No separate place search or autocomplete endpoint.
- No pagination on `/api/query`. The backend pages through GISTDA itself, with caps of 3,000 STAC items (which sets `truncated`) and 20,000 features per item. The frontend never sends `limit` or `offset`.
- No authentication on this API itself. This is separate from the GISTDA key the backend holds. Add authentication before any public deployment.
