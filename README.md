# askme-disaster — Rust Backend

> สถานะ: **implement แล้ว (v1)** — `cargo build` ผ่านสะอาด, ทดสอบ pipeline เต็มสาย (LLM extraction → gazetteer grounding → GISTDA STAC → Vallaris) แล้วจริงกับ LLM แบบ OpenAI-compatible (tekton-code) เอกสารนี้เดิมเขียนเป็น spec ก่อน implement — ตอนนี้ปรับให้ตรงกับโค้ดจริงแล้ว (ยกเว้นส่วนที่ยังทำเป็น Phase 2 ตามที่ระบุไว้)
> **สำหรับทีม frontend:** ดู [API.md](API.md) สำหรับ API contract ล้วนๆ (request/response ทุก field, error codes, โครงสร้าง geojson ต่อ hazard) — ไฟล์นี้ (README) เก็บ implementation notes/internal design ไว้เฉยๆ
> อ้างอิงพื้นฐานจาก: [../docs/learning-notes.md](../docs/learning-notes.md) (สรุป deck), [../docs/stac-api-structure.md](../docs/stac-api-structure.md) (STAC API จริงที่ทดสอบแล้ว), [../docs/gazetteer-structure.md](../docs/gazetteer-structure.md) (gazetteer)
> ค้นพบเพิ่มเมื่อ 2026-08-03 จากการอ่าน source ของ `gistda-disaster-frontend` (production frontend ของ GISTDA ที่มีอยู่แล้ว) — หลายจุดในนี้แก้ไข/เติมเต็มสิ่งที่ deck ไม่ได้พูดถึง

---

## 1. เป้าหมายของ backend นี้

รับคำถามภาษาธรรมชาติ (ไทย/อังกฤษ) จาก frontend (Next.js) → คืน GeoJSON + สรุปข้อความ ให้ frontend เอาไปแสดงบนแผนที่ MapLibre

```
ผู้ใช้พิมพ์: "น้ำท่วมที่อยุธยาสัปดาห์นี้"
      ↓
POST /api/query  { "text": "..." }
      ↓
Rust backend (เอกสารนี้)
      ↓
{ "summary": "...", "geojson": {...}, "bbox": [...], "hazard": "flood", ... }
```

