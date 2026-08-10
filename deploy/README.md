# Hosting BulkupMTG on Posit Connect

Short answer: **yes, but don't write a Shiny app for it.** `bulkup.html` is
already a complete browser application — the parsing, matching and scoring all
run client-side. Publish it as **static content** and Connect just serves the
file.

## Recommended: static content

```r
install.packages("rsconnect")
rsconnect::deployDoc("bulkup.html")
```

Or in the Connect UI: **Publish → Static content**, and upload `bulkup.html`.

Why this is the right shape:

* **No R process per viewer.** Connect serves one file; the browser does the
  work. Ten simultaneous users cost the same as one.
* **Nothing to schedule, nothing to keep warm.** No idle timeouts, no worker
  pool tuning.
* **Uploaded collections never leave the browser.** Bulk exports are read with
  the File API and scored in memory, so nothing lands on the server. Worth
  knowing if colleagues are uploading their collections.

The only server-side cost is the ~7 MB download, which the browser caches.

## If your Connect policy requires a Shiny app

`app.R` here is a thin wrapper that serves the page as a static asset and frames
it. Copy the generated page next to it first:

```bash
cp ../bulkup.html .
rsconnect::deployApp()      # from this directory
```

Be aware this is strictly worse than static content: every viewer occupies an R
process that does nothing. Use it only if publishing static content is not
permitted.

> Not tested — there is no R in the container this was written in. The wrapper
> is ten lines and standard Shiny, but treat it as a starting point rather than
> something known to run.

## Rewriting it as a real Shiny app

Possible, and a bad trade. The scoring would move to the server: 3,348
commanders × ~250 cards re-scored on every control change, per user, in R
instead of in the viewer's browser. The 13.6 MB dataset would need loading once
at startup (outside `server()`) to be shared across sessions, uploads would land
on the server, and the app would be slower and less scalable than the static
page while doing exactly the same job.

It is only worth it if you want something the browser genuinely cannot do —
per-user saved collections, authentication-gated data, or usage logging. Say so
and it can be built; otherwise static content is the answer.

## Updating after a new set

Regenerate and republish:

```bash
cargo run --release --bin bundle   # rewrites bulkup.html
rsconnect::deployDoc("bulkup.html")
```

Viewers can also press **Check for new commanders** in the page itself, which
fetches straight from Scryfall and EDHREC and keeps the result in their own
browser's `localStorage` — no redeploy needed, but it is per-viewer.
