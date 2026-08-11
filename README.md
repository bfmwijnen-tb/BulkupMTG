# BulkupMTG

Finds the Commander decks hiding in your bulk. Upload Archidekt bulk exports and
every legal commander gets scored against what you actually own, using EDHREC's
aggregate deck data.

## Install

Download the build for your machine from
[Releases](../../releases/latest), unzip it, and run it. Nothing else to
install — it is one self-contained file with the web UI compiled in.

| | |
| --- | --- |
| **Windows** | `bulkup-windows-x64.zip` → run `bulkup.exe` |
| **Mac (M1/M2/M3/M4)** | `bulkup-macos-apple-silicon.zip` → run `bulkup` |
| **Mac (Intel)** | `bulkup-macos-intel.zip` → run `bulkup` |
| **Linux** | `bulkup-linux-x64.zip` → run `bulkup` |

It opens <http://localhost:8080> in your browser by itself. On a first run it
starts downloading card data immediately and results appear as they arrive —
there is no setup step to remember.

Two platform notes. On macOS the first launch is blocked because the binary is
unsigned: right-click it, choose **Open**, then **Open** again. On Windows,
SmartScreen may show "Windows protected your PC" — click **More info** →
**Run anyway**.

### Or build from source

```bash
cargo build --release
./target/release/bulkup
```

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
| **deck ready** | Share of a real 99-card deck you hold, **counting basic lands**. The best single answer to "could I build this tonight?" |
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

## How long the first run takes

Roughly 25 minutes of background downloading, and you can use the tool while it
happens — commanders are ranked as they arrive. The limit is politeness, not
speed: EDHREC has no public API, so the crawler holds itself to 2.5 requests a
second.

Average decklists are fetched lazily, so opening any commander pulls its list
straight away rather than waiting for the background pass to reach it.

Everything is cached in `data/` (~110 MB), so this happens once.

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
| `BULKUP_OPEN` | `1` | Set to `0` to stop it opening a browser on start. |

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

## Standalone page (no install at all)

Some machines block running newly-created executables, which stops both the
release binary and `cargo build` (build scripts compile and run small
executables of their own). For those, everything ships as a single web page:

```bash
cargo run --release --bin bundle    # writes index.html
```

`index.html` is committed at the root of this repo, so it can be downloaded
straight from GitHub with nothing to build. It is ~7 MB and completely
self-contained — open it in a browser: no install, no server, no executable.
The crawled EDHREC data is gzipped and embedded, the page inflates it with
`DecompressionStream`, and card art loads from Scryfall's CDN.

Because everything runs in the browser, the page can also be hosted as static
content — see [`deploy/`](deploy/) for Posit Connect.

Both EDHREC and Scryfall send `Access-Control-Allow-Origin: *`, so the page can
also fetch new commanders itself: **Check for new commanders** diffs Scryfall's
commander list against the embedded data and pulls only what is missing,
storing it in `localStorage`. That covers a new set without regenerating the
file.


## Basic lands, and why "40 of 100" understates you

A Commander deck is 99 cards, and a large slice of that is basic land you
already own by the shoebox. EDHREC's ranked lists contain no basics at all, so
"40 of the top 100" silently compares your bulk against a pool that excludes a
third of the finished deck. The effect is worst exactly where you would expect:
across the crawled data the average decklist runs between **8 and 69 basics**,
median 23, and the mono-coloured commanders sit at the high end.

So the headline figure is **deck readiness**:

    (nonbasics you own from the average decklist + basics that deck runs) / 99

The meter under each commander shows it in two tones — gold for cards you own,
slate for the basics. Temmet reads *72% of a deck — 58 owned + 13 basics*,
which is a far more honest answer than *61/100*.

## Choosing which cards "belong to" a commander

There is no single right answer, so the **Card pool** selector offers three,
and the pool size is adjustable:

| Pool | What it is | When it is the right one |
| --- | --- | --- |
| **Most played** | Highest inclusion rate first | Matches what EDHREC's page shows, but the head of the list is format staples |
| **Highest synergy** | Biggest gap versus the colour baseline first | Finding the cards that are genuinely *about* this commander |
| **Average decklist** | Only cards in EDHREC's actual average deck | **Usually the best.** A real 99-card deck someone would sleeve up — correct size and composition, no arbitrary cutoff |

**Exclude lands** drops every land from the pool, which is useful when your
bulk is short on fixing and you want to judge the spells on their own.
