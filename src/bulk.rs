//! Archidekt bulk-export parsing.
//!
//! The export is one card per line: `1 Tasigur, the Golden Fang (TDC) 197`,
//! optionally suffixed with `*F*` for foils. Collector numbers are not always
//! numeric (`(TLTR) H13`), and section headers or blank lines can appear, so the
//! parser is deliberately forgiving: anything it cannot read is reported rather
//! than dropped silently.

use std::collections::HashMap;

use regex::Regex;
use serde::Serialize;

use crate::names;

/// One distinct card in the merged collection, with per-file provenance so the
/// UI can say *which box* to go digging in.
#[derive(Debug, Clone, Serialize)]
pub struct OwnedCard {
    pub name: String,
    pub qty: u32,
    /// file name -> copies in that file
    pub sources: HashMap<String, u32>,
}

#[derive(Debug, Default, Serialize)]
pub struct Collection {
    /// Keyed by `names::key`, plus front-face aliases for multi-face cards.
    #[serde(skip)]
    pub by_key: HashMap<String, OwnedCard>,
    pub files: Vec<FileReport>,
    /// Raw upload text, kept so removing one file can be handled by replaying
    /// the rest rather than by unpicking merged counts. A few thousand lines per
    /// file makes this far cheaper than tracking per-card deltas.
    #[serde(skip)]
    raw: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileReport {
    pub name: String,
    pub lines: usize,
    pub cards: u32,
    pub unparsed: Vec<String>,
}

/// `1 Card Name (SET) 123 *F*` — the name is greedy so a card whose own title
/// contains parentheses still yields the trailing set/collector pair.
fn line_re() -> &'static Regex {
    static CELL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        Regex::new(r"^\s*(\d+)\s+(.+?)\s*(?:\(([A-Za-z0-9]{2,6})\)\s+(\S+))?\s*(?:\*[FE]\*)?\s*$")
            .expect("static regex")
    })
}

/// Lines that are section markers rather than cards.
fn is_header(line: &str) -> bool {
    let l = line.trim().trim_end_matches(':').to_ascii_lowercase();
    matches!(
        l.as_str(),
        "commander" | "sideboard" | "deck" | "maybeboard" | "companion" | "tokens"
    )
}

impl Collection {
    /// Add or replace a file. Re-uploading the same name replaces that file's
    /// contribution instead of doubling it.
    pub fn add_file(&mut self, file_name: &str, contents: &str) {
        self.raw.retain(|(n, _)| n != file_name);
        self.raw.push((file_name.to_string(), contents.to_string()));
        self.rebuild();
    }

    /// Remove one file's cards.
    pub fn remove_file(&mut self, file_name: &str) {
        self.raw.retain(|(n, _)| n != file_name);
        self.rebuild();
    }

    fn rebuild(&mut self) {
        self.by_key.clear();
        self.files.clear();
        let raw = std::mem::take(&mut self.raw);
        for (name, contents) in &raw {
            self.ingest(name, contents);
        }
        self.raw = raw;
        self.add_front_face_aliases();
    }

    /// Alias multi-face cards by their front face, so a list that says `Wax`
    /// still matches a collection that says `Wax // Wane`. Runs only after all
    /// files are counted — an alias built mid-ingest would freeze a stale
    /// quantity.
    fn add_front_face_aliases(&mut self) {
        let aliases: Vec<(String, OwnedCard)> = self
            .by_key
            .iter()
            .filter_map(|(k, card)| {
                let fk = names::front_key(&card.name);
                (fk != *k && !fk.is_empty() && !self.by_key.contains_key(&fk))
                    .then(|| (fk, card.clone()))
            })
            .collect();
        for (k, card) in aliases {
            self.by_key.entry(k).or_insert(card);
        }
    }

    fn ingest(&mut self, file_name: &str, contents: &str) {
        let re = line_re();
        let mut report = FileReport {
            name: file_name.to_string(),
            lines: 0,
            cards: 0,
            unparsed: Vec::new(),
        };

        for raw in contents.lines() {
            let line = raw.trim_start_matches('\u{feff}').trim();
            if line.is_empty() || is_header(line) {
                continue;
            }
            report.lines += 1;

            let Some(caps) = re.captures(line) else {
                report.unparsed.push(line.to_string());
                continue;
            };
            let qty: u32 = caps[1].parse().unwrap_or(1);
            let name = caps[2].trim();
            if name.is_empty() {
                report.unparsed.push(line.to_string());
                continue;
            }

            report.cards += qty;
            self.insert(name, qty, file_name);
        }

        self.files.push(report);
    }

