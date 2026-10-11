use crate::models::{AdminLevel, GazetteerEntry};

use super::alias;

pub enum LookupResult {
    Found(GazetteerEntry),
    Ambiguous(Vec<GazetteerEntry>),
    NotFound,
}

/// True if `descendant` *is* `ancestor_id`, or is its child/grandchild — walked up the
/// province -> amphoe -> tambon chain (max 2 hops, matching our 3 admin levels).
fn is_same_or_descendant(entries: &[GazetteerEntry], ancestor_id: u32, descendant: &GazetteerEntry) -> bool {
    if descendant.area_idn == ancestor_id {
        return true;
    }
    let mut current = descendant;
    for _ in 0..2 {
        let parent_id = match current.parent_area_idn {
            Some(id) => id,
            None => return false,
        };
        if parent_id == ancestor_id {
            return true;
        }
        current = match entries.iter().find(|e| e.area_idn == parent_id) {
            Some(parent) => parent,
            None => return false,
        };
    }
    false
}

/// A capital amphoe commonly shares its province's exact name (e.g. "Phra Nakhon Si
/// Ayutthaya" province + amphoe) — matching both isn't real ambiguity, it's the same
/// real-world place asked at two admin levels. Collapse such same-name chains down to
/// their single broadest (root) entry; leave genuinely distinct places (e.g. "Bang Phli"
/// in two unrelated provinces) untouched.
fn collapse_same_chain(entries: &[GazetteerEntry], candidates: Vec<GazetteerEntry>) -> Vec<GazetteerEntry> {
    if candidates.len() <= 1 {
        return candidates;
    }
    let first_name = candidates[0].name_en.to_lowercase();
    if !candidates.iter().all(|c| c.name_en.to_lowercase() == first_name) {
        return candidates;
    }
    let roots: Vec<GazetteerEntry> = candidates
        .iter()
        .filter(|c| {
            !candidates
                .iter()
                .any(|other| other.area_idn != c.area_idn && is_same_or_descendant(entries, other.area_idn, c))
        })
        .cloned()
        .collect();
    if roots.len() == 1 {
        roots
    } else {
        candidates
    }
}

/// 18 of the 77 provinces share their exact name with some tambon or amphoe,
/// and for 10 of them that twin sits in a *different* province, so the
/// parent/child collapse above can never apply — "ขอนแก่น", "เชียงใหม่",
/// "อ่างทอง" and "สระแก้ว" were therefore permanently unanswerable, always
/// coming back as "pick one of these". A bare province name in a disaster
/// question means the province essentially every time, so when exactly one
/// candidate is province-level it wins.
///
/// Trade-off, accepted knowingly: the same-named tambon can no longer be
/// selected by that bare name (e.g. tambon ขอนแก่น in Roi Et). Genuine
/// ambiguity between peers is untouched — "Bang Phli" (amphoe vs tambon, no
/// province involved) still asks the user.
fn prefer_sole_province(candidates: Vec<GazetteerEntry>) -> Vec<GazetteerEntry> {
    if candidates.len() <= 1 {
        return candidates;
    }
    let mut provinces = candidates
        .iter()
        .filter(|c| c.level == AdminLevel::Province);
    match (provinces.next(), provinces.next()) {
        (Some(only), None) => vec![only.clone()],
        _ => candidates,
    }
}

/// Level words that may sit in front of a place name.
///
/// The extraction prompt is deliberately told to keep `location_text` exactly
/// as the user typed it and to leave normalisation to this module — so
/// "ไฟป่าที่อำเภอนางรอง" arrives here as "อำเภอนางรอง". Without stripping, that
/// matched nothing at all: the substring pass asks whether an entry's name
/// contains the query, and "นางรอง" does not contain "อำเภอนางรอง".
///
/// Verified safe across all 8,429 entries: not one begins with any of these.
const TH_PREFIXES: &[&str] = &[
    "จังหวัด", "อำเภอ", "ตำบล", "แขวง", "เขต", "จ.", "อ.", "ต.",
];

/// Same idea for romanised input. "Mueang"/"Muang" is deliberately absent —
/// 193 real names start with it ("Mueang Nonthaburi"), and stripping it would
/// make every capital amphoe collide.
const EN_PREFIXES: &[&str] = &[
    "amphoe ",
    "changwat ",
    "tambon ",
    "khwaeng ",
    "khet ",
    "district ",
    "province ",
];

