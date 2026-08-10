//! HTTP API and the embedded single-page UI.

use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::app::Shared;
use crate::score::{self, RankOptions, Sort};

const INDEX_HTML: &str = include_str!("../ui/index.html");

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/status", get(status))
        .route("/api/collection", get(collection_summary).post(upload).delete(clear))
        .route("/api/results", get(results))
        .route("/api/commander/{slug}", get(commander))
        .route("/api/update", post(update))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}

async fn index() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], Html(INDEX_HTML))
}

#[derive(Serialize)]
struct Status {
    progress: crate::app::Progress,
    commanders_cached: usize,
    commanders_known: usize,
    avg_decks_cached: usize,
    card_index_size: usize,
    card_index_updated: String,
    collection_files: usize,
    collection_cards: u32,
    collection_distinct: usize,
}

async fn status(State(state): State<Shared>) -> Json<Status> {
    let edh = state.edhrec.read().await;
    let idx = state.scryfall.read().await;
    let col = state.collection.read().await;
    Json(Status {
        progress: state.progress.read().await.clone(),
        commanders_cached: edh.len(),
        commanders_known: state.commander_count(),
        avg_decks_cached: edh.values().filter(|d| !d.avg_deck.is_empty()).count(),
        card_index_size: idx.cards.len(),
        card_index_updated: idx.source_updated_at.clone(),
        collection_files: col.files.len(),
        collection_cards: col.total_cards(),
        collection_distinct: col.distinct_cards(),
    })
}

#[derive(Serialize)]
struct CollectionSummary {
    files: Vec<crate::bulk::FileReport>,
    total_cards: u32,
    distinct_cards: usize,
}

async fn collection_summary(State(state): State<Shared>) -> Json<CollectionSummary> {
    let col = state.collection.read().await;
    Json(CollectionSummary {
        files: col.files.clone(),
        total_cards: col.total_cards(),
        distinct_cards: col.distinct_cards(),
    })
}

/// Accepts any number of bulk exports in one request. Re-uploading a file of the
/// same name replaces that file's contribution rather than double-counting it.
async fn upload(
    State(state): State<Shared>,
    mut multipart: Multipart,
) -> Result<Json<CollectionSummary>, (StatusCode, String)> {
    let mut col = state.collection.write().await;
    let mut any = false;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad upload: {e}")))?
    {
        let name = field
            .file_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "upload.txt".to_string());
        let text = field
            .text()
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("reading {name}: {e}")))?;
        col.add_file(&name, &text);
        any = true;
    }

    if !any {
        return Err((StatusCode::BAD_REQUEST, "no files in upload".into()));
    }

    Ok(Json(CollectionSummary {
        files: col.files.clone(),
        total_cards: col.total_cards(),
        distinct_cards: col.distinct_cards(),
    }))
}

#[derive(Deserialize)]
struct RemoveQuery {
    #[serde(default)]
    file: Option<String>,
}

/// With `?file=` removes a single upload; otherwise clears the collection.
async fn clear(
    State(state): State<Shared>,
    Query(q): Query<RemoveQuery>,
) -> Json<CollectionSummary> {
    let mut col = state.collection.write().await;
    match q.file {
        Some(name) => col.remove_file(&name),
        None => *col = crate::bulk::Collection::default(),
    }
    Json(CollectionSummary {
        files: col.files.clone(),
        total_cards: col.total_cards(),
        distinct_cards: col.distinct_cards(),
    })
}

#[derive(Deserialize)]
struct ResultsQuery {
    #[serde(default = "default_min")]
    min: usize,
    #[serde(default)]
    sort: String,
    #[serde(default)]
    owned: bool,
    #[serde(default)]
    colors: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_min() -> usize {
    40
}
fn default_limit() -> usize {
    200
}

#[derive(Serialize)]
struct ResultsResponse {
    total_matches: usize,
    commanders_scored: usize,
    matches: Vec<score::CommanderMatch>,
}

async fn results(
    State(state): State<Shared>,
    Query(q): Query<ResultsQuery>,
) -> Json<ResultsResponse> {
    let edh = state.edhrec.read().await;
    let idx = state.scryfall.read().await;
    let col = state.collection.read().await;

    let colors = q.colors.as_ref().map(|s| {
        s.chars()
            .filter(|c| "WUBRG".contains(c.to_ascii_uppercase()))
            .map(|c| c.to_ascii_uppercase().to_string())
            .collect::<Vec<_>>()
    });

    let opts = RankOptions {
        min_top_hits: q.min,
        sort: Sort::parse(&q.sort),
        owned_commander_only: q.owned,
        colors,
    };
    let mut matches = score::rank(&edh, &col, &idx, &opts);
    let total = matches.len();
    matches.truncate(q.limit);

    Json(ResultsResponse {
        total_matches: total,
        commanders_scored: edh.len(),
        matches,
    })
}

async fn commander(
    State(state): State<Shared>,
    Path(slug): Path<String>,
) -> Result<Json<score::CommanderDetail>, StatusCode> {
    let edh = state.edhrec.read().await;
    let data = edh.get(&slug).ok_or(StatusCode::NOT_FOUND)?;
    let idx = state.scryfall.read().await;
    let col = state.collection.read().await;
    Ok(Json(score::detail(data, &col, &idx)))
}

#[derive(Deserialize)]
struct UpdateQuery {
    #[serde(default)]
    force: bool,
}

async fn update(State(state): State<Shared>, Query(q): Query<UpdateQuery>) -> impl IntoResponse {
    if crate::app::spawn_update(state, q.force) {
        (StatusCode::ACCEPTED, "update started")
    } else {
        (StatusCode::CONFLICT, "an update is already running")
    }
}
