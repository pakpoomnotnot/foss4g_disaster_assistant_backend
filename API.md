# askme-disaster Backend — API Reference (สำหรับทีม Frontend)

> เอกสารนี้คือ contract ที่ implement จริงแล้ว (ไม่ใช่ spec) — ตรงกับโค้ดใน `src/handlers/` ทุกจุด
> ถ้าพฤติกรรมจริงกับเอกสารนี้ไม่ตรงกัน ให้ถือว่าเป็นบั๊ก แจ้งทีม backend ได้เลย
> ดู [README.md](README.md) ถ้าอยากรู้ว่าข้างในทำงานยังไง (LLM prompt, gazetteer, GISTDA auth ฯลฯ) — ไฟล์นี้พูดแค่ contract ที่ frontend ต้องคุยด้วย

---

## 1. Base URL

Dev: `http://localhost:8080` (พอร์ตตั้งได้ผ่าน `PORT` env var ฝั่ง backend)

**CORS:** เปิดแบบ permissive (`Access-Control-Allow-Origin: *`) สำหรับตอนนี้ — เรียกจาก origin ไหนก็ได้ตอน dev ไม่ต้องตั้งค่าอะไรฝั่ง frontend เพิ่ม (จะถูก lock ลงก่อน production)

**Content-Type:** ทุก request/response เป็น `application/json`

---

## 2. `POST /api/query`

รับคำถามภาษาธรรมชาติ (ไทย/อังกฤษ) แล้วคืน GeoJSON + สรุปข้อความ

### Request

```json
{ "text": "flooding in Ayutthaya this week" }
```

| field | type | required |
|---|---|---|
| `text` | string | ใช่ |

### Response — เป็นไปได้ 3 รูปแบบหลัก ทุกอันคืน HTTP 200 (ยกเว้น error case ในหัวข้อ 2.4)

ก่อนอ่านโค้ด frontend ให้เช็ค field `needs_clarification` ก่อนเสมอ — ถ้า `true` แปลว่า **ไม่มี** `geojson`/`bbox` ในตอบกลับ ต้องเอา `message`/`candidates` ไปแสดงถาม user ต่อ ไม่ใช่พยายาม render แผนที่

#### 2.1 ต้องการความชัดเจนเพิ่ม (`needs_clarification: true`)

เกิดได้จาก 3 สาเหตุ: (a) LLM เองแยก hazard/location ไม่ออกจากประโยค, (b) ชื่อสถานที่หาไม่เจอในฐานข้อมูล, (c) ชื่อสถานที่ตรงกับมากกว่า 1 แห่ง (เช่น "Chiang Mai" ตรงทั้งจังหวัดเชียงใหม่ และตำบลหนึ่งในร้อยเอ็ดที่บังเอิญสะกดเหมือนกัน)

