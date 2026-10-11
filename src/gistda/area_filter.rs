//! Post-filter for features returned by the Vallaris hop.
//!
//! A bbox is a rectangle, and a province is not. Khon Kaen's bbox spills into
//! Chaiyaphum, Roi Et and Maha Sarakham, so a bbox-only query genuinely returns
//! hotspots outside the province the user asked about (measured: the very first
//! hotspot returned for "ขอนแก่น" carried `pv_tn: "ชัยภูมิ"`). GISTDA stamps
//! every feature with the admin area it fell in, so we can drop those here.
//!
//! Deliberately lenient: a feature is only rejected when it *tells us* it
//! belongs somewhere else. Missing or unrecognised admin attributes mean the
//! feature is kept — the bbox already vouched for it, and silently deleting
//! data we cannot judge is worse than showing a few extra points.

use serde_json::{Map, Value};

use crate::models::{AdminLevel, GazetteerEntry};

/// Candidate property names per admin level, in the shapes actually observed:
/// fire hotspots use `pv_idn`/`pv_tn`/`changwat`, drought h3 cells use the same
/// `*_idn`/`*_tn` pair but with the id encoded as a *string* ("10" not 10).
struct LevelFields {
    id: &'static [&'static str],
    name_th: &'static [&'static str],
    name_en: &'static [&'static str],
}

fn fields_for(level: AdminLevel) -> LevelFields {
    match level {
        AdminLevel::Province => LevelFields {
            id: &["pv_idn", "pv_code", "prov_idn"],
            name_th: &["pv_tn", "changwat", "province_t", "pv_th"],
            name_en: &["pv_en", "province_e"],
        },
        AdminLevel::Amphoe => LevelFields {
            id: &["ap_idn", "amp_idn"],
            name_th: &["ap_tn", "amphoe", "amphoe_t", "ap_th"],
            name_en: &["ap_en", "amphoe_e"],
        },
        AdminLevel::Tambon => LevelFields {
            id: &["tb_idn", "tam_idn"],
            name_th: &["tb_tn", "tambol", "tambon", "tambon_t", "tb_th"],
            name_en: &["tb_en", "tambon_e"],
        },
    }
}

