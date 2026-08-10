//! Shared state and the background update job.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use serde::Serialize;
use tokio::sync::RwLock;

use crate::bulk::Collection;
use crate::edhrec::{self, CommanderData, RateLimiter};
use crate::names;
use crate::scryfall::{self, ScryfallIndex};

#[derive(Debug, Clone, Serialize, Default)]
pub struct Progress {
    pub running: bool,
    pub phase: String,
    pub done: usize,
    pub total: usize,
    pub message: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub errors: Vec<String>,
}

pub struct AppState {
    pub dir: PathBuf,
    pub client: reqwest::Client,
    pub limiter: RateLimiter,
    pub scryfall: RwLock<ScryfallIndex>,
    pub edhrec: RwLock<HashMap<String, CommanderData>>,
    pub collection: RwLock<Collection>,
    pub progress: RwLock<Progress>,
    updating: AtomicBool,
}

pub type Shared = Arc<AppState>;

impl AppState {
    pub fn new(dir: PathBuf, rps: f64) -> Result<Shared> {
        std::fs::create_dir_all(&dir)?;
        let scryfall_path = dir.join("scryfall-index.json");
        let scryfall = if scryfall_path.exists() {
            scryfall::load(&scryfall_path).unwrap_or_default()
        } else {
            ScryfallIndex::default()
        };
        let edh = edhrec::load_all(&dir)?;

        Ok(Arc::new(AppState {
            client: scryfall::client()?,
            limiter: RateLimiter::per_second(rps),
            scryfall: RwLock::new(scryfall),
            edhrec: RwLock::new(edh),
            collection: RwLock::new(Collection::default()),
            progress: RwLock::new(Progress::default()),
            updating: AtomicBool::new(false),
            dir,
        }))
    }

    async fn set_phase(&self, phase: &str, total: usize, message: &str) {
        let mut p = self.progress.write().await;
        p.phase = phase.to_string();
        p.done = 0;
        p.total = total;
        p.message = message.to_string();
    }

    async fn tick(&self, message: Option<String>) {
        let mut p = self.progress.write().await;
        p.done += 1;
        if let Some(m) = message {
            p.message = m;
        }
    }

    async fn note_error(&self, e: String) {
        let mut p = self.progress.write().await;
        // Keep the log bounded; a flaky network should not grow without limit.
        if p.errors.len() < 50 {
            p.errors.push(e);
        }
    }
}

/// Kick off an update if one is not already running.
///
/// `force` re-downloads Scryfall even when current and retries commanders
/// previously recorded as absent from EDHREC.
pub fn spawn_update(state: Shared, force: bool) -> bool {
    if state.updating.swap(true, Ordering::SeqCst) {
        return false;
    }
    tokio::spawn(async move {
        {
            let mut p = state.progress.write().await;
            *p = Progress {
                running: true,
                phase: "starting".into(),
                started_at: Some(chrono::Utc::now().to_rfc3339()),
                ..Default::default()
            };
        }

        if let Err(e) = run_update(&state, force).await {
            state.note_error(format!("update failed: {e:#}")).await;
        }

        {
            let mut p = state.progress.write().await;
            p.running = false;
            p.phase = "idle".into();
            p.finished_at = Some(chrono::Utc::now().to_rfc3339());
        }
        state.updating.store(false, Ordering::SeqCst);
    });
    true
}

async fn run_update(state: &Shared, force: bool) -> Result<()> {
    update_scryfall(state, force).await?;
    crawl_commanders(state, force).await?;
    crawl_avg_decks(state).await?;
    Ok(())
}

/// Refresh the card index only when Scryfall reports a newer dump than the one
/// on disk — a normal update after a new set is a single HEAD-ish request.
async fn update_scryfall(state: &Shared, force: bool) -> Result<()> {
    state.set_phase("scryfall", 1, "checking Scryfall bulk data").await;

    let remote = scryfall::remote_version(&state.client).await?;
    let (current, version) = {
        let idx = state.scryfall.read().await;
        (idx.source_updated_at.clone(), idx.version)
    };
    let compatible = version == scryfall::INDEX_VERSION;
    if !force && compatible && remote == current && !current.is_empty() {
        state.tick(Some("card index already current".into())).await;
        return Ok(());
    }

    state
        .set_phase("scryfall", 1, "downloading Scryfall bulk data (~24 MB)")
        .await;
    let index = scryfall::download(&state.client).await?;
    scryfall::save(&state.dir.join("scryfall-index.json"), &index)?;
    let n = index.commanders().len();
    *state.scryfall.write().await = index;
    state
        .tick(Some(format!("card index updated — {n} legal commanders")))
        .await;
    Ok(())
}

