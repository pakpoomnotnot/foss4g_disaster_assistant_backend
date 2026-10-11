use std::collections::HashMap;

use once_cell::sync::Lazy;

/// Colloquial place names mapped to the `area_idn` a user actually means.
/// Consulted BEFORE exact matching (see `lookup`), so an entry here overrides
/// the gazetteer — keep it short and only for names the gazetteer gets wrong
/// on its own. See README §5.2.
///
/// Most Thai short names need no entry: substring matching already resolves
/// "อยุธยา" inside "พระนครศรีอยุธยา", "กรุงเทพ" inside "กรุงเทพมหานคร", and
/// so on. Only add a name when plain matching lands somewhere wrong.
static ALIASES: Lazy<HashMap<&'static str, u32>> = Lazy::new(|| {
    HashMap::from([
        // A tambon is literally named "โคราช" (301803, ~5 km across). Without
        // this override both scripts resolve to it instead of the province.
        ("korat", 30), // Nakhon Ratchasima province
        ("โคราช", 30),
        // "Hat Yai" means the amphoe, not the whole of Songkhla. Pinning it
        // also breaks the tie with tambon "หาดยาย" (860413, Chumphon), whose
        // name_en romanises to the identical "HAT YAI".
        ("hat yai", 9011),
        ("หาดใหญ่", 9011),
    ])
});

pub fn resolve_alias(text: &str) -> Option<u32> {
    let key = text.trim().to_lowercase();
    ALIASES.get(key.as_str()).copied()
}
