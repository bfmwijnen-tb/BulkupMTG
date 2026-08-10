//! Matching a collection against every crawled commander.
//!
//! The headline number is *not* "how many of the top 100 do you own". That
//! measure is dominated by colourless staples — Arcane Signet, Lightning
//! Greaves, Cultivate, Command Tower — which count for every commander in the
//! right colours, and it also rewards wide colour identities simply because a
//! five-colour deck can legally use more of any given box.
//!
//! So the ranking runs on `synergy_score`: the sum of EDHREC's synergy figure
//! over the cards you own. Staples contribute ~0 by construction, leaving the
//! cards that actually point at this commander. The raw top-100 count is still
//! reported, because it is what the threshold filters on, and `staple_hits`
//! shows how much of that count is generic filler.

use std::collections::HashMap;

use serde::Serialize;

use crate::bulk::Collection;
use crate::edhrec::CommanderData;
use crate::names;
use crate::scryfall::ScryfallIndex;

/// Cards at or below this synergy are treated as generic staples.
const STAPLE_SYNERGY: f64 = 0.02;

/// How many of the ranked list counts as "the top cards" for the threshold.
pub const TOP_N: usize = 100;

#[derive(Debug, Clone, Serialize)]
pub struct CommanderMatch {
    pub slug: String,
    pub name: String,
    pub color_identity: Vec<String>,
    pub image_small: Option<String>,
    pub image_normal: Option<String>,
    pub scryfall_uri: String,
    pub deck_count: u32,

    /// Owned cards inside the commander's top 100 — the number the threshold uses.
    pub top_hits: usize,
    /// Owned cards anywhere in EDHREC's ~260-card list.
    pub total_hits: usize,
    /// Of `top_hits`, how many are generic staples rather than real payoffs.
    pub staple_hits: usize,
    /// Sum of positive synergy over owned cards. The ranking key.
    pub synergy_score: f64,
    /// Owned share of EDHREC's average decklist.
    pub avg_deck_owned: usize,
    pub avg_deck_total: usize,

    pub owns_commander: bool,
    /// How many of the top-100 hits are findable in each uploaded file, so you
    /// know which box to open. A card held in two files counts in both, so these
    /// need not sum to `top_hits`.
    pub by_source: HashMap<String, usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CardRow {
    pub name: String,
    pub owned: bool,
    pub qty: u32,
    pub synergy: f64,
    pub inclusion: f64,
    pub rank: usize,
    pub in_top: bool,
    pub in_avg_deck: bool,
    pub image_small: Option<String>,
    pub image_normal: Option<String>,
    pub scryfall_uri: Option<String>,
    pub type_line: Option<String>,
    pub sources: Vec<(String, u32)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommanderDetail {
    pub summary: CommanderMatch,
    pub cards: Vec<CardRow>,
}

fn owned_lookup<'a>(
    collection: &'a Collection,
    name: &str,
) -> Option<&'a crate::bulk::OwnedCard> {
    collection
        .get(&names::key(name))
        .or_else(|| collection.get(&names::front_key(name)))
}

pub fn score_commander(
    data: &CommanderData,
    collection: &Collection,
    scryfall: &ScryfallIndex,
) -> CommanderMatch {
    let info = scryfall.lookup(&data.name);

    let mut top_hits = 0;
    let mut total_hits = 0;
    let mut staple_hits = 0;
    let mut synergy_score = 0.0;
    let mut by_source: HashMap<String, usize> = HashMap::new();

    for (rank, card) in data.cards.iter().enumerate() {
        let Some(owned) = owned_lookup(collection, &card.name) else {
            continue;
        };
        total_hits += 1;
        // Negative synergy means the card is *under*-represented here relative to
        // its colours; it should not drag a commander down, just not help.
        synergy_score += card.synergy.max(0.0);

        if rank < TOP_N {
            top_hits += 1;
            if card.synergy <= STAPLE_SYNERGY {
                staple_hits += 1;
            }
            for src in owned.sources.keys() {
                *by_source.entry(src.clone()).or_insert(0) += 1;
            }
        }
    }

    let avg_deck_owned = data
        .avg_deck
        .iter()
        .filter(|n| owned_lookup(collection, n).is_some())
        .count();

    CommanderMatch {
        slug: data.slug.clone(),
        name: data.name.clone(),
        color_identity: info.map(|i| i.color_identity.clone()).unwrap_or_default(),
        image_small: info.and_then(|i| i.image_small.clone()),
        image_normal: info.and_then(|i| i.image_normal.clone()),
        scryfall_uri: info.map(|i| i.scryfall_uri.clone()).unwrap_or_default(),
        deck_count: data.deck_count,
        top_hits,
        total_hits,
        staple_hits,
        synergy_score,
        avg_deck_owned,
        avg_deck_total: data.avg_deck.len(),
        owns_commander: owned_lookup(collection, &data.name).is_some(),
        by_source,
    }
}