**ไม่ใช่หน้าที่ของ backend นี้:** render แผนที่ (MapLibre ฝั่ง frontend), เลือก basemap style (ฝั่ง frontend เช่นกัน — ดู [§7](#7-basemap--ไม่ใช่หน้าที่ backend-แต่บันทึกไว้ที่นี่))

---

## 2. Pipeline แบบละเอียด (คำสั่งต่อคำสั่ง)

```
1. รับ { text } จาก POST /api/query
2. เรียก LLM (OpenAI-compatible `/chat/completions`) พร้อม system prompt + few-shot (ตาม deck §4)
   → ได้ { hazard, location_text, time_window_days, confidence, needs_clarification }
3. ถ้า needs_clarification == true → ตอบกลับทันที ไม่ต้อง query อะไรต่อ
   → { needs_clarification: true, message: "กรุณาระบุ..." }
4. Grounding: location_text → gazetteer lookup (§5)
   - exact match → ได้ area_idn + bbox + level ทันที
   - ไม่เจอ → ลอง alias table (Korat → Nakhon Ratchasima) แล้ว lookup ใหม่
   - ยังไม่เจอ / เจอมากกว่า 1 (ambiguous) → คืน needs_clarification: true พร้อม list ตัวเลือกให้ user เลือก
     ⚠️ ห้ามให้ LLM เดา bbox เอง — grounding เป็นหน้าที่ module นี้เท่านั้น (deck §5)
5. คำนวณ datetime interval จาก time_window_days (เช่น 7 → "{today-7d}/{today}")
6. เรียก GISTDA STAC Gateway (§4):
   - ตรวจสอบ/ต่ออายุ access token ก่อนเรียกทุกครั้ง (§4.2)
   - GET /{hazard}/search?bbox={bbox}&datetime={interval}&token={access_token}
7. ถ้า context.matched == 0 → คืนผลว่างพร้อมข้อความ "ไม่พบข้อมูล..." (ไม่ใช่ error)
8. สำหรับแต่ละ item ที่ได้ → อ่าน item.assets.data.href → เรียกต่ออีกชั้น (double-hop ไปยัง Vallaris Dragonfly) **พร้อมแนบ `?bbox={bbox}` เดิมซ้ำอีกครั้งในการเรียกชั้นนี้ด้วย**
   - ยกเว้น drought ที่ item.bbox เป็นค่าจริงอยู่แล้ว แต่ asset.data ก็ยังต้องเรียกเพื่อได้ polygon จริง
   - ⚠️ **จุดที่พลาดง่ายมาก (เจอเป็น bug จริงระหว่างทดสอบ):** bbox ที่ใส่ใน STAC `/search` (ข้อ 6) กรองแค่ว่า **item container ไหนตรง** (เช่น "วันนี้มีอะไรเกิดขึ้นบ้างทั่วประเทศ") ไม่ได้กรองข้อมูลจุด/พื้นที่จริงข้างในเลย — ข้อมูลจริงอยู่ที่ Vallaris Dragonfly ซึ่งต้องส่ง `bbox` ของตัวเองอีกรอบ ไม่งั้นจะได้ข้อมูลที่ไม่เกี่ยวกับพื้นที่ที่ถามมาเลย (ทดสอบจริง: ถาม fire ใกล้อยุธยา แต่ได้จุดในอินโดนีเซียกลับมา ก่อนแก้) โค้ดที่แก้แล้วอยู่ที่ `src/gistda/vallaris.rs::fetch_features` (เพิ่ม parameter `bbox` แล้ว query ต่อ)
9. รวม features จากทุก item เป็น FeatureCollection เดียว
10. สร้าง summary ข้อความสั้นๆ (ตอนนี้ใช้ template ภาษาไทยตรงๆ ใน `handlers/query.rs` ไม่ได้เรียก LLM ซ้ำ — เร็วกว่าและ deterministic)
11. คืน JSON ให้ frontend
```

---

## 3. LLM Extraction (OpenAI-compatible)

> **อัปเดต 2026-08-03: เปลี่ยนจาก Ollama local มาใช้ endpoint แบบ OpenAI-compatible แล้ว** (`src/llm/openai.rs`) — ใช้ได้กับ gateway ใดก็ได้ที่มี `/chat/completions` รวมถึง Ollama เองผ่าน `http://localhost:11434/v1`
>
> **ผลที่วัดได้: latency ลดจาก ~5.5 วินาที เหลือ ~1.1 วินาที ต่อ 1 extraction**

**Endpoint:** `POST {LLM_BASE_URL}/chat/completions` (base ต้องมี `/v1` ต่อท้าย)

```json
{
  "model": "tekton-code",
  "messages": [
    { "role": "system", "content": "<SYSTEM_PROMPT ตาม deck §4>" },
    { "role": "user",   "content": "<ข้อความจากผู้ใช้>" }
  ],
  "response_format": { "type": "json_object" },
  "stream": false,
  "temperature": 0
}
```

- `response_format: {"type":"json_object"}` คือตัวแทนของ `format:"json"` เดิมของ Ollama — **ต้องใส่เสมอ** ไม่งั้นโมเดลอาจห่อ JSON ด้วยข้อความหรือ code fence
- `temperature: 0` เพื่อให้ประโยคเดิมได้ query object เดิมทุกครั้ง
- System prompt + few-shot ต้อง copy ตรงจาก [learning-notes.md §4](../docs/learning-notes.md#4-prompt-engineering-constrained-generation-ด้วย-json-schema--few-shot-หน้า-8)
- Response อยู่ที่ `choices[0].message.content` เป็น **string** ต้อง parse อีกชั้น (`serde_json::from_str::<ExtractedQuery>(&content)`)
- `LLM_API_KEY` ว่างได้ ถ้า server ไม่ต้อง auth — client จะไม่ใส่ header `Authorization` ให้

**Struct แนะนำ:**
```rust
#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
enum Hazard { Flood, Fire, Drought }

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
enum Confidence { High, Low }

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
struct ExtractedQuery {
    hazard: Hazard,
    location_text: String,
    time_window_days: u32,
    confidence: Confidence,
    needs_clarification: bool,
}
```

**Deploy note:** ตอนนี้ LLM เป็น service ที่อยู่นอกเครื่องแล้ว (`LLM_BASE_URL`) ปัญหาเดิมที่ต้องมี Ollama รันคู่กับ backend จึงหมดไป — ขอแค่ backend เรียก URL นั้นได้ทางเครือข่าย (endpoint ปัจจุบันเป็น IP ภายในองค์กร ถ้า deploy นอกวงต้องเปิดทางให้ถึงก่อน)

---

## 4. GISTDA STAC Gateway

### 4.1 Base + endpoints ที่ทดสอบแล้ว (ยืนยันจริง)

**Base:** `https://api-gateway.gistda.or.th/api/2.0`

| Endpoint | ใช้ทำอะไร |
|---|---|
| `GET /resources/stac/{hazard}` | root catalog, มี `links[]` ชี้ไป `search`/`collections`/`conformance` |
| `GET /resources/stac/{hazard}/search?bbox=&datetime=&collections=&limit=&token=` | **endpoint หลักที่ backend ควรใช้** — ยืนยันแล้วว่า `bbox`/`datetime` filter จริง (ทดสอบ negative case ด้วย bbox นอกไทย → `matched: 0`) |
| `GET {item.assets.data.href}` (opaque `/resources/{hash}?token=`) | ต้องเรียกต่ออีกชั้น → คืน JSON ที่มี `links[].href` ชี้ไปยัง **Vallaris Dragonfly Features API** (`vallaris.dragonfly.gistda.or.th/core/api/features/1.1/collections/{id}/items?api_key=...`) — `api_key` ฝังมาให้ในนั้นแล้ว ไม่ต้องเติมเอง |

รายละเอียดโครงสร้างข้อมูลแต่ละ hazard (flood/fire/drought ต่างกันมาก) ดู [stac-api-structure.md](../docs/stac-api-structure.md) เต็มๆ — ไฟล์นั้นมี sample JSON จริงประกอบด้วย

### 4.2 Auth — ใช้ `api_key` (แนะนำ) หรือ `token` (fallback)

> **อัปเดต 2026-08-03: เปลี่ยนมาใช้ `api_key` เป็นหลักแล้ว** — ทดสอบจริงยืนยันว่า gateway รับ `?api_key=...` ได้ทั้ง `/search` และ hop 2 (`asset.data`) และ **gateway จะส่ง auth scheme เดิมต่อให้ใน href ของ `asset.data` อัตโนมัติ** ดังนั้น hop 2 ไม่ต้องทำอะไรเพิ่มเลย
>
> ผลลัพธ์เทียบกันแล้วตรงกันเป๊ะ (query เดียวกันได้ 29 จุด, bbox เดียวกัน ทั้งสองวิธี)
>
> **ข้อดีที่สำคัญที่สุด: `api_key` ไม่มีวันหมดอายุ** จึงตัดปัญหา token อายุ 1 ชั่วโมงทิ้งไปทั้งหมด และทำให้ Phase 2 (auto-refresh) **ไม่จำเป็นอีกต่อไป**

**วิธีใช้:** ใส่ `GISTDA_API_KEY` ใน `.env` เท่านั้น — backend จะเลือกใช้อัตโนมัติ (ดู `config.rs::gistda_auth`) และ `/api/health` จะรายงาน `"gistda_auth": "api_key"`

```
GET {GATEWAY_HOST}/resources/stac/{hazard}/search?bbox=...&datetime=...&api_key={KEY}
```

---

#### 4.2.1 Token flow (fallback — เก็บไว้เผื่อกรณีไม่มี api_key)

ถ้า `GISTDA_API_KEY` ว่าง backend จะ fallback ไปใช้ `GISTDA_ACCESS_TOKEN` แทน (JWT อายุ ~1 ชั่วโมง ต้อง paste ใหม่เอง) วิธีขอ token จากการอ่านโค้ด production ของ `gistda-disaster-frontend`:

**ขอ token ใหม่ (password grant, ใช้ guest account):**
```
POST {GATEWAY_HOST}/api/2.0/token
Content-Type: application/x-www-form-urlencoded

grant_type=password&accountType=email&username={GUEST_USERNAME}&password={GUEST_PASSWORD}
```
→ response: `{ "accessToken": "...", "refreshToken": "...", "user": {...} }`

**ต่ออายุด้วย refresh token:**
```
POST {GATEWAY_HOST}/api/2.0/token
Content-Type: application/x-www-form-urlencoded
Authorization: Bearer {refresh_token}

grant_type=refresh_token&refresh_token={refresh_token}
```
→ response เดิม พร้อม accessToken/refreshToken ชุดใหม่

**access token ที่ได้เป็น JWT** มี claim `exp`/`iat`/`uid`/`ses`/`grt` — วัดจริงแล้ว `exp - iat = 3600` วินาที = **อายุแค่ 1 ชั่วโมงเท่านั้น** (ไม่ใช่หลายวันอย่างที่เคยเข้าใจผิดตอนแรก)

### 4.3 สรุปสถานะเรื่อง auth

| | `api_key` (ใช้อยู่) | `token` (fallback) |
|---|---|---|
| อายุ | ไม่หมดอายุ | ~1 ชั่วโมง |
| ต้อง refresh ไหม | ไม่ต้อง | ต้อง (ทำมือ หรือ implement flow §4.2.1) |
| ใช้กับ `/search` | ✅ ทดสอบแล้ว | ✅ |
| ใช้กับ hop 2 (`asset.data`) | ✅ gateway ใส่ให้อัตโนมัติ | ✅ เหมือนกัน |
| เหมาะกับ | dev + production | dev ชั่วคราวเท่านั้น |

**Phase 2 (auto token refresh) ที่เคยวางแผนไว้ — ยกเลิกแล้ว** เพราะ `api_key` แก้ปัญหาเดียวกันโดยไม่ต้องเขียนโค้ดเพิ่ม และไม่ต้องขอ guest account จาก GISTDA อีกต่อไป
(โค้ดตรวจ `exp` ของ JWT ยังอยู่ที่ `gistda/auth.rs` — ทำงานเฉพาะตอน fallback ไปใช้ token เท่านั้น)

---

## 5. Gazetteer / Grounding Module

**ไฟล์ข้อมูล (มีอยู่แล้วใน repo นี้ ที่ `src/lib/`):**

| ไฟล์ | เนื้อหา | หมายเหตุ |
|---|---|---|
| `src/lib/bbox_pat.json` | `{name_t, name_e, area_idn, bbox}` × 8,429 | ต้นฉบับจาก `gistda-disaster-frontend` |
| `src/lib/boundary.tsx` | `provinceList`/`amphoeList`/`tambonList` (ชื่อ + parent code) | เป็น **TypeScript source ไม่ใช่ JSON** — Rust parse ตรงๆ ไม่ได้ |
| `src/lib/boundary.json` | เหมือน `boundary.tsx` แต่แปลงเป็น JSON แล้ว (สร้างไว้ให้ระหว่างวิเคราะห์) | ใช้ตัวนี้แทน `.tsx` เวลา parse ด้วย Rust |
| **`src/lib/gazetteer_merged.json`** | **merge แล้ว: `boundary.json` (hierarchy) + `bbox_pat.json` (bbox) เข้าด้วยกัน 8,429 record, join แล้ว 100% ไม่มี record ไหนขาด bbox** | **แนะนำให้ backend โหลดไฟล์นี้ไฟล์เดียว** |

**Schema ของ `gazetteer_merged.json`:**
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
(`level` เป็น `"province" | "amphoe" | "tambon"`, `parent_area_idn` เป็น `null` สำหรับ province)

**Struct แนะนำ:**
```rust
#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
enum AdminLevel { Province, Amphoe, Tambon }

#[derive(serde::Deserialize, Debug, Clone)]
struct GazetteerEntry {
    area_idn: u32,
    level: AdminLevel,
    name_th: String,
    name_en: String,
    parent_area_idn: Option<u32>,
    bbox: Option<[f64; 4]>,
}
```

**⭐ ข้อค้นพบสำคัญ:** `area_idn` **ตรงกับ `pv_idn`/`ap_idn`/`tb_idn` ที่อยู่ใน STAC drought properties เป๊ะ** (ยืนยันแล้ว เช่น `1005` = บางเขน ทั้งสองฝั่ง — ดู [gazetteer-structure.md](../docs/gazetteer-structure.md)) ดังนั้นสำหรับ drought สามารถ join ด้วยรหัสตรงๆ ได้ ไม่ต้องพึ่ง bbox intersect

### 5.1 Lookup algorithm (`location_text → GazetteerEntry`)

```
1. normalize input (trim, lowercase สำหรับเทียบ name_en เท่านั้น — ภาษาไทยไม่มี case)
2. alias table ก่อนเสมอ (ดู 5.2) → เจอ → ใช้เลย
3. exact match กับ name_en/name_th ทุก level
   → เจอ 1 → ใช้เลย
   → เจอ >1 (ambiguous, พบจริง 875/7,424 ชื่อตำบลซ้ำ) → ไปข้อ 5
4. substring match กับ name_en **และ name_th**
   ("Ayutthaya" ⊂ "Phra Nakhon Si Ayutthaya", "อยุธยา" ⊂ "พระนครศรีอยุธยา")
5. ยังไม่เจอ / ambiguous → คืน needs_clarification: true พร้อม candidate list (ห้ามเดา)
```

**⚠️ 2 จุดที่แก้ทีหลังเพราะเป็นบั๊กจริง อย่าเผลอเปลี่ยนกลับ:**

1. **substring ต้องเช็ค `name_th` ด้วย** — เดิมเช็คแค่ `name_en` ทำให้คำถามภาษาไทยที่ใช้ชื่อย่อ **พังทั้งหมด** เช่น "อยุธยา" ไม่ match "พระนครศรีอยุธยา" (substring อยู่กลางคำ ไม่ใช่ต้นคำ) ทั้งที่ฝั่งอังกฤษ "Ayutthaya" ผ่านได้ปกติ
2. **alias ต้องมาก่อน exact** — มี **ตำบลชื่อ "โคราช" อยู่จริง** (area_idn 301803 กว้างแค่ ~5 กม.) ถ้า exact มาก่อน คำถาม "โคราชแล้งไหม" จะไปค้นในตำบลจิ๋วนั้นแทนทั้งจังหวัดนครราชสีมา (และได้ 0 ผลลัพธ์เสมอ) — น่าสังเกตว่าฝั่งอังกฤษ "Korat" ไม่โดนบั๊กนี้เพราะ `name_en` ของตำบลนั้นสะกด "KHORAT" (มี H) จึงไม่ตรงกัน

ทดสอบชื่อไทยที่คนใช้จริงหลังแก้แล้ว: อยุธยา / กรุงเทพ / ภูเก็ต / สงขลา / เชียงราย / ชลบุรี / นครปฐม / หาดใหญ่ → ผ่านหมด, โคราช → ได้จังหวัดถูกต้องผ่าน alias, เชียงใหม่ / ขอนแก่น / อุบล → คืน ambiguous ให้ผู้ใช้เลือก (ถูกต้องตาม design)

### 5.2 Alias table — ต้องสร้างเพิ่มเอง (gazetteer ไม่มีให้)

ทดสอบแล้วพบว่า `"Korat"` **ไม่มีในข้อมูลเลย** (ชื่อทางการคือ `"Nakhon Ratchasima"`) — ต้องมี mapping table แยกต่างหาก เริ่มจากรายการเล็กๆ ที่พบในชีวิตจริงก่อน แล้วขยายทีหลัง:

```json
{ "korat": 30, "โคราช": 30, "hat yai": 9011, "หาดใหญ่": 9011 }
```
(key เป็น alias แบบ lowercase, value เป็น `area_idn` ใน `gazetteer_merged.json` ที่จะ redirect ไปหา — โค้ดจริงอยู่ที่ `src/gazetteer/alias.rs`)

**หลักการเลือกว่าอะไรควรอยู่ในตารางนี้:** ตารางนี้ทำงาน**ก่อน** exact match จึงเป็นการ override gazetteer — ใส่เฉพาะชื่อที่ปล่อยให้ระบบ match เองแล้ว**ได้ผลผิด**เท่านั้น อย่าใส่พร่ำเพรื่อ

- `โคราช`/`korat` → **ต้องมี** เพราะชนกับตำบลจริงชื่อ "โคราช" (ดู 5.1)
- `หาดใหญ่`/`hat yai` → **ต้องมี** และต้องชี้ไป **อำเภอหาดใหญ่ (9011)** ไม่ใช่จังหวัดสงขลา (90) อย่างที่เคยตั้งไว้ตอนแรก เพราะคนพูดว่า "หาดใหญ่" หมายถึงอำเภอ ไม่ใช่ทั้งจังหวัด — และยังช่วยตัดปัญหาชื่อ romanize ชนกับตำบล "หาดยาย" (860413, ชุมพร) ที่ `name_en` สะกดเป็น "HAT YAI" เหมือนกันเป๊ะ
- ชื่อย่อไทยส่วนใหญ่ **ไม่ต้องใส่** เพราะ substring match จัดการให้แล้ว (อยุธยา, กรุงเทพ ฯลฯ)

---

## 6. HTTP API ที่ backend expose ให้ frontend

> **สถานะ: implement แล้ว จริง** — รายละเอียด request/response ทุก field, ทุก error case, และโครงสร้าง `geojson.features` ต่อ hazard อยู่ที่ **[API.md](API.md)** (เอกสารแยกเอาไว้ส่งให้ทีม frontend โดยเฉพาะ ไม่ปนกับ implementation notes ในไฟล์นี้) — ไฟล์นี้เก็บไว้แค่สรุปสั้นๆ

- `POST /api/query` — รับ `{ "text": string }` คืน 1 ใน 3 แบบ: ต้องการ clarification / พบข้อมูล / ไม่พบข้อมูล (ไม่ใช่ error) ดูตัวอย่างเต็มและ error status codes ทั้งหมดที่ [API.md §2](API.md#2-post-apiquery)
- `GET /api/health` — เช็ค LLM เข้าถึงได้ (ping `/models`) + gazetteer โหลดสำเร็จ + GISTDA token ยังไม่หมดอายุ ดู [API.md §3](API.md#3-get-apihealth)
- `GET /api/gazetteer/search?q=` — **ยังไม่ implement** (optional, เผื่ออนาคตทำ UI แบบ dropdown suggestion)
- CORS เปิดแบบ permissive ไว้ก่อนสำหรับ dev (ดู `src/main.rs`) — ต้อง lock ก่อน production

---

## 7. Basemap — ไม่ใช่หน้าที่ backend แต่บันทึกไว้ที่นี่

> อัปเดต: เปลี่ยนจาก Vallaris Maps style มาใช้ **MapTiler** แล้ว (ผู้ใช้ให้ API key มาโดยตรง ไม่ใช้ Vallaris style อีกต่อไป) — รองรับสลับ 2 แบบ: street กับ satellite/hybrid

`src/components/DisasterMap.tsx` สร้าง style URL จาก `NEXT_PUBLIC_MAPTILER_KEY` เอง (ไม่ใช่ full style URL แบบเดิม):

```ts
const STYLE_URL = {
  street: `https://api.maptiler.com/maps/streets-v2/style.json?key=${MAPTILER_KEY}`,
  satellite: `https://api.maptiler.com/maps/hybrid/style.json?key=${MAPTILER_KEY}`,
};
```

มีปุ่มสลับ "ถนน"/"ดาวเทียม" overlay บนแผนที่ (มุมซ้ายบน) เรียก `map.setStyle(...)` — เนื่องจาก `setStyle()` ล้าง source/layer เดิมทั้งหมด จึงต้อง re-add source (`query-result`) + layer (fill/line/circle) ใหม่ทุกครั้งใน handler ของ event `style.load` (event นี้ยิงทั้งตอน initial load และหลัง `setStyle()` ทุกครั้ง) — ไม่ใช่แค่ครั้งแรกตอน mount

**นี่คือ MapLibre `style` เรื่องของ frontend ล้วนๆ** — backend ไม่ต้อง proxy หรือประมวลผลอะไรเลย เก็บ key ไว้เป็น `NEXT_PUBLIC_MAPTILER_KEY` ใน `.env.local` ของฝั่ง Next.js (ไม่ใช่ backend `.env`) เพราะ `NEXT_PUBLIC_*` ต้อง build เข้า client bundle

> ⚠️ API key นี้ฝังไปกับทุก request ที่ browser ยิงตรงไปหา MapTiler — เหมือน token อื่นๆ ห้าม commit ค่าจริงเข้า git เก็บใน `.env.local` (มี `.env*` ใน `.gitignore` อยู่แล้วที่ root)

---

## 8. Environment Variables

สร้างไฟล์ `.env` ในโฟลเดอร์นี้ (ไม่ commit) — ตัวแปรที่ต้องมี:

| ตัวแปร | ตัวอย่าง | หมายเหตุ |
|---|---|---|
| `LLM_BASE_URL` | `http://172.30.20.35:20128/v1` | ต้องมี `/v1` ต่อท้าย |
| `LLM_API_KEY` | *(key ของ gateway)* | ว่างได้ถ้า server ไม่ต้อง auth |
| `LLM_MODEL` | `tekton-code` | |
| `GISTDA_GATEWAY_HOST` | `https://api-gateway.gistda.or.th/api/2.0` | |
| `GISTDA_API_KEY` | *(key จาก GISTDA)* | **ใช้ตัวนี้** — ไม่หมดอายุ ดู §4.2 |
| `GISTDA_ACCESS_TOKEN` | *(ว่างไว้)* | fallback เมื่อไม่มี api_key — JWT อายุ ~1 ชม. |
| `GISTDA_GUEST_USERNAME` | *(ไม่ใช้แล้ว)* | เคยไว้สำหรับ Phase 2 ซึ่งยกเลิกแล้ว |
| `GISTDA_GUEST_PASSWORD` | *(ไม่ใช้แล้ว)* | เคยไว้สำหรับ Phase 2 ซึ่งยกเลิกแล้ว |
| `GAZETTEER_PATH` | `../src/lib/gazetteer_merged.json` | |
| `PORT` | `8080` | |
| `RUST_LOG` | `info` | |

---

## 9. โครงสร้างโปรเจกต์ที่แนะนำ

```
backend/
├── Cargo.toml
├── .env               (gitignored — token จริง)
├── .env                (local only — gitignored)
├── README.md           (ไฟล์นี้)
└── src/
    ├── main.rs          (setup axum router, load gazetteer ตอน startup)
    ├── config.rs         (อ่าน env vars)
    ├── llm/
    │   ├── mod.rs
    │   ├── openai.rs     (เรียก /chat/completions, parse ExtractedQuery)
    │   └── prompt.rs     (system prompt + few-shot ตาม deck §4)
    ├── gazetteer/
    │   ├── mod.rs
    │   ├── loader.rs     (โหลด gazetteer_merged.json ตอน startup, เก็บใน memory)
    │   ├── lookup.rs     (algorithm §5.1)
    │   └── alias.rs      (alias table §5.2)
    ├── gistda/
    │   ├── mod.rs
    │   ├── auth.rs        (token provider, §4.2/4.3)
    │   ├── stac.rs         (เรียก /search, parse item)
    │   └── vallaris.rs     (double-hop asset fetch)
    ├── handlers/
    │   ├── mod.rs
    │   ├── query.rs        (POST /api/query — orchestrate ทั้ง pipeline)
    │   └── health.rs
    └── models.rs           (struct ที่ share กันทั้งโปรเจกต์)
```

**Crate ที่แนะนำ:**

| Crate | ใช้ทำอะไร |
|---|---|
| `axum` | HTTP framework (ทางเลือก: `actix-web`) |
| `tokio` | async runtime |
| `reqwest` | HTTP client (เรียก LLM, GISTDA, Vallaris) |
| `serde` / `serde_json` | (de)serialize JSON ทั้งหมด |
| `dotenvy` | โหลด `.env` |
| `tracing` / `tracing-subscriber` | logging |
| `thiserror` / `anyhow` | error handling |
| `chrono` | คำนวณ datetime interval จาก `time_window_days` |
| `once_cell` (หรือ `std::sync::OnceLock`) | เก็บ gazetteer ที่โหลดครั้งเดียวตอน startup ใน memory (8,429 record ไม่ใหญ่ โหลดเข้า `Vec`/`HashMap` ได้เลยไม่ต้องใช้ DB) |

---

## 10. Error Handling & Edge Cases (สรุปจาก deck + สิ่งที่ทดสอบจริง)

| กรณี | ต้องทำอะไร |
|---|---|
| Gemma 3 คืน JSON ผิด schema | retry 1 ครั้ง, ถ้ายังผิดอีก → 500 พร้อม log แต่ไม่ leak prompt/response ให้ user |
| `location_text` หา gazetteer ไม่เจอ/กำกวม | `needs_clarification: true` — **ห้ามเดา bbox เอง** |
| STAC `/search` คืน `context.matched: 0` | ไม่ใช่ error — คืน summary "ไม่พบข้อมูล" + `features: []` |
| Vallaris double-hop คืน `numberMatched: 0` | เหมือนกัน ไม่ใช่ error (เจอจริงตอนทดสอบ flood — ไม่มีน้ำท่วมช่วงที่ทดสอบ) |
| credential ไม่ถูกต้อง | เช็คก่อนเรียกทุกครั้ง (`config::gistda_credential_valid`): api_key แค่ต้องไม่ว่าง, token ต้องเช็ค `exp` claim ด้วย → ถ้าไม่ผ่านคืน 503 พร้อมข้อความชัดเจน ไม่ปล่อยให้ GISTDA คืน 401 เฉยๆ |
| LLM ไม่ตอบ/timeout | คืน 503 ให้ frontend รู้ว่าเป็นปัญหาที่ LLM ไม่ใช่คำถามผิด |
| fire/drought มี volume สูงมาก (fire หมื่น+ จุด/วัน, drought 85k+ polygon/collection) | **ต้องมี limit ที่ query ระดับ STAC/Vallaris เสมอ** ห้ามดึงทั้งหมดมา filter เอง — ใส่ default `limit` ที่ backend เอง (ไม่พึ่งฝั่ง client ส่งมา) |

---

## 11. Caching (แนะนำ ไม่บังคับสำหรับ v1)

- **Gazetteer:** โหลดครั้งเดียวตอน startup เก็บใน memory — ไม่ต้อง cache เพิ่ม (ข้อมูลนิ่ง)
- **STAC/Vallaris responses:** production frontend (`gistda-disaster-frontend`) ใช้ Redis เก็บ mapping ระหว่าง gateway resource hash ↔ resolved collection/tile id (ดู `src/lib/redis.ts` ของโปรเจกต์นั้น) — สำหรับ v1 ของเราไม่จำเป็นต้องมี Redis เลย เพราะ query ต่อครั้งมี scope แคบ (bbox+datetime เฉพาะคำถามเดียว) ไม่ได้โหลดทั้งประเทศเหมือน dashboard — พิจารณาเพิ่มทีหลังถ้า latency เป็นปัญหาจริง (deck §7 เตือนเรื่อง latency ไว้แล้ว)

---

## 12. คำถามที่ยังตอบไม่ได้ — ต้องตัดสินใจ/หาข้อมูลเพิ่มก่อน implement เต็มรูปแบบ

พบจากการอ่านโค้ด production ของ `gistda-disaster-frontend` ว่า**มี API surface คู่ขนานอีกชุดที่เราไม่มี credential** และอาจจะง่ายกว่าที่เราทดสอบไว้:

1. **`core/api/stac/1.0/{hazard}/search?api_key=...`** (env: `NEXT_PUBLIC_VA_HOST` / `NEXT_PUBLIC_VA_KEY`) — ดูเหมือนเป็น Vallaris-native STAC surface ที่ asset href ถูก resolve มาให้แล้วบางส่วน (ไม่ต้อง double-hop แบบที่เราเจอ) แถมมี fire collections แบบ **rolling window** (`viirs_1day/3days/7days/30days`, `modis_1day/3days/7days/30days`) ที่เราไม่เคยเจอตอนสำรวจ `api-gateway.gistda.or.th` เอง — **ยังไม่มี credential ทดสอบ**
2. **`{GATEWAY_HOST}/api/2.0/resources/{flood|viirs}/{1day|3days|7days|30days}?api_key=...`** และ **`/resources/summary/disaster?api_key=...`**, **`/resources/analytics/eco-plant/{rice|maize|cassava|sugarcane}?api_key=...`** (env: `NEXT_PUBLIC_GATEWAY_API_KEY`) — REST surface ที่ไม่ใช่ STAC เลย อาจจะคืนข้อมูลพร้อมใช้ทันทีโดยไม่ต้อง query+double-hop แบบที่เอกสารนี้ออกแบบไว้ — **ยังไม่มี credential ทดสอบเช่นกัน (คนละ key จาก `token` ที่มี)**
3. `GISTDA_GUEST_USERNAME`/`GISTDA_GUEST_PASSWORD` (§4.2) — ยังไม่มี ต้องขอจาก GISTDA ถ้าจะทำ Phase 2 (auto-refresh)

**ผลกระทบ:** spec นี้ออกแบบตาม surface ที่ #1 (`api-gateway.gistda.or.th/api/2.0/resources/stac`) เพราะเป็นอันเดียวที่มี token ทดสอบแล้วจริง ถ้าได้ credential ของ surface (1) หรือ (2) มาเพิ่มในอนาคต **ควรกลับมาทดสอบเทียบกันก่อน** เพราะอาจลดความซับซ้อนของ backend ได้มาก (ตัด double-hop, ได้ pre-aggregated summary ฟรี)

---

## 13. Testing

- Unit test gazetteer lookup module ด้วย test case 8 ข้อจาก [learning-notes.md §6](../docs/learning-notes.md#6-evaluation-design-หน้า-11-14) โดยเฉพาะ "Korat" (ต้องผ่าน alias table) และ "Ayutthaya" (ต้องผ่าน substring match)
- Integration test `/api/query` แบบ mock GISTDA/LLM response (ยังคงไม่ยิง API จริงใน CI เพื่อไม่ให้ test ขึ้นกับข้อมูลจริงที่เปลี่ยนทุกวัน)
- Smoke test กับ API จริงทำได้สะดวกขึ้นแล้วตั้งแต่เปลี่ยนมาใช้ `api_key` เพราะไม่ต้องคอยเปลี่ยน credential ก่อนรันทุกครั้ง
