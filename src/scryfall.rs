//! Scryfall bulk-data ingest.
//!
//! Scryfall asks that mass lookups go through their daily bulk dumps rather than
//! per-card API calls, so this downloads `oracle_cards` once (~24 MB gzipped)
//! and distils it into a compact on-disk index. Image URLs are kept as CDN links
//! — the browser fetches and caches the art itself, so nothing large is stored.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};

use crate::names;

const USER_AGENT: &str = concat!("BulkupMTG/", env!("CARGO_PKG_VERSION"), " (bulk collection matcher)");

/// Bump whenever `names::key` changes or a new field is distilled from the dump.
///
/// The index is keyed by normalised name, so a change to normalisation leaves
/// every cached key subtly wrong — lookups miss instead of failing loudly. The
/// version forces a rebuild rather than letting that happen quietly.
pub const INDEX_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardInfo {
    pub name: String,
    pub color_identity: Vec<String>,
    pub type_line: String,
    pub cmc: f64,
    pub image_small: Option<String>,
    pub image_normal: Option<String>,
    pub scryfall_uri: String,
    pub is_commander: bool,
    /// Basic lands are excluded from every score — owning Forests means nothing.
    pub is_basic: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ScryfallIndex {
    #[serde(default)]
    pub version: u32,
    pub updated_at: String,
    pub source_updated_at: String,
    /// `names::key` -> card. Contains front-face aliases too.
    pub cards: HashMap<String, CardInfo>,
}

impl ScryfallIndex {
    pub fn get(&self, key: &str) -> Option<&CardInfo> {
        self.cards.get(key)
    }

    pub fn lookup(&self, name: &str) -> Option<&CardInfo> {
        self.get(&names::key(name))
            .or_else(|| self.get(&names::front_key(name)))
    }

    /// Every commander-legal commander, de-duplicated by name.
    pub fn commanders(&self) -> Vec<&CardInfo> {
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<&CardInfo> = self
            .cards
            .values()
            .filter(|c| c.is_commander && seen.insert(c.name.as_str()))
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

// --- raw Scryfall shapes (only the fields we consume) ---

#[derive(Deserialize)]
struct BulkList {
    data: Vec<BulkEntry>,
}

#[derive(Deserialize)]
struct BulkEntry {
    #[serde(rename = "type")]
    kind: String,
    updated_at: String,
    download_uri: Option<String>,
    jsonl_download_uri: Option<String>,
}

#[derive(Deserialize)]
struct RawCard {
    name: String,
    #[serde(default)]
    color_identity: Vec<String>,
    #[serde(default)]
    type_line: String,
    #[serde(default)]
    cmc: f64,
    #[serde(default)]
    oracle_text: String,
    #[serde(default)]
    scryfall_uri: String,
    #[serde(default)]
    image_uris: Option<ImageUris>,
    #[serde(default)]
    card_faces: Option<Vec<RawFace>>,
    #[serde(default)]
    legalities: HashMap<String, String>,
}

#[derive(Deserialize)]
struct RawFace {
    #[serde(default)]
    type_line: String,
    #[serde(default)]
    oracle_text: String,
    #[serde(default)]
    image_uris: Option<ImageUris>,
}

#[derive(Deserialize, Clone)]
struct ImageUris {
    small: Option<String>,
    normal: Option<String>,
}

/// Legendary creatures, plus anything that says so in its own rules text
/// (Grist, backgrounds-adjacent designs, planeswalker commanders).
fn detect_commander(raw: &RawCard) -> bool {
    if raw.legalities.get("commander").map(String::as_str) != Some("legal") {
        return false;
    }
    let front_type = raw
        .card_faces
        .as_ref()
        .and_then(|f| f.first())
        .map(|f| f.type_line.as_str())
        .filter(|t| !t.is_empty())
        .unwrap_or(&raw.type_line);

    if front_type.contains("Legendary") && front_type.contains("Creature") {
        return true;
    }
    let text_says = raw.oracle_text.contains("can be your commander")
        || raw
            .card_faces
            .as_ref()
            .is_some_and(|fs| fs.iter().any(|f| f.oracle_text.contains("can be your commander")));
    text_says
}

fn images(raw: &RawCard) -> (Option<String>, Option<String>) {
    let uris = raw.image_uris.clone().or_else(|| {
        raw.card_faces
            .as_ref()
            .and_then(|f| f.first())
            .and_then(|f| f.image_uris.clone())
    });
    match uris {
        Some(u) => (u.small, u.normal),
        None => (None, None),
    }
}

pub fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .context("building HTTP client")
}

/// The `updated_at` Scryfall reports for the oracle dump, used to skip
/// re-downloading an index that is already current.
pub async fn remote_version(client: &reqwest::Client) -> Result<String> {
    Ok(fetch_entry(client).await?.updated_at)
}

async fn fetch_entry(client: &reqwest::Client) -> Result<BulkEntry> {
    let list: BulkList = client
        .get("https://api.scryfall.com/bulk-data")
        .send()
        .await
        .context("listing Scryfall bulk data")?
        .error_for_status()?
        .json()
        .await
        .context("parsing bulk data listing")?;
    list.data
        .into_iter()
        .find(|e| e.kind == "oracle_cards")
        .context("Scryfall bulk listing had no oracle_cards entry")
}

/// Download and distil the oracle dump. Handles both the plain-JSON and the
/// gzipped-JSONL forms, since Scryfall has served each at different times.
pub async fn download(client: &reqwest::Client) -> Result<ScryfallIndex> {
    let entry = fetch_entry(client).await?;
    let (uri, jsonl) = match (&entry.download_uri, &entry.jsonl_download_uri) {
        (Some(u), _) => (u.clone(), false),
        (None, Some(u)) => (u.clone(), true),
        _ => anyhow::bail!("Scryfall oracle_cards entry had no download URI"),
    };

    let bytes = client
        .get(&uri)
        .send()
        .await
        .context("downloading Scryfall bulk file")?
        .error_for_status()?
        .bytes()
        .await
        .context("reading Scryfall bulk file")?;

    let gzipped = uri.ends_with(".gz") || bytes.starts_with(&[0x1f, 0x8b]);
    let raws: Vec<RawCard> = if jsonl || gzipped {
        let reader: Box<dyn BufRead + '_> = if gzipped {
            Box::new(BufReader::new(GzDecoder::new(&bytes[..])))
        } else {
            Box::new(BufReader::new(&bytes[..]))
        };
        parse_stream(reader)?
    } else {
        serde_json::from_slice(&bytes).context("parsing Scryfall bulk JSON")?
    };

    let mut index = ScryfallIndex {
        version: INDEX_VERSION,
        updated_at: chrono::Utc::now().to_rfc3339(),
        source_updated_at: entry.updated_at,
        cards: HashMap::with_capacity(raws.len() * 2),
    };

    for raw in raws {
        let (image_small, image_normal) = images(&raw);
        let info = CardInfo {
            is_commander: detect_commander(&raw),
            is_basic: raw.type_line.contains("Basic Land"),
            color_identity: raw.color_identity.clone(),
            type_line: raw.type_line.clone(),
            cmc: raw.cmc,
            scryfall_uri: raw.scryfall_uri.clone(),
            image_small,
            image_normal,
            name: raw.name.clone(),
        };
        let k = names::key(&raw.name);
        let fk = names::front_key(&raw.name);
        if fk != k && !fk.is_empty() {
            index.cards.entry(fk).or_insert_with(|| info.clone());
        }
        index.cards.insert(k, info);
    }

    Ok(index)
}

/// The dump may be JSONL (one card per line) or a single JSON array; sniff it
/// from the first non-empty character rather than trusting the file extension.
fn parse_stream(mut reader: Box<dyn BufRead + '_>) -> Result<Vec<RawCard>> {
    let mut first = String::new();
    loop {
        first.clear();
        if reader.read_line(&mut first)? == 0 {
            return Ok(Vec::new());
        }
        if !first.trim().is_empty() {
            break;
        }
    }

    if first.trim_start().starts_with('[') {
        let mut rest = first.into_bytes();
        reader.read_to_end(&mut rest)?;
        return serde_json::from_slice(&rest).context("parsing Scryfall bulk JSON array");
    }

    let mut out = Vec::with_capacity(35_000);
    let mut push = |line: &str| {
        let line = line.trim().trim_end_matches(',');
        if line.is_empty() || line == "[" || line == "]" {
            return;
        }
        // A card that fails to deserialise is skipped rather than aborting the
        // whole import; the index stays usable.
        if let Ok(c) = serde_json::from_str::<RawCard>(line) {
            out.push(c);
        }
    };
    push(&first);
    for line in reader.lines() {
        push(&line?);
    }
    Ok(out)
}

use std::io::Read;

/// Load the cached index, discarding one built by an incompatible version so it
/// is re-downloaded rather than silently mis-matching every lookup.
pub fn load(path: &Path) -> Result<ScryfallIndex> {
    let f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let index: ScryfallIndex =
        serde_json::from_reader(BufReader::new(f)).context("parsing cached Scryfall index")?;
    if index.version != INDEX_VERSION {
        return Ok(ScryfallIndex::default());
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_index_version_is_discarded() {
        let dir = std::env::temp_dir().join(format!("bulkup-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.json");

        let mut old = ScryfallIndex {
            version: INDEX_VERSION - 1,
            source_updated_at: "2020-01-01".into(),
            ..Default::default()
        };
        old.cards.insert("stale key".into(), CardInfo {
            name: "Stale".into(), color_identity: vec![], type_line: String::new(), cmc: 0.0,
            image_small: None, image_normal: None, scryfall_uri: String::new(),
            is_commander: false, is_basic: false,
        });
        save(&path, &old).unwrap();

        // An index written under different name-normalisation rules must not be
        // trusted; loading it yields an empty index so the caller re-downloads.
        let loaded = load(&path).unwrap();
        assert!(loaded.cards.is_empty());
        assert_eq!(loaded.source_updated_at, "");

        let current = ScryfallIndex { version: INDEX_VERSION, ..old };
        save(&path, &current).unwrap();
        assert_eq!(load(&path).unwrap().cards.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }
}

pub fn save(path: &Path, index: &ScryfallIndex) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let f = std::fs::File::create(&tmp)?;
    serde_json::to_writer(std::io::BufWriter::new(f), index)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