    fn insert(&mut self, name: &str, qty: u32, file_name: &str) {
        let entry = self.by_key.entry(names::key(name)).or_insert_with(|| OwnedCard {
            name: name.to_string(),
            qty: 0,
            sources: HashMap::new(),
        });
        entry.qty += qty;
        *entry.sources.entry(file_name.to_string()).or_insert(0) += qty;
    }

    pub fn get(&self, key: &str) -> Option<&OwnedCard> {
        self.by_key.get(key)
    }

    pub fn total_cards(&self) -> u32 {
        self.files.iter().map(|f| f.cards).sum()
    }

    pub fn distinct_cards(&self) -> usize {
        // Aliases share the same underlying card, so count by name.
        let names: std::collections::HashSet<&str> =
            self.by_key.values().map(|c| c.name.as_str()).collect();
        names.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_archidekt_lines() {
        let mut c = Collection::default();
        c.add_file(
            "test.txt",
            "1 Tasigur, the Golden Fang (TDC) 197\n\
             3 Crash and Burn (DFT) 119\n\
             1 Hardbristle Bandit (OTJ) 168 *F*\n\
             1 The Ring // The Ring Tempts You (TLTR) H13\n\
             \n\
             Sideboard\n\
             1 Wax // Wane (DMR) 211\n",
        );
        assert_eq!(c.files[0].unparsed.len(), 0, "unparsed: {:?}", c.files[0].unparsed);
        assert_eq!(c.total_cards(), 7);
        assert_eq!(c.get(&names::key("Crash and Burn")).unwrap().qty, 3);
        // Foil marker must not leak into the name.
        assert!(c.get(&names::key("Hardbristle Bandit")).is_some());
        // Non-numeric collector numbers still parse.
        assert!(c.get(&names::key("The Ring // The Ring Tempts You")).is_some());
        // Front-face alias resolves.
        assert!(c.get("wax").is_some());
    }

    #[test]
    fn front_face_alias_reflects_final_quantity() {
        // The alias must be built after all files are counted, not on first sight
        // of the card, or the second copy would be invisible through the alias.
        let mut c = Collection::default();
        c.add_file("a.txt", "1 Wax // Wane (DMR) 211\n");
        c.add_file("b.txt", "2 Wax // Wane (DMR) 211\n");
        assert_eq!(c.get("wax").unwrap().qty, 3);
        assert_eq!(c.get(&names::key("Wax // Wane")).unwrap().qty, 3);
    }

    #[test]
    fn reupload_replaces_rather_than_doubles() {
        let mut c = Collection::default();
        c.add_file("a.txt", "2 Sol Ring (LEA) 1\n");
        c.add_file("a.txt", "2 Sol Ring (LEA) 1\n");
        assert_eq!(c.get(&names::key("Sol Ring")).unwrap().qty, 2);
        assert_eq!(c.files.len(), 1);
        assert_eq!(c.total_cards(), 2);
    }

    #[test]
    fn remove_file_drops_only_that_file() {
        let mut c = Collection::default();
        c.add_file("a.txt", "2 Sol Ring (LEA) 1\n1 Cultivate (M21) 177\n");
        c.add_file("b.txt", "1 Sol Ring (C21) 263\n");
        c.remove_file("b.txt");
        assert_eq!(c.get(&names::key("Sol Ring")).unwrap().qty, 2);
        assert!(c.get(&names::key("Cultivate")).is_some());
        assert_eq!(c.files.len(), 1);
    }

    #[test]
    fn merges_across_files_with_provenance() {
        let mut c = Collection::default();
        c.add_file("a.txt", "2 Sol Ring (LEA) 1\n");
        c.add_file("b.txt", "1 Sol Ring (C21) 263\n");
        let card = c.get(&names::key("Sol Ring")).unwrap();
        assert_eq!(card.qty, 3);
        assert_eq!(card.sources["a.txt"], 2);
        assert_eq!(card.sources["b.txt"], 1);
    }
}