pub fn detail(
    data: &CommanderData,
    collection: &Collection,
    scryfall: &ScryfallIndex,
) -> CommanderDetail {
    let summary = score_commander(data, collection, scryfall);
    let avg: std::collections::HashSet<&str> =
        data.avg_deck.iter().map(String::as_str).collect();

    let cards = data
        .cards
        .iter()
        .enumerate()
        .map(|(rank, card)| {
            let owned = owned_lookup(collection, &card.name);
            let info = scryfall.lookup(&card.name);
            let mut sources: Vec<(String, u32)> = owned
                .map(|o| o.sources.iter().map(|(k, v)| (k.clone(), *v)).collect())
                .unwrap_or_default();
            sources.sort();
            CardRow {
                name: card.name.clone(),
                owned: owned.is_some(),
                qty: owned.map(|o| o.qty).unwrap_or(0),
                synergy: card.synergy,
                inclusion: card.inclusion(),
                rank: rank + 1,
                in_top: rank < TOP_N,
                in_avg_deck: avg.contains(card.name.as_str()),
                image_small: info.and_then(|i| i.image_small.clone()),
                image_normal: info.and_then(|i| i.image_normal.clone()),
                scryfall_uri: info.map(|i| i.scryfall_uri.clone()),
                type_line: info.map(|i| i.type_line.clone()),
                sources,
            }
        })
        .collect();

    CommanderDetail { summary, cards }
}

pub struct RankOptions {
    pub min_top_hits: usize,
    pub sort: Sort,
    pub owned_commander_only: bool,
    pub colors: Option<Vec<String>>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Sort {
    Synergy,
    TopHits,
    AvgDeck,
}

impl Sort {
    pub fn parse(s: &str) -> Self {
        match s {
            "hits" => Sort::TopHits,
            "avgdeck" => Sort::AvgDeck,
            _ => Sort::Synergy,
        }
    }
}

pub fn rank(
    all: &HashMap<String, CommanderData>,
    collection: &Collection,
    scryfall: &ScryfallIndex,
    opts: &RankOptions,
) -> Vec<CommanderMatch> {
    let mut out: Vec<CommanderMatch> = all
        .values()
        .map(|d| score_commander(d, collection, scryfall))
        .filter(|m| m.top_hits >= opts.min_top_hits)
        .filter(|m| !opts.owned_commander_only || m.owns_commander)
        .filter(|m| match &opts.colors {
            // Keep commanders whose identity fits inside the chosen colours.
            Some(allowed) => m.color_identity.iter().all(|c| allowed.contains(c)),
            None => true,
        })
        .collect();

    out.sort_by(|a, b| {
        let ord = match opts.sort {
            Sort::Synergy => b
                .synergy_score
                .partial_cmp(&a.synergy_score)
                .unwrap_or(std::cmp::Ordering::Equal),
            Sort::TopHits => b.top_hits.cmp(&a.top_hits),
            Sort::AvgDeck => b.avg_deck_owned.cmp(&a.avg_deck_owned),
        };
        ord.then_with(|| {
            b.synergy_score
                .partial_cmp(&a.synergy_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| a.name.cmp(&b.name))
    });

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edhrec::EdhCard;

    fn card(name: &str, synergy: f64, num: u32) -> EdhCard {
        EdhCard { name: name.into(), synergy, num_decks: num, potential_decks: 1000 }
    }

    #[test]
    fn staples_do_not_drive_the_ranking() {
        let mut collection = Collection::default();
        collection.add_file("bulk.txt", "1 Arcane Signet (TDC) 105\n1 Ashnod's Altar (CMM) 1\n");
        let scryfall = ScryfallIndex::default();

        // A commander whose overlap is one staple, versus one whose overlap is a
        // genuine payoff. Equal hit counts; synergy must separate them.
        let staple_only = CommanderData {
            slug: "a".into(), name: "A".into(),
            cards: vec![card("Arcane Signet", 0.001, 900)],
            avg_deck: vec![], deck_count: 1000, fetched_at: String::new(),
        };
        let payoff = CommanderData {
            slug: "b".into(), name: "B".into(),
            cards: vec![card("Ashnod's Altar", 0.42, 600)],
            avg_deck: vec![], deck_count: 1000, fetched_at: String::new(),
        };

        let a = score_commander(&staple_only, &collection, &scryfall);
        let b = score_commander(&payoff, &collection, &scryfall);

        assert_eq!(a.top_hits, 1);
        assert_eq!(b.top_hits, 1);
        assert_eq!(a.staple_hits, 1);
        assert_eq!(b.staple_hits, 0);
        assert!(b.synergy_score > a.synergy_score);
    }

    #[test]
    fn negative_synergy_never_subtracts() {
        let mut collection = Collection::default();
        collection.add_file("bulk.txt", "1 Sol Ring (LEA) 1\n");
        let scryfall = ScryfallIndex::default();
        let data = CommanderData {
            slug: "c".into(), name: "C".into(),
            cards: vec![card("Sol Ring", -0.3, 950)],
            avg_deck: vec![], deck_count: 1000, fetched_at: String::new(),
        };
        let m = score_commander(&data, &collection, &scryfall);
        assert_eq!(m.synergy_score, 0.0);
    }

    #[test]
    fn tracks_which_file_each_hit_came_from() {
        let mut collection = Collection::default();
        collection.add_file("box_a.txt", "1 Ashnod's Altar (CMM) 1\n");
        collection.add_file("box_b.txt", "1 Grave Pact (CMM) 2\n");
        let scryfall = ScryfallIndex::default();
        let data = CommanderData {
            slug: "d".into(), name: "D".into(),
            cards: vec![card("Ashnod's Altar", 0.4, 600), card("Grave Pact", 0.3, 500)],
            avg_deck: vec![], deck_count: 1000, fetched_at: String::new(),
        };
        let m = score_commander(&data, &collection, &scryfall);
        assert_eq!(m.by_source["box_a.txt"], 1);
        assert_eq!(m.by_source["box_b.txt"], 1);
    }
}
