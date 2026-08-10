# BulkupMTG

Finds the Commander decks hiding in your bulk. Upload Archidekt bulk exports and
every legal commander gets scored against what you actually own, using EDHREC's
aggregate deck data.

```
cargo build --release
./target/release/bulkup update     # build the cache (first run, ~45 min)
./target/release/bulkup            # serve http://localhost:8080
```

Single binary, no runtime to install. The web UI is compiled in.

## What it ranks on

The obvious measure — "how many of this commander's top 100 cards do I own" — is
misleading, and the tool reports it without ranking on it. Two reasons:

* **Staples flatter everything.** Arcane Signet, Lightning Greaves, Cultivate,
  Command Tower and friends appear in the top 100 of nearly every commander in
  the right colours. A typical bulk box clears 25–30 hits before synergy is
  considered at all.
* **Colour identity skews it.** A five-colour commander can legally use every
  card in the box; a mono-white one can use a fraction. Rank on raw count and
  you have mostly ranked commanders by how many colours they have.

So the default ranking is **synergy score**: the sum of EDHREC's per-card synergy
figure over the cards you own. EDHREC defines synergy as *inclusion rate in this
commander's decks minus inclusion rate across the same colour identity
generally*, so a staple contributes ≈ 0 and a signature card contributes a lot.
Negative synergy is floored at zero — a card being under-played here shouldn't
count against a commander you can still build.

Each result also shows:

| Column | Meaning |
| --- | --- |
| **synergy score** | Ranking key. Sum of positive synergy over owned cards. |
| **N/100 owned** | The raw count, and what the threshold filters on. |
| **non-staple** | How much of that count is *not* generic filler. |
| **avg deck** | Owned share of EDHREC's actual 99-card average decklist — the best single answer to "could I build this tonight?" |
| **sources** | Which uploaded file each hit came from, so you know which box to open. |
| **✔** | You already own the commander itself. |

## Data sources

* **Scryfall** — card names, colour identity, and image URLs, via the daily
  [bulk data](https://scryfall.com/docs/api/bulk-data) dump (~24 MB gzipped).
  One download replaces ~100k API calls, which is what Scryfall asks for.
  Card images are referenced as CDN links and fetched lazily by the browser, so
  nothing bulky is stored locally.
* **EDHREC** — ~260 ranked cards per commander plus an average decklist, read
  from the same JSON its own front end uses. There is no documented public API,
  so the crawler rate-limits itself (2.5 req/s by default), identifies itself in
  the `User-Agent`, and caches everything to disk so a full crawl happens once.

## Updating after a new set

Press **Update data**, or run `bulkup update` on a schedule. It is incremental:

1. Ask Scryfall whether the bulk dump is newer than the cached one; skip the
   download if not.
2. Diff Scryfall's commander list against the cache and fetch **only new
   commanders**. A new set is ~30 commanders — under a minute, not a re-crawl.
3. Fill in any missing average decklists.

Commanders EDHREC does not know about are recorded in a manifest so they aren't
retried every time. **Full rebuild** ignores all of that and re-fetches
everything.

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `BULKUP_PORT` | `8080` | Port for the web UI. |
| `BULKUP_DATA` | `data` | Cache directory. |
| `BULKUP_RPS` | `2.5` | EDHREC requests per second. Please don't raise this much. |

## Input format

Archidekt bulk exports, one card per line:

```
1 Tasigur, the Golden Fang (TDC) 197
3 Crash and Burn (DFT) 119
1 Hardbristle Bandit (OTJ) 168 *F*
1 The Ring // The Ring Tempts You (TLTR) H13
```

Quantities are summed across files, non-numeric collector numbers and foil
markers are handled, and section headers are skipped. Any line the parser can't
read is surfaced in the UI rather than dropped silently. Re-uploading a file of
the same name replaces it instead of double-counting.

## Layout

| File | Role |
| --- | --- |
| `src/names.rs` | Name normalisation — reconciles Archidekt, Scryfall and EDHREC spellings. |
| `src/bulk.rs` | Archidekt export parsing and collection merging. |
| `src/scryfall.rs` | Bulk-data download and the card index. |
| `src/edhrec.rs` | Rate-limited crawler and on-disk cache. |
| `src/score.rs` | Matching and the synergy scoring model. |
| `src/app.rs` | Shared state and the background update job. |
| `src/web.rs` | HTTP API and embedded UI. |
