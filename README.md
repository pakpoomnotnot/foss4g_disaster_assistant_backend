# askme-disaster — Backend

Rust (axum) service that answers natural-language questions about disasters in Thailand. It takes a question in Thai or English, works out what is being asked, fetches the matching data from GISTDA, and returns GeoJSON, statistics and a short written analysis for the map frontend.

- **Frontend:** [foss4g_disaster_assistant_frontend](https://github.com/pakpoomnotnot/foss4g_disaster_assistant_frontend) (Next.js + MapLibre)
- **API contract for frontend developers:** [API.md](API.md) has every request and response field, the error codes, and the GeoJSON shape for each hazard. This README covers how the backend works inside.
- **Background docs** are in the frontend repo:
  - [learning-notes.md](https://github.com/pakpoomnotnot/foss4g_disaster_assistant_frontend/blob/main/docs/learning-notes.md): notes on the original design deck
  - [stac-api-structure.md](https://github.com/pakpoomnotnot/foss4g_disaster_assistant_frontend/blob/main/docs/stac-api-structure.md): the GISTDA STAC API, tested against the live service
  - [gazetteer-structure.md](https://github.com/pakpoomnotnot/foss4g_disaster_assistant_frontend/blob/main/docs/gazetteer-structure.md): the gazetteer format

---

## Quick start

Requirements: a recent stable Rust toolchain, a GISTDA API key, and an OpenAI-compatible LLM endpoint (OpenRouter or a local Ollama).

1. Clone this repo next to the frontend repo. The backend reads the gazetteer file from the frontend:

   ```
   project/
   ├── foss4g_disaster_assistant_backend/    (this repo)
   └── foss4g_disaster_assistant_frontend/   (provides src/lib/gazetteer_merged.json)
   ```

2. Create a `.env` file in this folder (see [Environment variables](#environment-variables)). It is gitignored. Never commit it.

3. Run the server:

   ```bash
   cargo run
   ```

   The server listens on `http://localhost:8080`. Check it with:

   ```bash
   curl localhost:8080/api/health
   ```

   ```bash
   curl -X POST localhost:8080/api/query -H 'Content-Type: application/json' -d '{"text":"flooding in Ayutthaya this week"}'
   ```

`.env` is loaded from the current working directory, so run `cargo run` from this folder.

---

## 1. What the backend does

It takes a natural-language question from the frontend and returns GeoJSON plus a summary, which the frontend draws on a MapLibre map.

```
User types: "น้ำท่วมที่อยุธยาสัปดาห์นี้" ("flooding in Ayutthaya this week")
      |
POST /api/query  { "text": "..." }
      |
Rust backend
      |
{ "summary", "analysis", "stats", "geojson", "bbox", "hazard", "lang", ... }
```

**Out of scope:** rendering the map and choosing the basemap. Both are frontend jobs.

Supported hazards are `flood`, `fire` (VIIRS/MODIS hotspots) and `drought` (DRI+, SMAP soil moisture and NDWI vegetation moisture).

---

## 2. Pipeline

```
 1. Receive { text } on POST /api/query.
 2. Detect the reply language (th/en) from the user's text (src/lang.rs).
    No LLM call is needed for this.
 3. Call the LLM (OpenAI-compatible /chat/completions) with a system prompt and few-shot
    examples (src/llm/prompt.rs). It returns:
      { hazard, location_text, time_window_days, date_from, date_to,
        dataset, confidence, needs_clarification }
 4. If needs_clarification is true, or the hazard is not supported, reply
    with a clarification message and stop.
 5. Grounding: look up location_text in the gazetteer (section 5).
      - one match           -> area_idn + bbox + admin level
      - no match / several  -> needs_clarification with a candidate list
    The LLM never produces coordinates. Only the gazetteer does.
 6. Resolve the time range: date_from/date_to if the user named dates,
    otherwise the last time_window_days days.
 7. Search the GISTDA STAC Gateway (section 4) with bbox + datetime, paging through results.
 8. For each matching item, follow item.assets.data.href to the Vallaris
    features API ("second hop"), sending the bbox again and paging via offset.
 9. Post-filter the features:
      - area_filter: drop features that GISTDA stamps with a different province or
        amphoe (a bbox is a rectangle, a province is not)
      - time_filter: drop features whose observation date is outside the range
10. Compute statistics in code (src/analysis/stats.rs): totals, top areas
    and categories, with the rest grouped as "other".
11. Ask the LLM to write a short analysis of those statistics only
    (src/llm/analysis_prompt.rs). It never sees raw features, so it cannot
    invent counts.
12. Return summary + analysis + stats + GeoJSON FeatureCollection + bbox.
```

**Pitfall, found as a real bug during testing:** the `bbox` sent to STAC `/search` (step 7) only selects which **item containers** overlap the area. It does not filter the points or polygons inside them. Those come from Vallaris, which needs its own `bbox`. Without it, results are unrelated to the area asked about. In one test, a fire query near Ayutthaya returned hotspots in Indonesia. The fix is in `src/gistda/vallaris.rs::fetch_features`.

---

## 3. LLM (OpenAI-compatible)

Any endpoint that implements `/chat/completions` works: OpenRouter, a company gateway, or Ollama at `http://localhost:11434/v1`. Client code is in `src/llm/openai.rs`.

**Extraction request:** `POST {LLM_BASE_URL}/chat/completions` (the base URL must end with `/v1`)

```json
{
  "model": "<LLM_MODEL>",
  "messages": [
    { "role": "system", "content": "<system prompt + few-shot>" },
    { "role": "user",   "content": "<user text>" }
  ],
  "response_format": { "type": "json_object" },
  "stream": false,
  "temperature": 0
}
```

- `response_format: {"type":"json_object"}` is required. Without it, models tend to wrap the JSON in prose or code fences.
- `temperature: 0` makes the same sentence produce the same query object every time.
- The reply is a JSON **string** in `choices[0].message.content` and is parsed into `ExtractedQuery`. If the schema is invalid, the backend retries once.
- If `LLM_API_KEY` is empty, no `Authorization` header is sent. For OpenRouter, `OPENROUTER_API_KEY` from the shell is used as a fallback.
- `LLM_REASONING=true` sends OpenRouter's `reasoning: {enabled: true}`. It is off by default because it is slower and extraction does not need it.

**Analysis request:** a second call that turns the computed statistics into a short paragraph. There is one prompt per reply language. It does not use `response_format`.

**Choosing a model:**

| Option | Example | Notes |
|---|---|---|
| OpenRouter | `google/gemma-4-26b-a4b-it:free` | Good quality. `:free` models can be rate-limited upstream (HTTP 429), and the API then returns `LLM unavailable or timed out`. |
| Ollama (local) | `gemma3:4b` | Free and offline. Extraction works, but the written analysis is less reliable with small models. |

---

## 4. GISTDA STAC Gateway

### 4.1 Endpoints

**Base:** `https://api-gateway.gistda.or.th/api/2.0`

| Endpoint | Purpose |
|---|---|
| `GET /resources/stac/{hazard}` | Root catalog, with `links[]` to `search`, `collections` and `conformance` |
| `GET /resources/stac/{hazard}/search?bbox=&datetime=&limit=&page=&api_key=` | The main endpoint. `bbox` and `datetime` are confirmed to filter: a bbox outside Thailand returns `matched: 0`. |
| `GET {item.assets.data.href}` | Second hop. Returns JSON whose `links[].href` points to the **Vallaris Dragonfly Features API** (`vallaris.dragonfly.gistda.or.th/core/api/features/1.1/collections/{id}/items`). Auth is already embedded in that link. |

The data structure differs a lot between flood, fire and drought. See [stac-api-structure.md](https://github.com/pakpoomnotnot/foss4g_disaster_assistant_frontend/blob/main/docs/stac-api-structure.md), which includes real sample JSON.

### 4.2 Auth: `api_key` (preferred) or `token` (fallback)

`GISTDA_API_KEY` is the preferred credential:

- The gateway accepts `?api_key=...` on `/search` and on the second hop. It also carries the same auth into the `asset.data` href, so the second hop needs nothing extra.
- An `api_key` never expires, so no refresh flow is needed.
- With `GISTDA_API_KEY` set, the backend uses it automatically (`config.rs::gistda_auth`), and `/api/health` reports `"gistda_auth": "api_key"`.
- Both methods were compared on the same query and bbox, and returned identical results (29 points).

```
GET {GATEWAY_HOST}/resources/stac/{hazard}/search?bbox=...&datetime=...&api_key={KEY}
```

#### 4.2.1 Token fallback

If `GISTDA_API_KEY` is empty, the backend uses `GISTDA_ACCESS_TOKEN`. This is a JWT that expires about 1 hour after issue (`exp - iat = 3600`), so you have to paste it in again by hand. GISTDA's production frontend gets tokens this way:

**New token (password grant):**
```
POST {GATEWAY_HOST}/api/2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=password&accountType=email&username={USERNAME}&password={PASSWORD}
```
Response: `{ "accessToken": "...", "refreshToken": "...", "user": {...} }`

**Refresh:**
```
POST {GATEWAY_HOST}/api/2.0/token
Content-Type: application/x-www-form-urlencoded
Authorization: Bearer {refresh_token}

grant_type=refresh_token&refresh_token={refresh_token}
```

### 4.3 Auth summary

| | `api_key` (in use) | `token` (fallback) |
|---|---|---|
| Lifetime | Does not expire | About 1 hour |
| Needs refresh | No | Yes (manually) |
| Works with `/search` | Yes (tested) | Yes |
| Works with second hop | Yes (gateway adds it) | Yes |
| Suitable for | Development and production | Short-term development only |

An automatic token refresh flow was planned as Phase 2. It was dropped because `api_key` solves the same problem without extra code. The JWT `exp` check in `gistda/auth.rs` is still used, but only for the token fallback.

---

## 5. Gazetteer and grounding

The gazetteer lives in the frontend repo at `src/lib/gazetteer_merged.json`. It has 8,429 Thai admin areas (provinces, amphoe and tambon). It merges the area hierarchy from `boundary.json` with the bounding boxes from `bbox_pat.json`. Every record has a bbox.

```json
{
  "area_idn": 100508,
  "level": "tambon",
  "name_th": "ท่าแร้ง",
  "name_en": "THA RAENG",
  "parent_area_idn": 1005,
  "bbox": [100.618962, 13.842544, 100.685775, 13.886752]
}
```

`level` is `"province" | "amphoe" | "tambon"`. `parent_area_idn` is `null` for provinces. The file is loaded once at startup into memory (`src/gazetteer/loader.rs`), and no database is needed.

**Key finding:** `area_idn` matches the `pv_idn`, `ap_idn` and `tb_idn` fields in GISTDA drought features exactly. For example, `1005` is Bang Khen on both sides. Drought data can therefore be joined by code rather than by bbox intersection.

### 5.1 Lookup algorithm (`location_text` to gazetteer entry)

Implemented in `src/gazetteer/lookup.rs`.

```
1. Normalise the input: trim, and lowercase for name_en comparisons (Thai has no case).
   Strip admin prefixes such as "จังหวัด" (province) and "อำเภอ" (district).
2. Check the alias table first (5.2). If it matches, use it.
3. Exact match on name_en / name_th at every level.
     - exactly one       -> use it
     - several (875 of 7,424 tambon names are duplicates) -> try collapsing
       parent/child matches, otherwise go to 5
4. Substring match on name_en and name_th
   ("Ayutthaya" in "Phra Nakhon Si Ayutthaya", "อยุธยา" in "พระนครศรีอยุธยา").
5. Still nothing, or still ambiguous -> needs_clarification with a candidate list.
   Never guess.
```

**Two rules that came from real bugs. Do not revert them:**

1. **Substring matching must check `name_th` too.** It used to check only `name_en`, so every Thai short name failed, for example "อยุธยา" did not match "พระนครศรีอยุธยา".
2. **Aliases must run before exact match.** There is a real tambon called "โคราช" (area_idn 301803, about 5 km across). If exact match ran first, "โคราชแล้งไหม" ("is Korat in drought?") would search that tiny tambon instead of Nakhon Ratchasima province, and always return nothing.

### 5.2 Alias table

Defined in `src/gazetteer/alias.rs`. The gazetteer has no informal names. For example, "Korat" does not appear at all, because the official name is Nakhon Ratchasima.

```json
{ "korat": 30, "โคราช": 30, "hat yai": 9011, "หาดใหญ่": 9011 }
```

Keys are lowercase aliases. Values are the target `area_idn`.

The table runs before exact matching, so it overrides the gazetteer. Only add a name when automatic matching gives the **wrong** result:

- `โคราช` / `korat` is needed because it collides with the real tambon "โคราช".
- `หาดใหญ่` / `hat yai` is needed, and points to **Hat Yai district (9011)** rather than Songkhla province, because that is what people mean. It also avoids tambon "หาดยาย" (860413, Chumphon), whose `name_en` is spelled "HAT YAI" too.
- Most Thai short names do **not** need an alias, because substring matching already handles them (อยุธยา, กรุงเทพ, and so on).

---

## 6. HTTP API (summary)

Full details are in [API.md](API.md).

- `POST /api/query` takes `{ "text": string }`. It returns one of three shapes: a clarification request, a result, or an empty result. An empty result is not an error.
- `GET /api/health` reports whether the LLM is reachable (it pings `/models`), whether the gazetteer is loaded, and whether the GISTDA credential is valid.
- CORS is fully permissive for development (`src/main.rs`). Restrict it before production.

---

## Environment variables

Create `.env` in this folder. It is gitignored. Never commit it.

| Variable | Example | Notes |
|---|---|---|
| `LLM_BASE_URL` | `https://openrouter.ai/api/v1` | Must end with `/v1`. Use `http://localhost:11434/v1` for Ollama. |
| `LLM_API_KEY` | *(your key)* | Can be empty for servers without auth. Falls back to `OPENROUTER_API_KEY`. |
| `LLM_MODEL` | `google/gemma-4-26b-a4b-it:free` | Use `gemma3:4b` (or similar) for Ollama |
| `LLM_REASONING` | `false` | OpenRouter reasoning mode |
| `GISTDA_GATEWAY_HOST` | `https://api-gateway.gistda.or.th/api/2.0` | |
| `GISTDA_API_KEY` | *(key from GISTDA)* | Preferred. It does not expire (section 4.2). |
| `GISTDA_ACCESS_TOKEN` | *(empty)* | Fallback when there is no API key. It is a JWT that expires after about 1 hour. |
| `GAZETTEER_PATH` | `../foss4g_disaster_assistant_frontend/src/lib/gazetteer_merged.json` | Path relative to this folder |
| `PORT` | `8080` | |
| `RUST_LOG` | `info` | |

---

## Project structure

```
foss4g_disaster_assistant_backend/
├── Cargo.toml
├── API.md                 API contract for the frontend
├── README.md
├── .env                   local only, gitignored
└── src/
    ├── main.rs            axum router, CORS, loads the gazetteer at startup
    ├── config.rs          reads environment variables
    ├── error.rs           error type -> HTTP status mapping
    ├── models.rs          shared types (Hazard, ExtractedQuery, GazetteerEntry, ...)
    ├── lang.rs            reply-language detection
    ├── llm/
    │   ├── openai.rs      /chat/completions client, extraction + analysis calls
    │   ├── prompt.rs      extraction system prompt + few-shot examples
    │   └── analysis_prompt.rs   analysis prompts (Thai and English)
    ├── gazetteer/
    │   ├── loader.rs      loads gazetteer_merged.json
    │   ├── lookup.rs      lookup algorithm (5.1)
    │   └── alias.rs       alias table (5.2)
    ├── gistda/
    │   ├── auth.rs        api_key / token handling (4.2)
    │   ├── stac.rs        STAC /search with paging
    │   ├── vallaris.rs    second-hop feature fetch with bbox and paging
    │   ├── area_filter.rs drops features outside the requested admin area
    │   └── time_filter.rs drops features outside the requested date range
    ├── analysis/
    │   └── stats.rs       statistics computed from the features
    └── handlers/
        ├── query.rs       POST /api/query, runs the whole pipeline
        └── health.rs      GET /api/health
```

Main crates: `axum`, `tokio`, `reqwest`, `serde` / `serde_json`, `dotenvy`, `tracing`, `thiserror` / `anyhow`, `chrono`, `tower-http` (CORS).

---

## 7. Error handling and edge cases

| Case | Behaviour |
|---|---|
| LLM returns JSON that does not match the schema | Retry once. If it fails again, return an error without leaking the prompt or response. |
| Location not found or ambiguous | `needs_clarification: true` with candidates. Never guess a bbox. |
| STAC `/search` returns `context.matched: 0` | Not an error. Return an empty result with a "no data found" summary. |
| Vallaris returns `numberMatched: 0` | Same as above. This is common, for example with flood when there is no flooding in the period. |
| Missing or expired GISTDA credential | Checked before every call (`config::gistda_credential_valid`). Returns 503 with a clear message instead of passing through a GISTDA 401. |
| LLM unreachable, timed out or rate-limited | 503, so the frontend knows the problem is the LLM, not the question. |
| Very large result sets (fire: 10,000+ points per day; drought: 85,000+ polygons per collection) | Always query with bbox and datetime, and page through results. Never download everything and filter locally. |

---

## 8. Caching

- **Gazetteer:** loaded once at startup and kept in memory. The data is static.
- **STAC/Vallaris responses:** GISTDA's production dashboard uses Redis to cache resolved collection IDs. This backend does not need that for v1, because each query is narrow (one area and one time range). Add caching later if latency becomes a real problem.

---

## 9. Open questions

GISTDA's production frontend uses two more API surfaces that we have no credentials for. Either might be simpler than the STAC + second-hop route:

1. **`core/api/stac/1.0/{hazard}/search?api_key=...`** (Vallaris-native STAC). Asset links appear to be partly resolved already, so no second hop is needed. It also has rolling-window fire collections (`viirs_1day/3days/7days/30days`, `modis_...`).
2. **`{GATEWAY_HOST}/api/2.0/resources/{flood|viirs}/{1day|3days|7days|30days}`**, `/resources/summary/disaster` and `/resources/analytics/eco-plant/{rice|maize|cassava|sugarcane}`. These are REST endpoints, not STAC, and may return ready-to-use, pre-aggregated data.

The current design uses `api-gateway.gistda.or.th/api/2.0/resources/stac` because it is the only surface tested with real credentials. If credentials for (1) or (2) become available, compare them first. They could remove the second hop and provide summaries without extra work.

---

## 10. Testing

```bash
cargo test
```

- 37 unit tests cover admin-prefix stripping in the gazetteer lookup, the area and time filters, statistics, language detection, the prompts and LLM response parsing.
- Planned: lookup tests for the 8 evaluation cases in [learning-notes.md](https://github.com/pakpoomnotnot/foss4g_disaster_assistant_frontend/blob/main/docs/learning-notes.md), especially "Korat" (alias table) and "Ayutthaya" (substring match).
- Planned: integration tests for `/api/query` with mocked GISTDA and LLM responses. CI should not call live APIs, because the data changes daily.
- For smoke tests against the live APIs, set `GISTDA_API_KEY` and run the curl commands in [Quick start](#quick-start).
