# Minimal Shiny wrapper around the generated page.
#
# Only use this if your Posit Connect policy will not accept static content —
# see README.md. The page does all of its work in the browser, so every R
# process this spawns sits idle. `rsconnect::deployDoc("index.html")` is the
# better deployment.
#
# Copy the generated page next to this file before deploying:
#     cp ../index.html .

library(shiny)

page <- if (file.exists("index.html")) "index.html" else "../index.html"
if (!file.exists(page)) {
  stop("index.html not found. Run `cargo run --release --bin bundle`, ",
       "then copy the file into this directory.")
}

# Served as a static asset rather than inlined: it is a complete HTML document,
# and nesting <html> inside Shiny's own page would produce invalid markup.
addResourcePath("bulkup", normalizePath(dirname(page)))

ui <- tags$html(
  tags$head(
    tags$title("BulkupMTG"),
    tags$style(HTML("html, body { margin: 0; height: 100%; overflow: hidden; }"))
  ),
  tags$body(
    tags$iframe(
      src = paste0("bulkup/", basename(page)),
      style = "border: 0; width: 100%; height: 100vh; display: block;"
    )
  )
)

server <- function(input, output, session) {}

shinyApp(ui, server)
