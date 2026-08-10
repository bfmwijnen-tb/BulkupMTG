//! Name normalisation.
//!
//! Three vocabularies have to line up: Archidekt exports, Scryfall oracle names
//! and EDHREC slugs. They disagree about accents, punctuation and how they spell
//! double-faced cards, so everything is funnelled through `key()` before it is
//! compared, and `slug()` reproduces EDHREC's URL form from a card name.

use unicode_normalization::UnicodeNormalization;

/// Strip combining marks so `Éomer` and `Eomer` compare equal.
fn deaccent(s: &str) -> String {
    s.nfd().filter(|c| !is_combining(*c)).collect()
}

fn is_combining(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x20D0..=0x20FF)
}

/// Comparison key: lowercase, unaccented, punctuation removed.
///
/// `Kroxa, Titan of Death's Hunger` -> `kroxa titan of deaths hunger`
pub fn key(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_space = true;
    for c in deaccent(name).chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_space = false;
        } else if c == '\'' || c == '\u{2019}' {
            // Elided, not treated as a separator, so `Ajani's` keys as `ajanis`
            // and matches however another source spells the apostrophe.
            continue;
        } else if c == '/' {
            // Preserve the face separator so `a // b` stays distinct from `ab`,
            // collapsing `//` and the spaces either side into a single slash.
            while out.ends_with(' ') {
                out.pop();
            }
            if !out.ends_with('/') {
                out.push('/');
            }
            last_space = true;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim().trim_end_matches('/').trim().to_string()
}

/// The front face of a multi-face name, as a comparison key.
///
/// Archidekt writes `Wax // Wane`, EDHREC sometimes writes only `Wax`. Matching
/// falls back to this when the full name misses.
pub fn front_key(name: &str) -> String {
    let k = key(name);
    match k.split_once('/') {
        Some((front, _)) => front.trim().to_string(),
        None => k,
    }
}

/// EDHREC URL slug for a commander name.
///
/// `Éomer, King of Rohan` -> `eomer-king-of-rohan`. Multi-face commanders are
/// keyed off their front face, which is how EDHREC names them.
pub fn slug(name: &str) -> String {
    let front = name.split("//").next().unwrap_or(name);
    let mut out = String::with_capacity(front.len());
    let mut pending_sep = false;
    for c in deaccent(front).chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('-');
            }
            pending_sep = false;
            out.push(c);
        } else if c == '\'' || c == '\u{2019}' {
            // Apostrophes vanish rather than becoming separators:
            // `Death's Hunger` -> `deaths-hunger`.
            continue;
        } else {
            pending_sep = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_match_edhrec() {
        assert_eq!(slug("Tasigur, the Golden Fang"), "tasigur-the-golden-fang");
        assert_eq!(slug("Éomer, King of Rohan"), "eomer-king-of-rohan");
        assert_eq!(slug("The Scarab God"), "the-scarab-god");
        assert_eq!(slug("Greasefang, Okiba Boss"), "greasefang-okiba-boss");
        assert_eq!(slug("Kroxa, Titan of Death's Hunger"), "kroxa-titan-of-deaths-hunger");
        assert_eq!(slug("Grub, Storied Matriarch // Grub, Notorious Auntie"), "grub-storied-matriarch");
    }

    #[test]
    fn keys_ignore_accents_and_punctuation() {
        assert_eq!(key("Éomer, Marshal of Rohan"), "eomer marshal of rohan");
        assert_eq!(key("Ajani's Pridemate"), "ajanis pridemate");
        assert_eq!(front_key("Wax // Wane"), "wax");
        assert_eq!(key("Wax // Wane"), "wax/wane");
    }
}