fn strip_admin_prefix(query: &str) -> &str {
    let q = query.trim();
    for p in TH_PREFIXES {
        if let Some(rest) = q.strip_prefix(p) {
            return rest.trim_start();
        }
    }
    let lower = q.to_lowercase();
    for p in EN_PREFIXES {
        // These prefixes are ASCII, so a byte offset taken from the lowercased
        // copy is still a valid char boundary in the original.
        if lower.starts_with(p) {
            return q[p.len()..].trim_start();
        }
    }
    q
}

/// Implements README §5.1: exact match -> alias table -> substring match -> ambiguous/not-found.
/// Never guesses a bbox — callers must treat `Ambiguous`/`NotFound` as `needs_clarification`.
pub fn lookup(entries: &[GazetteerEntry], location_text: &str) -> LookupResult {
    let query = strip_admin_prefix(location_text);
    if query.is_empty() {
        return LookupResult::NotFound;
    }
    let query_lower = query.to_lowercase();

    // Aliases win over an exact name match on purpose. They are a short,
    // hand-curated statement of what a colloquial name actually means, and an
    // exact hit can be a coincidence: there is a real 5 km tambon literally
    // named "โคราช" (area_idn 301803), so exact-first sent every "โคราช" query
    // to that speck instead of the province the whole country means by it.
    if let Some(area_idn) = alias::resolve_alias(query) {
        if let Some(e) = entries.iter().find(|e| e.area_idn == area_idn) {
            return LookupResult::Found(e.clone());
        }
    }

    let exact: Vec<GazetteerEntry> = entries
        .iter()
        .filter(|e| e.name_en.to_lowercase() == query_lower || e.name_th == query)
        .cloned()
        .collect();
    let exact = prefer_sole_province(collapse_same_chain(entries, exact));
    match exact.len() {
        1 => return LookupResult::Found(exact.into_iter().next().unwrap()),
        n if n > 1 => return LookupResult::Ambiguous(exact),
        _ => {}
    }

    // Substring covers the very common case of a short colloquial name sitting
    // inside the official one. It has to run over BOTH scripts: "Ayutthaya" is
    // inside "Phra Nakhon Si Ayutthaya", and equally "อยุธยา" is inside
    // "พระนครศรีอยุธยา" — checking only name_en made every such Thai query fail.
    // Thai has no case, so the raw query is compared for name_th.
    let substring: Vec<GazetteerEntry> = entries
        .iter()
        .filter(|e| {
            e.name_en.to_lowercase().contains(&query_lower) || e.name_th.contains(query)
        })
        .cloned()
        .collect();
    let substring = prefer_sole_province(collapse_same_chain(entries, substring));
    match substring.len() {
        1 => LookupResult::Found(substring.into_iter().next().unwrap()),
        n if n > 1 => LookupResult::Ambiguous(substring),
        _ => LookupResult::NotFound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_thai_level_words() {
        assert_eq!(strip_admin_prefix("อำเภอนางรอง"), "นางรอง");
        assert_eq!(strip_admin_prefix("อำเภอกมลาไสย"), "กมลาไสย");
        assert_eq!(strip_admin_prefix("จังหวัดขอนแก่น"), "ขอนแก่น");
        assert_eq!(strip_admin_prefix("ต. สะเมิงใต้"), "สะเมิงใต้");
        assert_eq!(strip_admin_prefix("  อ.หาดใหญ่  "), "หาดใหญ่");
    }

    #[test]
    fn strips_romanised_level_words() {
        assert_eq!(strip_admin_prefix("Amphoe Nang Rong"), "Nang Rong");
        assert_eq!(strip_admin_prefix("AMPHOE SAMOENG"), "SAMOENG");
        assert_eq!(strip_admin_prefix("Changwat Chiang Mai"), "Chiang Mai");
    }

    #[test]
    fn leaves_mueang_alone() {
        // Stripping this would turn every capital amphoe into its province's name.
        assert_eq!(strip_admin_prefix("Mueang Nonthaburi"), "Mueang Nonthaburi");
        assert_eq!(strip_admin_prefix("เมืองขอนแก่น"), "เมืองขอนแก่น");
    }

    #[test]
    fn leaves_ordinary_names_untouched() {
        assert_eq!(strip_admin_prefix("นางรอง"), "นางรอง");
        assert_eq!(strip_admin_prefix("Nang Rong"), "Nang Rong");
        assert_eq!(strip_admin_prefix("โคราช"), "โคราช");
    }
}
