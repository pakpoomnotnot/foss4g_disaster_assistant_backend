//! Which language the answer is written in.
//!
//! Decided from the user's own text rather than asked of the LLM: it is a
//! one-line check that cannot fail or time out, and a wrong guess here would
//! mean the whole reply comes back in the wrong language.

use serde::Serialize;

use crate::models::Hazard;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Th,
    En,
}

impl Lang {
    /// Any Thai character means a Thai answer. A Thai speaker who writes
    /// "flooding ที่อยุธยา" is writing Thai with a loanword and expects Thai
    /// back; only a question with no Thai at all is treated as English.
    pub fn detect(text: &str) -> Lang {
        if text.chars().any(|c| ('\u{0E00}'..='\u{0E7F}').contains(&c)) {
            Lang::Th
        } else {
            Lang::En
        }
    }

    pub fn pick<'a>(&self, th: &'a str, en: &'a str) -> &'a str {
        match self {
            Lang::Th => th,
            Lang::En => en,
        }
    }
}

pub fn hazard_label(hazard: Hazard, lang: Lang) -> &'static str {
    match (hazard, lang) {
        (Hazard::Flood, Lang::Th) => "น้ำท่วม",
        (Hazard::Flood, Lang::En) => "flood",
        (Hazard::Fire, Lang::Th) => "จุดความร้อน",
        (Hazard::Fire, Lang::En) => "hotspot",
        (Hazard::Drought, Lang::Th) => "ภัยแล้ง",
        (Hazard::Drought, Lang::En) => "drought",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_english_is_english() {
        assert_eq!(Lang::detect("flooding in Ayutthaya this week"), Lang::En);
        assert_eq!(Lang::detect("NDWI Chiang Rai"), Lang::En);
    }

    #[test]
    fn any_thai_makes_it_thai() {
        assert_eq!(Lang::detect("น้ำท่วมอยุธยา"), Lang::Th);
        // Mixed script: still a Thai question.
        assert_eq!(Lang::detect("flooding ที่อยุธยา"), Lang::Th);
    }

    #[test]
    fn digits_and_symbols_do_not_decide() {
        assert_eq!(Lang::detect("hotspots 2025-01-01"), Lang::En);
        assert_eq!(Lang::detect("จุดความร้อน 30 วัน"), Lang::Th);
    }
}