/// Reads an admin id that may arrive as a JSON number (fire) or a JSON string
/// (drought). `pv_code` is excluded from this leniency elsewhere because a bare
/// "01" amphoe code is not a full area_idn — only the `*_idn` family is.
fn read_id(props: &Map<String, Value>, key: &str) -> Option<u32> {
    match props.get(key)? {
        Value::Number(n) => n.as_u64()?.try_into().ok(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn read_str<'a>(props: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    match props.get(key)? {
        Value::String(s) if !s.trim().is_empty() => Some(s),
        _ => None,
    }
}

/// Thai admin names arrive with and without the level prefix ("ขอนแก่น" vs
/// "จังหวัดขอนแก่น") and with stray spacing. Note "เมือง" is *not* stripped —
/// it is a real part of capital-amphoe names ("เมืองขอนแก่น") and removing it
/// would make every capital amphoe look alike.
fn normalize_th(s: &str) -> String {
    let mut out = s.trim();
    for prefix in ["จังหวัด", "อำเภอ", "ตำบล", "เขต", "แขวง", "จ.", "อ.", "ต."] {
        if let Some(rest) = out.strip_prefix(prefix) {
            out = rest.trim();
            break;
        }
    }
    out.chars().filter(|c| !c.is_whitespace()).collect()
}

fn normalize_en(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Loose on purpose (the user's "ไม่ต้อง match 100%"): equality, or either
/// string containing the other, so "Ayutthaya" still matches "Phra Nakhon Si
/// Ayutthaya". The 3-char floor stops a very short name from matching half the
/// country by accident.
fn names_match(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let shorter = a.len().min(b.len());
    shorter >= 3 && (a.contains(b) || b.contains(a))
}

/// `None` = the feature carries no attribute for this level, so we cannot
/// judge it. `Some(bool)` = it identified itself and this is the verdict.
fn verdict(props: &Map<String, Value>, area: &GazetteerEntry) -> Option<bool> {
    let f = fields_for(area.level);

    // The id is authoritative when present: it is the same area_idn scheme the
    // gazetteer is keyed by, so no string handling can beat it.
    for key in f.id {
        if key.ends_with("_idn") {
            if let Some(id) = read_id(props, key) {
                return Some(id == area.area_idn);
            }
        }
    }

    let want_th = normalize_th(&area.name_th);
    for key in f.name_th {
        if let Some(v) = read_str(props, key) {
            return Some(names_match(&normalize_th(v), &want_th));
        }
    }

    let want_en = normalize_en(&area.name_en);
    for key in f.name_en {
        if let Some(v) = read_str(props, key) {
            return Some(names_match(&normalize_en(v), &want_en));
        }
    }

    None
}

pub fn belongs_to_area(feature: &Value, area: &GazetteerEntry) -> bool {
    match feature.get("properties").and_then(Value::as_object) {
        Some(props) => verdict(props, area).unwrap_or(true),
        None => true,
    }
}

/// Keeps only features inside `area`, returning `(kept, dropped_count)`.
pub fn filter_to_area(features: Vec<Value>, area: &GazetteerEntry) -> (Vec<Value>, usize) {
    let before = features.len();
    let kept: Vec<Value> = features
        .into_iter()
        .filter(|f| belongs_to_area(f, area))
        .collect();
    let dropped = before - kept.len();
    (kept, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn province(area_idn: u32, th: &str, en: &str) -> GazetteerEntry {
        GazetteerEntry {
            area_idn,
            level: AdminLevel::Province,
            name_th: th.to_string(),
            name_en: en.to_string(),
            parent_area_idn: None,
            bbox: None,
        }
    }

    fn khon_kaen() -> GazetteerEntry {
        province(40, "ขอนแก่น", "Khon Kaen")
    }

    #[test]
    fn numeric_id_decides() {
        let inside = json!({"properties": {"pv_idn": 40, "pv_tn": "ขอนแก่น"}});
        let outside = json!({"properties": {"pv_idn": 36, "pv_tn": "ชัยภูมิ"}});
        assert!(belongs_to_area(&inside, &khon_kaen()));
        assert!(!belongs_to_area(&outside, &khon_kaen()));
    }

    #[test]
    fn string_id_decides_too() {
        // drought h3 cells encode the id as a string
        let bangkok = province(10, "กรุงเทพมหานคร", "Bangkok");
        assert!(belongs_to_area(
            &json!({"properties": {"pv_idn": "10"}}),
            &bangkok
        ));
        assert!(!belongs_to_area(
            &json!({"properties": {"pv_idn": "40"}}),
            &bangkok
        ));
    }

    #[test]
    fn falls_back_to_thai_name_with_prefix() {
        let f = json!({"properties": {"changwat": "จังหวัด ขอนแก่น"}});
        assert!(belongs_to_area(&f, &khon_kaen()));
    }

    #[test]
    fn partial_english_name_still_matches() {
        let ayutthaya = province(14, "พระนครศรีอยุธยา", "Phra Nakhon Si Ayutthaya");
        let f = json!({"properties": {"pv_en": "Ayutthaya"}});
        assert!(belongs_to_area(&f, &ayutthaya));
    }

    #[test]
    fn unjudgeable_features_are_kept() {
        assert!(belongs_to_area(&json!({"properties": {"frp": 7.4}}), &khon_kaen()));
        assert!(belongs_to_area(&json!({}), &khon_kaen()));
    }

    #[test]
    fn tambon_query_matches_on_tambon_field_only() {
        let na_siao = GazetteerEntry {
            area_idn: 360109,
            level: AdminLevel::Tambon,
            name_th: "นาเสียว".to_string(),
            name_en: "Na Siao".to_string(),
            parent_area_idn: Some(3601),
            bbox: None,
        };
        let f = json!({"properties": {"pv_idn": 36, "tb_idn": 360109}});
        let other = json!({"properties": {"pv_idn": 36, "tb_idn": 360110}});
        assert!(belongs_to_area(&f, &na_siao));
        assert!(!belongs_to_area(&other, &na_siao));
    }

    #[test]
    fn counts_what_it_drops() {
        let feats = vec![
            json!({"properties": {"pv_idn": 40}}),
            json!({"properties": {"pv_idn": 36}}),
            json!({"properties": {"pv_idn": 45}}),
        ];
        let (kept, dropped) = filter_to_area(feats, &khon_kaen());
        assert_eq!(kept.len(), 1);
        assert_eq!(dropped, 2);
    }
}
