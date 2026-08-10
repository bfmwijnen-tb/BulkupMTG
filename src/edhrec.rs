//! EDHREC crawl and cache.
//!
//! EDHREC publishes no documented API, so this reads the same JSON its own front
//! end consumes. To stay a good citizen the crawler is rate-limited to a couple
//! of requests per second, identifies itself in the User-Agent, and caches every
//! commander to disk so a full crawl happens once rather than per session.
//!
//! Two endpoints per commander:
//!   * `pages/commanders/<slug>.json`     — ~260 ranked cards with synergy scores
//!   * `pages/average-decks/<slug>.json`  — a concrete 99-card list
//!
//! The synergy score is the important one. EDHREC defines it as *inclusion rate
//! in this commander's decks minus inclusion rate across the same colour
//! identity generally*, which is exactly what separates a real payoff from a
//! staple every deck in those colours runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdhCard {
    pub name: String,
    /// Inclusion rate here minus the colour-identity baseline. Staples sit near
    /// zero; signature cards run high. Can legitimately be negative.
    pub synergy: f64,
    pub num_decks: u32,
    pub potential_decks: u32,
}

impl EdhCard {
    /// Share of this commander's decks that run the card.
    pub fn inclusion(&self) -> f64 {
        if self.potential_decks == 0 {
            0.0
        } else {
            self.num_decks as f64 / self.potential_decks as f64
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommanderData {
    pub slug: String,
    pub name: String,
    /// De-duplicated, sorted by inclusion descending.
    pub cards: Vec<EdhCard>,
    /// EDHREC's average decklist, empty until the second crawl pass runs.
    pub avg_deck: Vec<String>,
    pub deck_count: u32,
    pub fetched_at: String,
}

/// Which commanders have been crawled, and which are known to be absent from
/// EDHREC so they are not retried on every update.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub fetched: HashMap<String, String>,
    pub missing: HashMap<String, String>,
}

// --- polite pacing ---

#[derive(Clone)]
pub struct RateLimiter {
    interval: Duration,
    last: Arc<Mutex<Option<Instant>>>,
}

impl RateLimiter {
    pub fn per_second(rps: f64) -> Self {
        Self {
            interval: Duration::from_secs_f64(1.0 / rps.max(0.1)),
            last: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn acquire(&self) {
        let mut guard = self.last.lock().await;
        if let Some(prev) = *guard {
            let elapsed = prev.elapsed();
            if elapsed < self.interval {
                tokio::time::sleep(self.interval - elapsed).await;
            }
        }
        *guard = Some(Instant::now());
    }
}

// --- raw shapes ---

#[derive(Deserialize)]
struct Page {
    container: Container,
}

#[derive(Deserialize)]
struct Container {
    json_dict: JsonDict,
}

#[derive(Deserialize)]
struct JsonDict {
    #[serde(default)]
    cardlists: Vec<CardList>,
}

#[derive(Deserialize)]
struct CardList {
    #[serde(default)]
    header: String,
    #[serde(default)]
    cardviews: Vec<CardView>,
}

#[derive(Deserialize)]
struct CardView {
    name: String,
    #[serde(default)]
    synergy: f64,
    #[serde(default)]
    num_decks: u32,
    #[serde(default)]
    potential_decks: u32,
}

#[derive(Deserialize)]
struct AvgDeckPage {
    deck: AvgDeck,
}

#[derive(Deserialize)]
struct AvgDeck {
    #[serde(default)]
    cards: HashMap<String, Vec<(String, u32)>>,
}

/// Lists that restate cards already present in the type-grouped lists. Keeping
/// them would double-count, so they are skipped once the card is seen.
const BASIC_LANDS: [&str; 6] = ["Plains", "Island", "Swamp", "Mountain", "Forest", "Wastes"];

pub enum FetchOutcome {
    Fetched(Box<CommanderData>),
    Missing,
}

pub async fn fetch_commander(
    client: &reqwest::Client,
    limiter: &RateLimiter,
    name: &str,
    slug: &str,
) -> Result<FetchOutcome> {
    limiter.acquire().await;
    let url = format!("https://json.edhrec.com/pages/commanders/{slug}.json");
    let resp = client.get(&url).send().await.context("requesting EDHREC commander page")?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND || resp.status() == reqwest::StatusCode::FORBIDDEN {
        return Ok(FetchOutcome::Missing);
    }
    let page: Page = resp
        .error_for_status()?
        .json()
        .await
        .with_context(|| format!("parsing EDHREC page for {slug}"))?;

    let mut seen = std::collections::HashSet::new();
    let mut cards = Vec::new();
    let mut deck_count = 0u32;

    for list in &page.container.json_dict.cardlists {
        // "Lands" here is the basic-land breakdown; the useful nonbasics are in
        // "Utility Lands".
        if list.header.eq_ignore_ascii_case("basic lands") {
            continue;
        }
        for cv in &list.cardviews {
            if BASIC_LANDS.contains(&cv.name.as_str()) || cv.name == name {
                continue;
            }
            if !seen.insert(cv.name.clone()) {
                continue;
            }
            deck_count = deck_count.max(cv.potential_decks);
            cards.push(EdhCard {
                name: cv.name.clone(),
                synergy: cv.synergy,
                num_decks: cv.num_decks,
                potential_decks: cv.potential_decks,
            });
        }
    }

    cards.sort_by(|a, b| b.inclusion().partial_cmp(&a.inclusion()).unwrap_or(std::cmp::Ordering::Equal));

    Ok(FetchOutcome::Fetched(Box::new(CommanderData {
        slug: slug.to_string(),
        name: name.to_string(),
        cards,
        avg_deck: Vec::new(),
        deck_count,
        fetched_at: chrono::Utc::now().to_rfc3339(),
    })))
}

pub async fn fetch_avg_deck(
    client: &reqwest::Client,
    limiter: &RateLimiter,
    slug: &str,
) -> Result<Vec<String>> {
    limiter.acquire().await;
    let url = format!("https://json.edhrec.com/pages/average-decks/{slug}.json");
    let resp = client.get(&url).send().await.context("requesting EDHREC average deck")?;
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let page: AvgDeckPage = match resp.json().await {
        Ok(p) => p,
        Err(_) => return Ok(Vec::new()),
    };
    let mut out = Vec::new();
    for (_, entries) in page.deck.cards {
        for (name, _qty) in entries {
            if !BASIC_LANDS.contains(&name.as_str()) {
                out.push(name);
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

// --- cache I/O ---

pub fn commander_path(dir: &Path, slug: &str) -> PathBuf {
    dir.join("edhrec").join(format!("{slug}.json"))
}

pub fn save_commander(dir: &Path, data: &CommanderData) -> Result<()> {
    let path = commander_path(dir, &data.slug);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_vec(data)?)?;
    Ok(())
}

pub fn load_all(dir: &Path) -> Result<HashMap<String, CommanderData>> {
    let edh = dir.join("edhrec");
    let mut out = HashMap::new();
    if !edh.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&edh)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("manifest.json") {
            continue;
        }
        match std::fs::read(&path).map_err(anyhow::Error::from).and_then(|b| {
            serde_json::from_slice::<CommanderData>(&b).map_err(anyhow::Error::from)
        }) {
            Ok(data) => {
                out.insert(data.slug.clone(), data);
            }
            // A truncated file from an interrupted crawl just gets re-fetched.
            Err(_) => continue,
        }
    }
    Ok(out)
}

pub fn manifest_path(dir: &Path) -> PathBuf {
    dir.join("edhrec").join("manifest.json")
}

pub fn load_manifest(dir: &Path) -> Manifest {
    std::fs::read(manifest_path(dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_manifest(dir: &Path, m: &Manifest) -> Result<()> {
    let path = manifest_path(dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_vec(m)?)?;
    Ok(())
}
