//! BulkupMTG — find the Commander decks hiding in your bulk.
//!
//! Serves a local web UI. Upload Archidekt bulk exports, and every commander is
//! scored against what you own using EDHREC's synergy data.

mod app;
mod bulk;
mod edhrec;
mod names;
mod score;
mod scryfall;

use std::path::PathBuf;

use anyhow::{Context, Result};

const DEFAULT_PORT: u16 = 8080;
const DEFAULT_RPS: f64 = 2.5;

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let data_dir: PathBuf = std::env::var("BULKUP_DATA")
        .unwrap_or_else(|_| "data".to_string())
        .into();
    let rps = env_or("BULKUP_RPS", DEFAULT_RPS);

    let state = app::AppState::new(data_dir.clone(), rps)?;

    // `bulkup update` runs the crawl headlessly, for a cron job or a first run.
    if args.get(1).map(String::as_str) == Some("update") {
        let force = args.iter().any(|a| a == "--force");
        println!("Updating cache in {} (EDHREC at {rps:.1} req/s)…", data_dir.display());
        app::spawn_update(std::sync::Arc::clone(&state), force);
        let mut last = String::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let p = state.progress.read().await.clone();
            let line = format!("[{}] {}/{} {}", p.phase, p.done, p.total, p.message);
            if line != last {
                println!("{line}");
                last = line;
            }
            if !p.running && p.finished_at.is_some() {
                for e in &p.errors {
                    eprintln!("warning: {e}");
                }
                break;
            }
        }
        println!("Done. {} commanders cached.", state.edhrec.read().await.len());
        return Ok(());
    }

    let port: u16 = env_or("BULKUP_PORT", DEFAULT_PORT);
    let cached = state.edhrec.read().await.len();
    let indexed = state.scryfall.read().await.cards.len();

    println!("BulkupMTG — http://localhost:{port}");
    println!("  data dir: {}", data_dir.display());
    println!("  cards indexed: {indexed}");
    println!("  commanders cached: {cached}");
    if cached == 0 {
        println!("  no EDHREC data yet — press Update in the UI, or run `bulkup update`");
    }

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("binding port {port}"))?;
    axum::serve(listener, web::router(state)).await.context("serving")?;
    Ok(())
}

mod web;
