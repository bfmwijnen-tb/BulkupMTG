//! Builds `dist/bulkup.html` — the whole tool as one self-contained web page.
//!
//! This exists because a compiled binary is unusable on a managed machine that
//! blocks running new executables. A single HTML file needs no install and no
//! server: the crawled EDHREC data is gzipped, base64'd and embedded, the page
//! inflates it with `DecompressionStream`, and card art is loaded straight from
//! Scryfall's CDN.
//!
//! Run `bulkup update` first so `data/` is populated, then `cargo run --bin bundle`.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::Engine;
use serde::Serialize;

#[path = "../edhrec.rs"]
mod edhrec;
#[path = "../names.rs"]
mod names;
#[path = "../scryfall.rs"]
mod scryfall;

const TEMPLATE: &str = include_str!("../../ui/standalone.html");

/// `[name, scryfall uuid]`. The uuid rebuilds a CDN image URL client-side,
/// which is far smaller than storing the URL itself.
#[derive(Serialize)]
struct Card(String, String);

/// `[name, uuid, colour identity, deck count, cards, average deck]`,
/// where each card entry is `[index, synergy %, inclusion %]` and the average
/// deck is a list of indices.
#[derive(Serialize)]
struct Commander(
    String,
    String,
    String,
    u32,
    Vec<(usize, i32, i32)>,
    Vec<usize>,
);

#[derive(Serialize)]
struct Bundle {
    generated: String,
    cards: Vec<Card>,
    commanders: Vec<Commander>,
}

/// Scryfall image URLs end `/<a>/<b>/<uuid>.jpg`, so the uuid is all we keep.
fn uuid_from_image(url: &str) -> String {
    url.rsplit('/')
        .next()
        .and_then(|f| f.split('.').next())
        .unwrap_or("")
        .to_string()
}

fn main() -> Result<()> {
    let dir: PathBuf = std::env::var("BULKUP_DATA")
        .unwrap_or_else(|_| "data".to_string())
        .into();

    let index = scryfall::load(&dir.join("scryfall-index.json"))
        .context("loading the Scryfall index — run `bulkup update` first")?;
    let commanders = edhrec::load_all(&dir)?;
    anyhow::ensure!(
        !commanders.is_empty(),
        "no EDHREC data in {} — run `bulkup update` first",
        dir.display()
    );

    let mut card_ids: HashMap<String, usize> = HashMap::new();
    let mut cards: Vec<Card> = Vec::new();
    let mut intern = |name: &str, index: &scryfall::ScryfallIndex| -> usize {
        if let Some(i) = card_ids.get(name) {
            return *i;
        }
        let info = index.lookup(name);
        let uuid = info
            .and_then(|i| i.image_normal.as_deref())
            .map(uuid_from_image)
            .unwrap_or_default();
        let i = cards.len();
        cards.push(Card(name.to_string(), uuid));
        card_ids.insert(name.to_string(), i);
        i
    };

    let mut list: Vec<&crate::edhrec::CommanderData> = commanders.values().collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = Vec::with_capacity(list.len());
    for data in list {
        let info = index.lookup(&data.name);
        let entries: Vec<(usize, i32, i32)> = data
            .cards
            .iter()
            // Basics are excluded here rather than in the page, so the browser
            // never needs type lines.
            .filter(|c| !index.lookup(&c.name).is_some_and(|i| i.is_basic))
            .map(|c| {
                (
                    intern(&c.name, &index),
                    (c.synergy * 100.0).round() as i32,
                    (c.inclusion() * 100.0).round() as i32,
                )
            })
            .collect();
        let avg: Vec<usize> = data
            .avg_deck
            .iter()
            .filter(|n| !index.lookup(n).is_some_and(|i| i.is_basic))
            .map(|n| intern(n, &index))
            .collect();

        out.push(Commander(
            data.name.clone(),
            info.and_then(|i| i.image_normal.as_deref())
                .map(uuid_from_image)
                .unwrap_or_default(),
            info.map(|i| i.color_identity.join("")).unwrap_or_default(),
            data.deck_count,
            entries,
            avg,
        ));
    }

    let bundle = Bundle {
        generated: chrono::Utc::now().format("%Y-%m-%d").to_string(),
        cards,
        commanders: out,
    };

    let raw = serde_json::to_vec(&bundle)?;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    std::io::Write::write_all(&mut enc, &raw)?;
    let gz = enc.finish()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&gz);

    let html = TEMPLATE.replace("__BUNDLE__", &b64);
    std::fs::create_dir_all("dist")?;
    std::fs::write("dist/bulkup.html", &html)?;

    println!(
        "dist/bulkup.html — {} commanders, {} cards\n  json {:.1} MB → gz {:.1} MB → page {:.1} MB",
        bundle.commanders.len(),
        bundle.cards.len(),
        raw.len() as f64 / 1e6,
        gz.len() as f64 / 1e6,
        html.len() as f64 / 1e6
    );
    Ok(())
}