async fn crawl_commanders(state: &Shared, force: bool) -> Result<()> {
    let commanders: Vec<(String, String)> = {
        let idx = state.scryfall.read().await;
        idx.commanders()
            .into_iter()
            .map(|c| (c.name.clone(), names::slug(&c.name)))
            .collect()
    };

    let mut manifest = edhrec::load_manifest(&state.dir);
    let have = state.edhrec.read().await;
    // Only commanders we have never successfully fetched, minus the ones EDHREC
    // has already told us it does not know about.
    let todo: Vec<(String, String)> = commanders
        .into_iter()
        .filter(|(_, slug)| force || (!have.contains_key(slug) && !manifest.missing.contains_key(slug)))
        .collect();
    drop(have);

    state
        .set_phase("commanders", todo.len(), &format!("{} commanders to fetch", todo.len()))
        .await;
    if todo.is_empty() {
        return Ok(());
    }

    let queue = Arc::new(tokio::sync::Mutex::new(todo.into_iter()));
    let workers = 4;
    let mut handles = Vec::new();

    for _ in 0..workers {
        let state = Arc::clone(state);
        let queue = Arc::clone(&queue);
        handles.push(tokio::spawn(async move {
            loop {
                let Some((name, slug)) = queue.lock().await.next() else {
                    break;
                };
                match edhrec::fetch_commander(&state.client, &state.limiter, &name, &slug).await {
                    Ok(edhrec::FetchOutcome::Fetched(data)) => {
                        if let Err(e) = edhrec::save_commander(&state.dir, &data) {
                            state.note_error(format!("{name}: cache write failed: {e}")).await;
                        }
                        state.edhrec.write().await.insert(slug.clone(), *data);
                        state.tick(Some(name)).await;
                    }
                    Ok(edhrec::FetchOutcome::Missing) => {
                        state.tick(Some(format!("{name} (not on EDHREC)"))).await;
                    }
                    Err(e) => {
                        state.note_error(format!("{name}: {e}")).await;
                        state.tick(None).await;
                    }
                }
            }
        }));
    }
    for h in handles {
        let _ = h.await;
    }

    // Record outcomes so the next update skips what is already settled.
    let now = chrono::Utc::now().to_rfc3339();
    let have = state.edhrec.read().await;
    for (slug, data) in have.iter() {
        manifest.fetched.insert(slug.clone(), data.fetched_at.clone());
    }
    {
        let idx = state.scryfall.read().await;
        for c in idx.commanders() {
            let slug = names::slug(&c.name);
            if !have.contains_key(&slug) {
                manifest.missing.insert(slug, now.clone());
            } else {
                manifest.missing.remove(&slug);
            }
        }
    }
    drop(have);
    edhrec::save_manifest(&state.dir, &manifest)?;
    Ok(())
}

/// Second pass: the concrete 99-card lists. Split out so the UI is usable as
/// soon as the synergy data lands.
async fn crawl_avg_decks(state: &Shared) -> Result<()> {
    let todo: Vec<String> = state
        .edhrec
        .read()
        .await
        .values()
        .filter(|d| d.avg_deck.is_empty())
        .map(|d| d.slug.clone())
        .collect();

    state
        .set_phase("average decks", todo.len(), &format!("{} average decks to fetch", todo.len()))
        .await;
    if todo.is_empty() {
        return Ok(());
    }

    let queue = Arc::new(tokio::sync::Mutex::new(todo.into_iter()));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let state = Arc::clone(state);
        let queue = Arc::clone(&queue);
        handles.push(tokio::spawn(async move {
            loop {
                let Some(slug) = queue.lock().await.next() else {
                    break;
                };
                match edhrec::fetch_avg_deck(&state.client, &state.limiter, &slug).await {
                    Ok(list) => {
                        if !list.is_empty() {
                            let mut guard = state.edhrec.write().await;
                            if let Some(d) = guard.get_mut(&slug) {
                                d.avg_deck = list;
                                let snapshot = d.clone();
                                drop(guard);
                                let _ = edhrec::save_commander(&state.dir, &snapshot);
                            }
                        }
                        state.tick(Some(slug)).await;
                    }
                    Err(e) => {
                        state.note_error(format!("{slug}: {e}")).await;
                        state.tick(None).await;
                    }
                }
            }
        }));
    }
    for h in handles {
        let _ = h.await;
    }
    Ok(())
}
