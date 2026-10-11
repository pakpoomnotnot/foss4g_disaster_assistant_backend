use anyhow::Context;

use crate::models::GazetteerEntry;

pub fn load(path: &str) -> anyhow::Result<Vec<GazetteerEntry>> {
    let data = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read gazetteer file at {path}"))?;
    let entries: Vec<GazetteerEntry> =
        serde_json::from_str(&data).context("failed to parse gazetteer JSON")?;
    Ok(entries)
}
