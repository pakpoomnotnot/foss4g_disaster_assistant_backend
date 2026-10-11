//! Aggregates a result set into the handful of numbers worth showing.
//!
//! Deliberately computed here rather than by the LLM. The model is given only
//! this summary and asked to explain it — it never sees the raw features and
//! never counts anything, so it cannot invent a hotspot total or a district
//! that is not in the data.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

use crate::lang::Lang;
use crate::models::{AdminLevel, GazetteerEntry, Hazard};

/// Rows beyond this are folded into an "อื่นๆ" bucket — a table of 180 tambons
/// is not an insight.
const TOP_N: usize = 8;

#[derive(Debug, Serialize)]
pub struct Bucket {
    pub name: String,
    pub count: usize,
    /// Area in km², present only for hazards measured by extent rather than by
    /// occurrence. Flood rows are ranked by this, not by `count`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_km2: Option<f64>,
    /// Percentage of the total, rounded to one decimal.
    pub share: f64,
    /// True only for the folded remainder row. Flagged rather than left for the
    /// client to detect by name: it is a residual, not a place, so it must not
    /// be scaled against real rows (its total is routinely larger than the
    /// biggest single one) nor coloured as if it were one.
    pub other: bool,
}

#[derive(Debug, Serialize)]
pub struct Metric {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct Stats {
    pub total: usize,
    /// Admin level of `breakdown` — one step below the area that was queried.
    pub sub_level: Option<AdminLevel>,
    pub sub_level_label: Option<String>,
    /// Sub-areas ranked by count.
    pub breakdown: Vec<Bucket>,
    /// How many distinct sub-areas had at least one feature.
    pub sub_areas_affected: usize,
    /// Secondary categorical splits (land use, risk class, …), keyed by a Thai
    /// title so the client can render each as its own block.
    pub categories: Vec<Category>,
    /// Per-day counts, only for hazards whose features carry an event date.
    pub timeline: Vec<Bucket>,
    pub metrics: Vec<Metric>,
}

#[derive(Debug, Serialize)]
pub struct Category {
    pub title: String,
    pub buckets: Vec<Bucket>,
}

fn prop_str(f: &Value, key: &str) -> Option<String> {
    let v = f.get("properties")?.get(key)?;
    let s = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!s.is_empty() && s != "null").then_some(s)
}

