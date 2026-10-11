use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Hazard {
    Flood,
    Fire,
    Drought,
}

impl Hazard {
    pub fn as_str(&self) -> &'static str {
        match self {
            Hazard::Flood => "flood",
            Hazard::Fire => "fire",
            Hazard::Drought => "drought",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Low,
}

/// The `drought` STAC endpoint serves three unrelated products under one name:
/// a drought-risk index (DRI+), soil moisture (SMAP) and a vegetation water
/// index (NDWI). They are measured on incompatible scales — 0-100 risk, 0-1
/// wetness, -1..1 index — so a question naming one has to fetch only that one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DroughtProduct {
    DriPlus,
    Smap,
    Ndwi,
}

impl DroughtProduct {
    /// STAC item ids are `<product>_<YYYYMMDD>`, e.g. `dri_plus_20260705`.
    pub fn item_prefix(&self) -> &'static str {
        match self {
            DroughtProduct::DriPlus => "dri_plus_",
            DroughtProduct::Smap => "smap_",
            DroughtProduct::Ndwi => "ndwi_",
        }
    }
}

/// Accepts a hazard the system does not support instead of failing the whole
/// response. Asked about an earthquake, the model dutifully answers
/// `"hazard": "earthquake"`, which used to abort deserialization and surface as
/// "failed to parse LLM response" — a crash where the honest answer is "this
/// system only covers flood, fire and drought".
fn lenient_hazard<'de, D>(d: D) -> Result<Option<Hazard>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(d)?;
    Ok(raw.and_then(|s| match s.trim().to_lowercase().as_str() {
        "flood" => Some(Hazard::Flood),
        "fire" => Some(Hazard::Fire),
        "drought" => Some(Hazard::Drought),
        _ => None,
    }))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExtractedQuery {
    /// `None` when the model named a hazard outside the supported three.
    #[serde(default, deserialize_with = "lenient_hazard")]
    pub hazard: Option<Hazard>,
    pub location_text: String,
    /// Relative window, used when the user said "last N days" or nothing at all.
    pub time_window_days: u32,
    /// Absolute range as `YYYY-MM-DD`, set only when the user named actual
    /// dates ("1-15 July", "เดือนกรกฎาคม"). Takes precedence over
    /// `time_window_days`, which a relative-only phrasing cannot express.
    #[serde(default)]
    pub date_from: Option<String>,
    #[serde(default)]
    pub date_to: Option<String>,
    /// Which drought product the question asked for. `None` means the user did
    /// not name one, so every product is returned and the client separates them.
    #[serde(default)]
    pub dataset: Option<DroughtProduct>,
    #[allow(dead_code)]
    pub confidence: Confidence,
    pub needs_clarification: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminLevel {
    Province,
    Amphoe,
    Tambon,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GazetteerEntry {
    pub area_idn: u32,
    pub level: AdminLevel,
    pub name_th: String,
    pub name_en: String,
    pub parent_area_idn: Option<u32>,
    pub bbox: Option<[f64; 4]>,
}

#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    pub text: String,
    /// Set when the user picked one of the disambiguation chips. The client
    /// already knows exactly which area was chosen, so it says so instead of
    /// re-encoding the choice into the sentence and hoping the extractor and
    /// the gazetteer reconstruct it — that round trip could not work at all
    /// for a Thai question, where the English name it tried to substitute
    /// never appears in the text.
    #[serde(default)]
    pub area_idn: Option<u32>,
}
