//! Name normalisation.
//!
//! Three vocabularies have to line up: Archidekt exports, Scryfall oracle names
//! and EDHREC slugs. They disagree about accents, punctuation and how they spell
//! double-faced cards, so everything is funnelled through `key()` before it is
//! compared, and `slug()` reproduces EDHREC's URL form from a card name.
//!
//! Two rules are load-bearing and were both learned from EDHREC's live URLs:
//!
//! * **Periods and apostrophes are elided, not separated.** `M.O.D.O.K.` is
//!   `modok`, and `Nick Fury, Agent of S.H.I.E.L.D.` is
//!   `nick-fury-agent-of-shield`. Treating a period as a word break yields
//!   `m-o-d-o-k`, which 403s.
//! * **Only a *spaced* ` // ` separates card faces.** Scryfall writes real
//!   multi-face cards as `Wax // Wane`, but `SP//dr, Piloted by Peni` is one
//!   face whose name merely contains slashes — it slugs to
//!   `sp-dr-piloted-by-peni`.

use unicode_normalization::UnicodeNormalization;

/// Bump when these rules change. Cached artefacts keyed by name embed the old
/// rules and must be rebuilt rather than trusted.
pub const SLUG_VERSION: u32 = 3;

/// Scryfall's separator for genuine multi-face cards.
const FACE_SEP: &str = " // ";

/// Strip combining marks so `Éomer` and `Eomer` compare equal.
fn deaccent(s: &str) -> String {
    s.nfd().filter(|c| !is_combining(*c)).collect()
}

fn is_combining(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x20D0..=0x20FF)
}

/// Elided outright: they never mark a word boundary in either vocabulary.
///
/// A scan of all 3,353 commander names turns up only five non-alphanumeric
/// characters beyond space, comma and hyphen: `/`, `.`, `&`, `"` and the
/// modifier colon in `Ratonhnhaké꞉ton`. `/`, `&` and `"` behave as separators
/// (`Bebop, Skull & Crossbones` is `bebop-skull-crossbones`); the other two are
/// elided, so this list is complete for the current card pool.
fn is_elided(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '.' | '\u{A789}')
}

/// Normalise a single face: lowercase, unaccented, punctuation removed, runs of
/// anything else collapsed to one space.
fn normalise_face(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for c in deaccent(s).chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        } else if !is_elided(c) {
            pending_space = true;
        }
    }
    out
}

/// Comparison key. Faces are joined by `/` so `Wax // Wane` stays distinct from
/// a hypothetical card called `Wax Wane`.
pub fn key(name: &str) -> String {
    name.split(FACE_SEP)
        .map(normalise_face)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// The front face of a multi-face name, as a comparison key.
///
/// Archidekt writes `Wax // Wane`, EDHREC sometimes writes only `Wax`. Matching
/// falls back to this when the full name misses.
pub fn front_key(name: &str) -> String {
    normalise_face(name.split(FACE_SEP).next().unwrap_or(name))
}

/// EDHREC URL slug for a commander name, keyed off the front face.
pub fn slug(name: &str) -> String {
    front_key(name).replace(' ', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_match_edhrec() {
        assert_eq!(slug("Tasigur, the Golden Fang"), "tasigur-the-golden-fang");
        assert_eq!(slug("Éomer, King of Rohan"), "eomer-king-of-rohan");
        assert_eq!(slug("The Scarab God"), "the-scarab-god");
        assert_eq!(slug("Kroxa, Titan of Death's Hunger"), "kroxa-titan-of-deaths-hunger");
        assert_eq!(slug("Grub, Storied Matriarch // Grub, Notorious Auntie"), "grub-storied-matriarch");
    }

    /// Every one of these was a live 404 before periods were elided; the
    /// right-hand sides are the URLs EDHREC actually serves.
    #[test]
    fn periods_are_elided_not_separated() {
        assert_eq!(slug("M.O.D.O.K."), "modok");
        assert_eq!(slug("Nick Fury, Agent of S.H.I.E.L.D."), "nick-fury-agent-of-shield");
        assert_eq!(slug("Quake, Agent of S.H.I.E.L.D."), "quake-agent-of-shield");
        assert_eq!(slug("Scientist Supreme of A.I.M."), "scientist-supreme-of-aim");
        assert_eq!(slug("U.S.Agent, John Walker"), "usagent-john-walker");
        // U+A789 MODIFIER LETTER COLON, the only such character in the pool.
        assert_eq!(slug("Ratonhnhaké꞉ton"), "ratonhnhaketon");
    }

    /// Ampersands and quotes do break words, unlike periods.
    #[test]
    fn ampersands_and_quotes_separate() {
        assert_eq!(slug("Bebop, Skull & Crossbones"), "bebop-skull-crossbones");
    }

    /// `SP//dr` is a single face whose name contains slashes, so the slashes are
    /// word breaks rather than a face separator.
    #[test]
    fn only_spaced_double_slash_separates_faces() {
        assert_eq!(slug("SP//dr, Piloted by Peni"), "sp-dr-piloted-by-peni");
        assert_eq!(key("SP//dr, Piloted by Peni"), "sp dr piloted by peni");
        assert_eq!(front_key("SP//dr, Piloted by Peni"), "sp dr piloted by peni");

        assert_eq!(key("Wax // Wane"), "wax/wane");
        assert_eq!(front_key("Wax // Wane"), "wax");
    }

    #[test]
    fn keys_ignore_accents_and_punctuation() {
        assert_eq!(key("Éomer, Marshal of Rohan"), "eomer marshal of rohan");
        assert_eq!(key("Ajani's Pridemate"), "ajanis pridemate");
        assert_eq!(key("M.O.D.O.K."), "modok");
    }
}