```json
{
  "needs_clarification": true,
  "message": "พบชื่อ \"Bang Phli\" หลายแห่ง กรุณาระบุให้ชัดเจนขึ้น",
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

(ตัวอย่างจริงจากข้อมูล — "Bang Phli" ตรงกับทั้งอำเภอในสมุทรปราการ และตำบลในอยุธยาที่บังเอิญสะกดเหมือนกัน)

`candidates` เป็น `[]` (array ว่าง) ในกรณี (a) และ (b) — มีแต่กรณี (c) เท่านั้นที่มี candidate list ให้เลือก จำกัดไว้สูงสุด 10 รายการ

**Candidate object:**

| field | type | หมายเหตุ |
|---|---|---|
| `area_idn` | number | รหัสพื้นที่ ใช้ถ้าจะทำ UI แบบเลือกจาก dropdown แล้วส่ง query ใหม่ที่ระบุชัดขึ้น |
| `level` | `"province"` \| `"amphoe"` \| `"tambon"` | |
| `name_th` | string | |
| `name_en` | string | |
| `province` | string \| null | ชื่อจังหวัด (name_en) ที่ candidate นั้นสังกัด — `null` ถ้า candidate เป็น province เอง หรือหา parent ไม่เจอ |

**แนะนำ UX:** ให้ user คลิกเลือก candidate แล้วส่ง query ใหม่ที่ระบุชื่อจังหวัด/อำเภอชัดเจนขึ้น (เช่น ต่อ `name_en` + `province` เป็นข้อความใหม่) — backend ไม่มี endpoint สำหรับส่ง `area_idn` ตรงๆ ใน v1 นี้

#### 2.2 พบข้อมูล (`needs_clarification: false`, มี features)

```json
{
  "needs_clarification": false,
  "hazard": "flood",
  "location_text": "Ayutthaya",
  "resolved_area": {
    "area_idn": 14,
    "level": "province",
    "name_th": "พระนครศรีอยุธยา",
    "name_en": "Phra Nakhon Si Ayutthaya"
  },
  "time_window_days": 7,
  "summary": "พบ 3 พื้นที่น้ำท่วมในพระนครศรีอยุธยา ช่วง 7 วันที่ผ่านมา",
  "geojson": { "type": "FeatureCollection", "features": [ /* ดูหัวข้อ 4 */ ] },
  "bbox": [100.212706, 14.111092, 100.82323, 14.673195]
}
```

#### 2.3 ไม่พบข้อมูลในเงื่อนไขนี้ (`needs_clarification: false`, features ว่าง — **ไม่ใช่ error**)

รูปแบบเดียวกับ 2.2 ทุกอย่าง ต่างแค่:

```json
{
  "...": "... (fields อื่นเหมือน 2.2) ...",
  "summary": "ไม่พบข้อมูลน้ำท่วมในพระนครศรีอยุธยา ช่วง 7 วันที่ผ่านมา",
  "geojson": { "type": "FeatureCollection", "features": [] },
  "bbox": null
}
```

Frontend ควรแสดงข้อความ `summary` เฉยๆ (ไม่ zoom แผนที่ เพราะ `bbox` เป็น `null`) — ห้าม treat เป็น error state

**Response fields ทั้งหมดของ 2.2/2.3:**

| field | type | หมายเหตุ |
|---|---|---|
| `needs_clarification` | `false` | |
| `hazard` | `"flood"` \| `"fire"` \| `"drought"` | |
| `location_text` | string | ข้อความสถานที่ตามที่ LLM ดึงมา (ไม่ normalize) |
| `resolved_area` | object | `{ area_idn, level, name_th, name_en }` — เหมือน candidate แต่ไม่มี `province` field |
| `time_window_days` | `1` \| `3` \| `7` \| `30` | |
| `summary` | string | ข้อความสรุปภาษาไทย เอาไปแสดงตรงๆ ได้ |
| `geojson` | `FeatureCollection` | ดูหัวข้อ 4 — โครงสร้าง properties **ต่างกันตาม hazard มาก** |
| `bbox` | `[minLon, minLat, maxLon, maxLat]` \| `null` | คำนวณจาก geometry จริงของ features ที่ได้ (ไม่ใช่ bbox ของพื้นที่ที่ query) — ใช้กับ `map.fitBounds()` ได้ตรงๆ, `null` เมื่อไม่มี feature |

#### 2.4 Error responses (ไม่ใช่ 200)

```json
{ "error": "GISTDA credential missing or expired — set GISTDA_API_KEY in .env" }
```

| HTTP status | เมื่อไหร่เกิด | `error` message |
|---|---|---|
| 503 | LLM ไม่ตอบ/timeout/คืน non-2xx | `"LLM unavailable or timed out"` |
| 500 | LLM ตอบ JSON ผิด schema (แม้ retry 1 ครั้งแล้ว) | `"failed to parse LLM response"` |
| 503 | GISTDA credential ใช้ไม่ได้ (ไม่ได้ตั้งค่า หรือ — เฉพาะกรณี fallback ไปใช้ token — หมดอายุแล้ว) | `"GISTDA credential missing or expired — set GISTDA_API_KEY in .env"` |
| 502 | GISTDA STAC search หรือ Vallaris asset fetch ล้มเหลว/timeout | `"upstream GISTDA/Vallaris error"` |
| 500 | error ภายในอื่นๆ ที่ไม่คาดคิด | `"internal server error"` |

**แนะนำ frontend:** แสดง toast/banner ทั่วไปว่า "ระบบขัดข้อง ลองใหม่อีกครั้ง" สำหรับทุก error case ข้างบน — message ที่ backend คืนมาตั้งใจเขียนให้ dev อ่าน ไม่ใช่ user-facing copy (โดยเฉพาะ 503 credential เป็นปัญหาฝั่ง backend config ล้วนๆ ผู้ใช้แก้เองไม่ได้)

---

## 3. `GET /api/health`

Health check เฉยๆ ไม่ต้องส่ง body

```json
{
  "status": "ok",
  "gazetteer_loaded": true,
  "gazetteer_entries": 8429,
  "gistda_token_valid": true,
  "gistda_auth": "api_key",
  "llm_reachable": true,
  "llm_model": "tekton-code"
}
```

- HTTP 200 ถ้า `status: "ok"` (ทุก field เป็น `true`/มีค่า), HTTP 503 ถ้า `status: "degraded"` (field ใด field หนึ่งเป็น `false`)
- ไม่ต้อง poll ถี่ๆ ฝั่ง frontend — เอาไว้เช็คตอน boot หรือแสดง system status banner ก็พอ

---

## 4. โครงสร้าง `geojson.features` — **ต่างกันตาม `hazard` มาก อ่านก่อน render**

Backend ไม่ normalize properties ของแต่ละ feature เลย ส่งต่อจาก GISTDA/Vallaris ตรงๆ (เว้นแต่คำนวณ `bbox` ที่ response level ให้แล้ว) ดังนั้น shape ของ `properties` ในแต่ละ feature ขึ้นกับ `hazard` ที่ query:

### `hazard: "flood"` — polygon พื้นที่น้ำท่วม

```json
{
  "type": "Feature",
  "geometry": { "type": "Polygon", "coordinates": [ /* ... */ ] },
  "properties": { /* เดิมทีมัก near-empty ในข้อมูลตัวอย่างที่ทดสอบ — อย่า assume field ตายตัว */ }
}
```

### `hazard: "fire"` — point จุดความร้อนดาวเทียม (VIIRS/MODIS)

```json
{
  "type": "Feature",
  "geometry": { "type": "Point", "coordinates": [122.7704, -8.5391] },
  "properties": {
    "acq_date": "2025-01-31",
    "acq_time": "1734",
    "instrument": "VIIRS",
    "satellite": "N",
    "confidence": "nominal",
    "frp": 2.95,
    "changwat": null,
    "amphoe": null,
    "tambol": null,
    "ct_en": "Indonesia",
    "ct_tn": "อินโดนีเซีย"
  }
}
```

จุดสำคัญสำหรับ frontend:
- `confidence` (`"low"` \| `"nominal"` \| `"high"`) และ `frp` (Fire Radiative Power, ตัวเลข) มีประโยชน์สำหรับ styling ตาม severity
- `changwat`/`amphoe`/`tambol` เป็นชื่อเขตปกครองไทย (reverse-geocoded มาให้แล้วจาก provider) — เป็น `null` ถ้าจุดอยู่นอกประเทศไทย ห้าม assume ว่ามีค่าเสมอ

### `hazard: "drought"` — polygon ระดับตำบลทั่วประเทศ

```json
{
  "type": "Feature",
  "geometry": { "type": "MultiPolygon", "coordinates": [ /* ... */ ] },
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

จุดสำคัญสำหรับ frontend:
- มีชื่อจังหวัด/อำเภอ/ตำบลติดมาในทุก polygon แล้ว (`pv_en`/`ap_en`/`tb_en`) ใช้ทำ tooltip/popup ได้ตรงๆ
- `dri_mean` (ค่าดัชนี drought risk) มักเป็น `null` ในข้อมูลที่ทดสอบได้ — ถ้าจะทำ choropleth ตาม severity ต้อง handle `null` case ไว้ด้วย

**สรุป:** ก่อน render popup/tooltip ให้ switch ตาม `hazard` field ที่ response level เสมอ อย่าพยายามอ่าน property ชื่อเดียวกันข้าม hazard เพราะ schema เป็นคนละชุดกันจริงๆ

---

## 5. Enum values อ้างอิง

```ts
type Hazard = "flood" | "fire" | "drought";
type TimeWindowDays = 1 | 3 | 7 | 30;
type AdminLevel = "province" | "amphoe" | "tambon";
```

---

## 6. ตัวอย่าง curl (ทดสอบแล้วจริงกับ backend นี้)

```bash
curl -s -X POST http://localhost:8080/api/query \
  -H "Content-Type: application/json" \
  -d '{"text": "is Korat in drought?"}'

curl -s http://localhost:8080/api/health
```

---

## 7. สิ่งที่ยังไม่มีใน v1 (เผื่อวางแผน UI ล่วงหน้า)

- ไม่มี endpoint สำหรับ autocomplete/ค้นหาสถานที่แยก (`GET /api/gazetteer/search` เป็น optional ใน backend README §6 ยังไม่ implement)
- ไม่มี pagination ใน `/api/query` — backend จำกัด result ไว้ที่ระดับ 50 items/collection เองแล้ว (ป้องกัน fire/drought volume สูงเกินไป) frontend ไม่ต้องส่ง limit/offset มา
- ไม่มี auth/API key ฝั่ง backend นี้เอง (คนละชั้นกับ GISTDA token ที่ backend ถือไว้เอง) — ถ้าจะ deploy public ต้องคุยเรื่องนี้เพิ่ม