fn prop_f64(f: &Value, key: &str) -> Option<f64> {
    match f.get("properties")?.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Ranks counts, keeps the top `TOP_N` and folds the rest into "อื่นๆ".
fn rank(counts: HashMap<String, usize>, total: usize, lang: Lang) -> Vec<Bucket> {
    rank_with_area(counts, &HashMap::new(), total, lang)
}

/// Ranks by area when areas are supplied, otherwise by count.
///
/// Flood is measured by extent: an amphoe split into many small polygons is not
/// worse off than one covered by a single large polygon, so ordering its rows by
/// how many polygons they contain would be meaningless.
fn rank_with_area(
    counts: HashMap<String, usize>,
    areas: &HashMap<String, f64>,
    total: usize,
    lang: Lang,
) -> Vec<Bucket> {
    let by_area = !areas.is_empty();
    let mut rows: Vec<(String, usize)> = counts.into_iter().collect();
    if by_area {
        rows.sort_by(|a, b| {
            let (x, y) = (
                areas.get(&a.0).copied().unwrap_or(0.0),
                areas.get(&b.0).copied().unwrap_or(0.0),
            );
            y.partial_cmp(&x)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
    } else {
        // Count desc, then name asc so equal counts do not shuffle between calls.
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    }

    let share = |n: usize| {
        if total == 0 {
            0.0
        } else {
            ((n as f64 / total as f64) * 1000.0).round() / 10.0
        }
    };

    let mut out: Vec<Bucket> = rows
        .iter()
        .take(TOP_N)
        .map(|(name, count)| Bucket {
            name: name.clone(),
            count: *count,
            area_km2: areas.get(name).map(|m2| round1(m2 / 1_000_000.0)),
            share: share(*count),
            other: false,
        })
        .collect();

    if rows.len() > TOP_N {
        let rest: usize = rows[TOP_N..].iter().map(|(_, c)| c).sum();
        if rest > 0 {
            let rest_area: f64 = rows[TOP_N..]
                .iter()
                .filter_map(|(name, _)| areas.get(name))
                .sum();
            out.push(Bucket {
                name: match lang {
                    Lang::Th => format!("อื่นๆ ({} พื้นที่)", rows.len() - TOP_N),
                    Lang::En => format!("Others ({} areas)", rows.len() - TOP_N),
                },
                count: rest,
                area_km2: by_area.then(|| round1(rest_area / 1_000_000.0)),
                share: share(rest),
                other: true,
            });
        }
    }
    out
}

/// The land-use fields arrive in Thai only — unlike place names, the feed has
/// no `lu_name_en` to switch to. It is a closed set of 13 values (the two
/// legend tables), so an English answer translates them rather than dropping
/// Thai rows into an otherwise English table.
const LAND_USE_EN: &[(&str, &str)] = &[
    // lu_name — who is responsible for the land
    ("ป่าอนุรักษ์", "Conservation forest"),
    ("เขต สปก.", "ALRO land"),
    ("ป่าสงวนแห่งชาติ", "National reserved forest"),
    ("พื้นที่ริมทางหลวง", "Highway margin"),
    ("พื้นที่เกษตร", "Agricultural land"),
    ("ชุมชนและอื่น ๆ", "Community and other"),
    ("นอกประเทศ", "Outside Thailand"),
    // lu_hp_name — what the land is used for
    ("พื้นที่ป่า", "Forest"),
    ("อ้อย", "Sugarcane"),
    ("นาข้าว", "Paddy field"),
    ("ข้าวโพดและไร่หมุนเวียน", "Maize and shifting cultivation"),
    ("เกษตรอื่น ๆ", "Other agriculture"),
    ("อื่น ๆ", "Other"),
];

fn land_use(raw: &str, lang: Lang) -> String {
    match lang {
        Lang::Th => raw.to_string(),
        Lang::En => LAND_USE_EN
            .iter()
            .find(|(th, _)| *th == raw)
            .map(|(_, en)| en.to_string())
            // An unmapped value keeps its Thai text rather than being hidden —
            // a visible untranslated row is a bug report; a missing one is not.
            .unwrap_or_else(|| raw.to_string()),
    }
}

fn count_by(features: &[Value], key: &str) -> HashMap<String, usize> {
    let mut m = HashMap::new();
    for f in features {
        if let Some(v) = prop_str(f, key) {
            *m.entry(v).or_insert(0) += 1;
        }
    }
    m
}

fn category(
    features: &[Value],
    key: &str,
    title: &str,
    total: usize,
    lang: Lang,
) -> Option<Category> {
    let raw = count_by(features, key);
    if raw.is_empty() {
        return None;
    }
    // Translate before ranking so two Thai values mapping to one English label
    // merge into a single row instead of appearing twice.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (name, n) in raw {
        *counts.entry(land_use(&name, lang)).or_insert(0) += n;
    }
    Some(Category {
        title: title.to_string(),
        buckets: rank(counts, total, lang),
    })
}

/// One level below the queried area — a province question is answered by
/// naming amphoes, an amphoe question by naming tambons. A tambon is already
/// the finest level the data carries, so it gets no breakdown.
/// Returns the level below `level`, the feature property to group by, and the
/// Thai label for it. The property differs per language on purpose: the data
/// carries both `ap_tn`/`ap_en`, and grouping by the Thai field in an English
/// answer would produce an English table full of Thai row names.
fn sub_level_of(
    level: AdminLevel,
    lang: Lang,
) -> Option<(AdminLevel, &'static str, &'static str)> {
    match level {
        AdminLevel::Province => Some((
            AdminLevel::Amphoe,
            lang.pick("ap_tn", "ap_en"),
            lang.pick("อำเภอ", "district"),
        )),
        AdminLevel::Amphoe => Some((
            AdminLevel::Tambon,
            lang.pick("tb_tn", "tb_en"),
            lang.pick("ตำบล", "subdistrict"),
        )),
        AdminLevel::Tambon => None,
    }
}

const RANGE_LABEL_TH: &[(&str, &str)] = &[
    ("no_risk", "ไม่เสี่ยง"),
    ("low_risk", "เสี่ยงต่ำ"),
    ("medium_risk", "เสี่ยงปานกลาง"),
    ("high_risk", "เสี่ยงสูง"),
    ("very_high_risk", "เสี่ยงสูงมาก"),
    ("very_low_moisture", "ชื้นน้อยที่สุด"),
    ("low_moisture", "ชื้นน้อย"),
    ("medium_moisture", "ชื้นปานกลาง"),
    ("high_moisture", "ชื้นมาก"),
    ("very_high_moisture", "ชื้นมากที่สุด"),
    ("low_moisture_high_impact", "ความชื้นต่ำ ผลต่อพืชมาก"),
    ("low_moisture_medium_impact", "ความชื้นต่ำ ผลต่อพืชปานกลาง"),
    ("low_moisture_low_impact", "ความชื้นต่ำ ผลต่อพืชน้อย"),
];

const RANGE_LABEL_EN: &[(&str, &str)] = &[
    ("no_risk", "no risk"),
    ("low_risk", "low risk"),
    ("medium_risk", "medium risk"),
    ("high_risk", "high risk"),
    ("very_high_risk", "very high risk"),
    ("very_low_moisture", "driest"),
    ("low_moisture", "dry"),
    ("medium_moisture", "moderate"),
    ("high_moisture", "moist"),
    ("very_high_moisture", "wettest"),
    ("low_moisture_high_impact", "low moisture, high impact on crops"),
    ("low_moisture_medium_impact", "low moisture, moderate impact on crops"),
    ("low_moisture_low_impact", "low moisture, low impact on crops"),
];

fn range_label(raw: &str, lang: Lang) -> String {
    let table = match lang {
        Lang::Th => RANGE_LABEL_TH,
        Lang::En => RANGE_LABEL_EN,
    };
    table
        .iter()
        .find(|(k, _)| *k == raw)
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| raw.to_string())
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Flood cells are re-observed on every satellite pass, so the same H3 cell
/// arrives once per acquisition date. Summing across them counts the same
/// ground several times over: a December query for Ayutthaya held 38,190
/// features but only 10,589 distinct cells, and the naive sum came to 2,465 km²
/// — 96% of the province — where the real flooded extent was 612 km².
///
/// Keeps the first sighting of each cell. Features with no `h3_address` cannot
/// be de-duplicated and are all kept, since dropping them would lose real area.
fn distinct_cells(features: &[Value]) -> Vec<&Value> {
    let mut seen = std::collections::HashSet::new();
    features
        .iter()
        .filter(|f| match prop_str(f, "h3_address") {
            Some(id) => seen.insert(id),
            None => true,
        })
        .collect()
}

fn sum_of(features: &[&Value], key: &str) -> f64 {
    features.iter().filter_map(|f| prop_f64(f, key)).sum()
}

fn mean(vals: &[f64]) -> f64 {
    if vals.is_empty() {
        0.0
    } else {
        vals.iter().sum::<f64>() / vals.len() as f64
    }
}

fn collect(features: &[Value], key: &str) -> Vec<f64> {
    features.iter().filter_map(|f| prop_f64(f, key)).collect()
}

pub fn compute(
    features: &[Value],
    resolved: &GazetteerEntry,
    hazard: Hazard,
    lang: Lang,
) -> Stats {
    let total = features.len();

    // Flood is the one hazard whose features are re-observations of the same
    // ground, so its breakdown is built from distinct cells and ordered by area.
    let extent_based = hazard == Hazard::Flood;
    let rows: Vec<&Value> = if extent_based {
        distinct_cells(features)
    } else {
        features.iter().collect()
    };

    let (sub_level, sub_level_label, breakdown, sub_areas_affected) =
        match sub_level_of(resolved.level, lang) {
            Some((level, key, label)) => {
                let mut counts: HashMap<String, usize> = HashMap::new();
                let mut areas: HashMap<String, f64> = HashMap::new();
                for f in &rows {
                    let Some(name) = prop_str(f, key) else { continue };
                    *counts.entry(name.clone()).or_insert(0) += 1;
                    if extent_based {
                        *areas.entry(name).or_insert(0.0) += prop_f64(f, "f_area").unwrap_or(0.0);
                    }
                }
                let affected = counts.len();
                (
                    Some(level),
                    Some(label.to_string()),
                    rank_with_area(counts, &areas, rows.len(), lang),
                    affected,
                )
            }
            None => (None, None, Vec::new(), 0),
        };

    let mut categories = Vec::new();
    let mut timeline = Vec::new();
    let mut metrics = Vec::new();

    match hazard {
        Hazard::Fire => {
            if let Some(c) = category(features, "lu_name", lang.pick("พื้นที่รับผิดชอบ", "Responsible area"), total, lang) {
                categories.push(c);
            }
            if let Some(c) = category(
                features,
                "lu_hp_name",
                lang.pick("การใช้ประโยชน์ที่ดิน", "Land use"),
                total,
                lang,
            ) {
                categories.push(c);
            }

            // Dates arrive as "2026-07-18T00:00:00"; only the day matters here.
            let mut by_day: HashMap<String, usize> = HashMap::new();
            for f in features {
                if let Some(d) = prop_str(f, "acq_date") {
                    *by_day.entry(d.chars().take(10).collect()).or_insert(0) += 1;
                }
            }
            if !by_day.is_empty() {
                let mut days: Vec<(String, usize)> = by_day.into_iter().collect();
                days.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                if let Some((day, count)) = days.first() {
                    metrics.push(Metric {
                        label: lang.pick("วันที่พบมากที่สุด", "Peak day").into(),
                        value: match lang {
                            Lang::Th => format!("{day} ({count} จุด)"),
                            Lang::En => format!("{day} ({count} points)"),
                        },
                    });
                }
                metrics.push(Metric {
                    label: lang.pick("จำนวนวันที่พบ", "Days with data").into(),
                    value: match lang {
                        Lang::Th => format!("{} วัน", days.len()),
                        Lang::En => format!("{} days", days.len()),
                    },
                });
                // Chronological for the chart, unlike the ranked tables.
                days.sort_by(|a, b| a.0.cmp(&b.0));
                timeline = days
                    .into_iter()
                    .map(|(name, count)| Bucket {
                        name,
                        count,
                        area_km2: None,
                        share: 0.0,
                        other: false,
                    })
                    .collect();
            }

            let frp = collect(features, "frp");
            if !frp.is_empty() {
                let max = frp.iter().cloned().fold(f64::MIN, f64::max);
                metrics.push(Metric {
                    label: lang.pick("ความแรงเฉลี่ย (FRP)", "Mean intensity (FRP)").into(),
                    value: format!("{:.1} MW", mean(&frp)),
                });
                metrics.push(Metric {
                    label: lang.pick("ความแรงสูงสุด", "Peak intensity").into(),
                    value: format!("{max:.1} MW"),
                });
            }
            let conf = collect(features, "confidence");
            if !conf.is_empty() {
                metrics.push(Metric {
                    label: lang.pick("ความเชื่อมั่นเฉลี่ย", "Mean confidence").into(),
                    value: format!("{:.0}%", mean(&conf)),
                });
            }
        }
        Hazard::Drought => {
            let mut counts: HashMap<String, usize> = HashMap::new();
            for f in features {
                if let Some(v) = prop_str(f, "range_label") {
                    *counts.entry(range_label(&v, lang)).or_insert(0) += 1;
                }
            }
            if !counts.is_empty() {
                categories.push(Category {
                    title: lang.pick("ระดับความรุนแรง", "Severity").into(),
                    buckets: rank(counts, total, lang),
                });
            }

            // Each product is on its own scale, so they are reported separately
            // rather than averaged into one meaningless number.
            let dri = collect(features, "dri_mean");
            if !dri.is_empty() {
                metrics.push(Metric {
                    label: lang.pick("ค่า DRI เฉลี่ย", "Mean DRI").into(),
                    value: format!("{:.1}", mean(&dri)),
                });
            }
            let smap = collect(features, "smap_mean");
            if !smap.is_empty() {
                let min = smap.iter().cloned().fold(f64::MAX, f64::min);
                metrics.push(Metric {
                    label: lang.pick("ความชื้นดินเฉลี่ย", "Mean soil moisture").into(),
                    value: format!("{:.1}%", mean(&smap) * 100.0),
                });
                metrics.push(Metric {
                    label: lang.pick("ความชื้นดินต่ำสุด", "Lowest soil moisture").into(),
                    value: format!("{:.1}%", min * 100.0),
                });
            }
            let ndwi = collect(features, "ndwi_mean");
            if !ndwi.is_empty() {
                metrics.push(Metric {
                    label: lang.pick("ค่า NDWI เฉลี่ย", "Mean NDWI").into(),
                    value: format!("{:.3}", mean(&ndwi)),
                });
            }
        }
        Hazard::Flood => {
            // Everything below is computed over distinct cells. Without that,
            // a month with three satellite passes reports three times the
            // flooded ground and three times the affected population.
            let cells = distinct_cells(features);

            let area_km2 = sum_of(&cells, "f_area") / 1_000_000.0;
            if area_km2 > 0.0 {
                metrics.push(Metric {
                    label: lang.pick("พื้นที่น้ำท่วม", "Flooded area").into(),
                    value: match lang {
                        Lang::Th => format!("{:.1} ตร.กม.", area_km2),
                        Lang::En => format!("{area_km2:.1} km²"),
                    },
                });
            }

            metrics.push(Metric {
                label: lang.pick("พื้นที่สำรวจ (ไม่นับซ้ำ)", "Distinct cells surveyed").into(),
                value: match lang {
                    Lang::Th => format!("{} เซลล์ จาก {} รายการ", cells.len(), features.len()),
                    Lang::En => format!("{} of {} records", cells.len(), features.len()),
                },
            });

            // Impact attributes ride along on the same cells. Zeros are left
            // out entirely — "โรงพยาบาล 0" is noise, and its absence already
            // says none were affected.
            let population = sum_of(&cells, "population");
            if population > 0.0 {
                metrics.push(Metric {
                    label: lang.pick("ประชากรที่ได้รับผลกระทบ", "People affected").into(),
                    value: match lang {
                        Lang::Th => format!("{:.0} คน", population),
                        Lang::En => format!("{population:.0}"),
                    },
                });
            }
            for (key, th, en) in [
                ("building", "สิ่งปลูกสร้าง", "Buildings"),
                ("hospital", "โรงพยาบาล", "Hospitals"),
                ("school", "โรงเรียน", "Schools"),
            ] {
                let n = sum_of(&cells, key);
                if n > 0.0 {
                    metrics.push(Metric {
                        label: lang.pick(th, en).into(),
                        value: match lang {
                            Lang::Th => format!("{n:.0} แห่ง"),
                            Lang::En => format!("{n:.0}"),
                        },
                    });
                }
            }
            let road_km = sum_of(&cells, "length_road") / 1000.0;
            if road_km > 0.0 {
                metrics.push(Metric {
                    label: lang.pick("ถนนที่ได้รับผลกระทบ", "Roads affected").into(),
                    value: match lang {
                        Lang::Th => format!("{road_km:.1} กม."),
                        Lang::En => format!("{road_km:.1} km"),
                    },
                });
            }

            // Crop damage, reported per crop and only where non-zero.
            let mut crops: HashMap<String, usize> = HashMap::new();
            let mut crop_area: HashMap<String, f64> = HashMap::new();
            for (key, th, en) in [
                ("rice_area", "นาข้าว", "Paddy"),
                ("maize_area", "ข้าวโพด", "Maize"),
                ("cassava_area", "มันสำปะหลัง", "Cassava"),
                ("sugarcane_area", "อ้อย", "Sugarcane"),
            ] {
                let m2 = sum_of(&cells, key);
                if m2 > 0.0 {
                    let name = lang.pick(th, en).to_string();
                    crops.insert(name.clone(), cells.iter().filter(|f| prop_f64(f, key).is_some_and(|v| v > 0.0)).count());
                    crop_area.insert(name, m2);
                }
            }
            if !crops.is_empty() {
                categories.push(Category {
                    title: lang.pick("พื้นที่เกษตรที่ถูกน้ำท่วม", "Farmland flooded").into(),
                    buckets: rank_with_area(crops, &crop_area, total, lang),
                });
            }
        }
    }

    Stats {
        total,
        sub_level,
        sub_level_label,
        breakdown,
        sub_areas_affected,
        categories,
        timeline,
        metrics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn area(area_idn: u32, level: AdminLevel) -> GazetteerEntry {
        GazetteerEntry {
            area_idn,
            level,
            name_th: "ทดสอบ".into(),
            name_en: "Test".into(),
            parent_area_idn: None,
            bbox: None,
        }
    }

    fn hotspot(amphoe: &str, day: &str, frp: f64, lu: &str) -> Value {
        json!({"properties": {
            "ap_tn": amphoe, "acq_date": format!("{day}T00:00:00"),
            "frp": frp, "lu_name": lu, "confidence": 80
        }})
    }

    #[test]
    fn province_query_breaks_down_by_amphoe() {
        let feats = vec![
            hotspot("เมือง", "2026-07-01", 10.0, "พื้นที่เกษตร"),
            hotspot("เมือง", "2026-07-01", 20.0, "พื้นที่เกษตร"),
            hotspot("ชุมแพ", "2026-07-02", 30.0, "ป่าอนุรักษ์"),
        ];
        let s = compute(&feats, &area(40, AdminLevel::Province), Hazard::Fire, Lang::Th);

        assert_eq!(s.total, 3);
        assert_eq!(s.sub_level_label.as_deref(), Some("อำเภอ"));
        assert_eq!(s.sub_areas_affected, 2);
        // Ranked by count, so the busiest amphoe leads.
        assert_eq!(s.breakdown[0].name, "เมือง");
        assert_eq!(s.breakdown[0].count, 2);
        assert!((s.breakdown[0].share - 66.7).abs() < 0.05);
    }

    #[test]
    fn amphoe_query_breaks_down_by_tambon() {
        let feats = vec![json!({"properties": {"tb_tn": "ในเมือง", "ap_tn": "เมือง"}})];
        let s = compute(&feats, &area(4011, AdminLevel::Amphoe), Hazard::Fire, Lang::Th);
        assert_eq!(s.sub_level_label.as_deref(), Some("ตำบล"));
        assert_eq!(s.breakdown[0].name, "ในเมือง");
    }

    #[test]
    fn tambon_query_has_no_breakdown() {
        // Tambon is the finest level the data carries — nothing to split into.
        let feats = vec![json!({"properties": {"tb_tn": "ในเมือง"}})];
        let s = compute(&feats, &area(401101, AdminLevel::Tambon), Hazard::Fire, Lang::Th);
        assert!(s.sub_level.is_none());
        assert!(s.breakdown.is_empty());
    }

    #[test]
    fn long_tails_collapse_into_one_row() {
        let feats: Vec<Value> = (0..TOP_N + 5)
            .map(|i| hotspot(&format!("อ{i}"), "2026-07-01", 1.0, "พื้นที่เกษตร"))
            .collect();
        let s = compute(&feats, &area(40, AdminLevel::Province), Hazard::Fire, Lang::Th);
        assert_eq!(s.breakdown.len(), TOP_N + 1);
        assert!(s.breakdown.last().unwrap().other);
        assert!(s.breakdown[..TOP_N].iter().all(|b| !b.other));
        assert_eq!(s.breakdown.last().unwrap().count, 5);
        // Nothing may be lost in the fold.
        let summed: usize = s.breakdown.iter().map(|b| b.count).sum();
        assert_eq!(summed, s.total);
    }

    #[test]
    fn timeline_is_chronological_and_metrics_are_real() {
        let feats = vec![
            hotspot("ก", "2026-07-03", 10.0, "พื้นที่เกษตร"),
            hotspot("ก", "2026-07-01", 20.0, "พื้นที่เกษตร"),
            hotspot("ก", "2026-07-01", 30.0, "พื้นที่เกษตร"),
        ];
        let s = compute(&feats, &area(40, AdminLevel::Province), Hazard::Fire, Lang::Th);

        let days: Vec<&str> = s.timeline.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(days, vec!["2026-07-01", "2026-07-03"]);
        assert_eq!(s.timeline[0].count, 2);

        let m = |label: &str| {
            s.metrics
                .iter()
                .find(|m| m.label == label)
                .map(|m| m.value.clone())
        };
        assert_eq!(m("วันที่พบมากที่สุด").as_deref(), Some("2026-07-01 (2 จุด)"));
        assert_eq!(m("ความแรงเฉลี่ย (FRP)").as_deref(), Some("20.0 MW"));
        assert_eq!(m("ความแรงสูงสุด").as_deref(), Some("30.0 MW"));
    }

    #[test]
    fn drought_labels_are_translated_and_smap_scaled_to_percent() {
        let feats = vec![
            json!({"properties": {"ap_tn": "ก", "smap_mean": 0.25, "range_label": "medium_moisture"}}),
            json!({"properties": {"ap_tn": "ก", "smap_mean": 0.15, "range_label": "low_moisture"}}),
        ];
        let s = compute(&feats, &area(50, AdminLevel::Province), Hazard::Drought, Lang::Th);

        let names: Vec<&str> = s.categories[0]
            .buckets
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert!(names.contains(&"ชื้นปานกลาง"));
        assert!(names.contains(&"ชื้นน้อย"));

        let mean_pct = s
            .metrics
            .iter()
            .find(|m| m.label == "ความชื้นดินเฉลี่ย")
            .unwrap();
        assert_eq!(mean_pct.value, "20.0%");
    }

    fn flood_cell(h3: &str, amphoe: &str, f_area: f64, population: f64) -> Value {
        json!({"properties": {
            "h3_address": h3, "ap_tn": amphoe, "f_area": f_area, "population": population
        }})
    }

    fn metric(s: &Stats, label: &str) -> Option<String> {
        s.metrics.iter().find(|m| m.label == label).map(|m| m.value.clone())
    }

    #[test]
    fn flood_area_counts_each_cell_once_across_passes() {
        // The same two cells seen on three satellite passes. Area and
        // population must reflect the ground, not the number of observations.
        let mut feats = Vec::new();
        for _pass in 0..3 {
            feats.push(flood_cell("a", "เสนา", 2_000_000.0, 100.0));
            feats.push(flood_cell("b", "เสนา", 1_000_000.0, 50.0));
        }
        let s = compute(&feats, &area(14, AdminLevel::Province), Hazard::Flood, Lang::Th);

        assert_eq!(metric(&s, "พื้นที่น้ำท่วม").as_deref(), Some("3.0 ตร.กม."));
        assert_eq!(metric(&s, "ประชากรที่ได้รับผลกระทบ").as_deref(), Some("150 คน"));
        assert_eq!(
            metric(&s, "พื้นที่สำรวจ (ไม่นับซ้ำ)").as_deref(),
            Some("2 เซลล์ จาก 6 รายการ")
        );
        // The breakdown counts cells, not observations.
        assert_eq!(s.breakdown[0].count, 2);
        assert_eq!(s.breakdown[0].area_km2, Some(3.0));
    }

    #[test]
    fn flood_breakdown_ranks_by_area_not_polygon_count() {
        // One big cell beats three small ones: extent is what a flood means.
        let feats = vec![
            flood_cell("a", "ใหญ่", 9_000_000.0, 0.0),
            flood_cell("b", "เล็ก", 1_000_000.0, 0.0),
            flood_cell("c", "เล็ก", 1_000_000.0, 0.0),
            flood_cell("d", "เล็ก", 1_000_000.0, 0.0),
        ];
        let s = compute(&feats, &area(14, AdminLevel::Province), Hazard::Flood, Lang::Th);
        assert_eq!(s.breakdown[0].name, "ใหญ่");
        assert_eq!(s.breakdown[0].area_km2, Some(9.0));
        assert_eq!(s.breakdown[1].name, "เล็ก");
        assert_eq!(s.breakdown[1].count, 3);
    }

    #[test]
    fn flood_omits_impacts_that_are_zero() {
        let feats = vec![flood_cell("a", "เสนา", 1_000_000.0, 0.0)];
        let s = compute(&feats, &area(14, AdminLevel::Province), Hazard::Flood, Lang::Th);
        // No population, so no row claiming zero people were affected.
        assert!(metric(&s, "ประชากรที่ได้รับผลกระทบ").is_none());
        assert!(metric(&s, "โรงพยาบาล").is_none());
        assert_eq!(metric(&s, "พื้นที่น้ำท่วม").as_deref(), Some("1.0 ตร.กม."));
    }

    #[test]
    fn cells_without_an_h3_id_are_all_kept() {
        // Nothing to de-duplicate on: dropping them would lose real area.
        let feats = vec![
            json!({"properties": {"ap_tn": "ก", "f_area": 1_000_000.0}}),
            json!({"properties": {"ap_tn": "ก", "f_area": 1_000_000.0}}),
        ];
        let s = compute(&feats, &area(14, AdminLevel::Province), Hazard::Flood, Lang::Th);
        assert_eq!(metric(&s, "พื้นที่น้ำท่วม").as_deref(), Some("2.0 ตร.กม."));
    }

    #[test]
    fn empty_result_is_safe() {
        let s = compute(&[], &area(40, AdminLevel::Province), Hazard::Fire, Lang::Th);
        assert_eq!(s.total, 0);
        assert_eq!(s.sub_areas_affected, 0);
        assert!(s.breakdown.is_empty());
        assert!(s.metrics.is_empty());
    }
}
