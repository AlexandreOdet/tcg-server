//! Tirage d'un booster : probabilités par rareté, carte garantie, générateur injectable.

use crate::rarity::Rarity;

pub const CARDS_PER_BOOSTER: usize = 5;

/// Poids de tirage par carte, en pour mille. Seuls les rapports comptent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawRates {
    pub legendary: u32,
    pub super_rare: u32,
    pub rare: u32,
    pub common: u32,
}

impl Default for DrawRates {
    /// 1 % légendaire, 4 % super rare, 20 % rare, 75 % commune par emplacement.
    fn default() -> Self {
        Self {
            legendary: 10,
            super_rare: 40,
            rare: 200,
            common: 750,
        }
    }
}

// Ordre de parcours du tirage : de la plus rare à la plus commune.
const ORDER: [Rarity; 4] = [
    Rarity::Legendary,
    Rarity::SuperRare,
    Rarity::Rare,
    Rarity::Common,
];

fn slot(rarity: Rarity) -> usize {
    ORDER
        .iter()
        .position(|r| *r == rarity)
        .expect("rareté connue")
}

impl DrawRates {
    fn weight(&self, rarity: Rarity) -> u32 {
        match rarity {
            Rarity::Legendary => self.legendary,
            Rarity::SuperRare => self.super_rare,
            Rarity::Rare => self.rare,
            Rarity::Common => self.common,
        }
    }
}

/// Cartes du catalogue, classées par rareté.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pools: [Vec<i64>; 4],
}

impl Catalog {
    pub fn new(entries: impl IntoIterator<Item = (Rarity, i64)>) -> Self {
        let mut catalog = Self::default();
        for (rarity, id) in entries {
            catalog.pools[slot(rarity)].push(id);
        }
        catalog
    }

    pub fn is_empty(&self) -> bool {
        self.pools.iter().all(Vec::is_empty)
    }

    pub fn count(&self, rarity: Rarity) -> usize {
        self.pools[slot(rarity)].len()
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DrawError {
    #[error("le catalogue est vide")]
    EmptyCatalog,
}

/// Source d'aléa : `rng(n)` renvoie un entier uniforme dans `0..n` (`n > 0`).
pub type Rng<'a> = &'a mut (dyn FnMut(u32) -> u32 + Send);

/// Tire `CARDS_PER_BOOSTER` cartes (doublons possibles). Le dernier emplacement est
/// garanti « rare ou mieux ». Une rareté sans carte au catalogue est simplement ignorée
/// (les poids des autres sont conservés), de sorte qu'un petit catalogue reste utilisable.
pub fn draw_booster(
    catalog: &Catalog,
    rates: &DrawRates,
    rng: Rng<'_>,
) -> Result<Vec<i64>, DrawError> {
    if catalog.is_empty() {
        return Err(DrawError::EmptyCatalog);
    }
    let mut cards = Vec::with_capacity(CARDS_PER_BOOSTER);
    for index in 0..CARDS_PER_BOOSTER {
        let guaranteed = index == CARDS_PER_BOOSTER - 1;
        let rarity = pick_rarity(catalog, rates, guaranteed, rng);
        let pool = &catalog.pools[slot(rarity)];
        cards.push(pool[rng(pool.len() as u32) as usize]);
    }
    Ok(cards)
}

fn pick_rarity(catalog: &Catalog, rates: &DrawRates, guaranteed: bool, rng: Rng<'_>) -> Rarity {
    let weights = |only_rare_plus: bool| -> Vec<(Rarity, u32)> {
        ORDER
            .iter()
            .filter(|r| !(only_rare_plus && **r == Rarity::Common))
            .filter(|r| catalog.count(**r) > 0)
            .map(|r| (*r, rates.weight(*r)))
            .filter(|(_, w)| *w > 0)
            .collect()
    };
    let mut candidates = weights(guaranteed);
    if candidates.is_empty() {
        // Pas de rare+ disponible (ou poids nuls) : on retombe sur tout ce qui existe.
        candidates = ORDER
            .iter()
            .filter(|r| catalog.count(**r) > 0)
            .map(|r| (*r, rates.weight(*r).max(1)))
            .collect();
    }
    let total: u32 = candidates.iter().map(|(_, w)| w).sum();
    let mut roll = rng(total);
    for (rarity, weight) in &candidates {
        if roll < *weight {
            return *rarity;
        }
        roll -= weight;
    }
    candidates.last().expect("au moins un candidat").0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Générateur déterministe (xorshift64*), suffisant pour des statistiques de test.
    fn seeded(seed: u64) -> impl FnMut(u32) -> u32 {
        let mut state = seed;
        move |n| {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let x = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
            ((x >> 32) % u64::from(n)) as u32
        }
    }

    /// 100 communes (ids 0..100), 30 rares (100..130), 8 super rares, 2 légendaires.
    fn catalog() -> Catalog {
        let spec = [
            (Rarity::Common, 100),
            (Rarity::Rare, 30),
            (Rarity::SuperRare, 8),
            (Rarity::Legendary, 2),
        ];
        let mut id = 0;
        let mut entries = Vec::new();
        for (rarity, n) in spec {
            for _ in 0..n {
                entries.push((rarity, id));
                id += 1;
            }
        }
        Catalog::new(entries)
    }

    fn rarity_of(id: i64) -> Rarity {
        match id {
            0..=99 => Rarity::Common,
            100..=129 => Rarity::Rare,
            130..=137 => Rarity::SuperRare,
            _ => Rarity::Legendary,
        }
    }

    #[test]
    fn a_booster_has_five_cards_from_the_catalog() {
        let mut rng = seeded(1);
        let cards = draw_booster(&catalog(), &DrawRates::default(), &mut rng).unwrap();
        assert_eq!(cards.len(), CARDS_PER_BOOSTER);
        assert!(cards.iter().all(|id| (0..140).contains(id)));
    }

    #[test]
    fn every_booster_has_at_least_one_rare_or_better() {
        let mut rng = seeded(7);
        for _ in 0..20_000 {
            let cards = draw_booster(&catalog(), &DrawRates::default(), &mut rng).unwrap();
            assert!(cards.iter().any(|id| rarity_of(*id) != Rarity::Common));
        }
    }

    #[test]
    fn frequencies_match_the_configured_rates() {
        let mut rng = seeded(42);
        let rates = DrawRates::default();
        let boosters = 100_000;
        let mut counts = [0u32; 4]; // légendaire, super rare, rare, commune
                                    // Les 4 premiers emplacements suivent exactement les taux ; le 5e est garanti.
        for _ in 0..boosters {
            let cards = draw_booster(&catalog(), &rates, &mut rng).unwrap();
            for id in &cards[..CARDS_PER_BOOSTER - 1] {
                counts[slot(rarity_of(*id))] += 1;
            }
        }
        let total = f64::from(boosters * (CARDS_PER_BOOSTER as u32 - 1));
        let expected = [0.010, 0.040, 0.200, 0.750];
        for (observed, expected) in counts.iter().zip(expected) {
            let share = f64::from(*observed) / total;
            assert!(
                (share - expected).abs() < 0.004,
                "part observée {share:.4}, attendue {expected}"
            );
        }
    }

    #[test]
    fn guaranteed_slot_follows_relative_weights_of_rare_and_above() {
        let mut rng = seeded(5);
        let mut counts = [0u32; 4];
        let n = 100_000;
        for _ in 0..n {
            let cards = draw_booster(&catalog(), &DrawRates::default(), &mut rng).unwrap();
            counts[slot(rarity_of(cards[CARDS_PER_BOOSTER - 1]))] += 1;
        }
        assert_eq!(counts[3], 0, "jamais de commune sur l'emplacement garanti");
        // rare+ = 10 + 40 + 200 = 250 : légendaire 4 %, super rare 16 %, rare 80 %.
        let share = |i: usize| f64::from(counts[i]) / f64::from(n);
        assert!((share(0) - 0.04).abs() < 0.005);
        assert!((share(1) - 0.16).abs() < 0.007);
        assert!((share(2) - 0.80).abs() < 0.008);
    }

    #[test]
    fn same_seed_gives_same_booster() {
        let a = draw_booster(&catalog(), &DrawRates::default(), &mut seeded(9)).unwrap();
        let b = draw_booster(&catalog(), &DrawRates::default(), &mut seeded(9)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn empty_catalog_is_an_error() {
        let mut rng = seeded(1);
        assert_eq!(
            draw_booster(&Catalog::default(), &DrawRates::default(), &mut rng),
            Err(DrawError::EmptyCatalog)
        );
    }

    #[test]
    fn catalog_without_rare_cards_still_works() {
        let only_common = Catalog::new((0..10).map(|id| (Rarity::Common, id)));
        let mut rng = seeded(3);
        let cards = draw_booster(&only_common, &DrawRates::default(), &mut rng).unwrap();
        assert_eq!(cards.len(), CARDS_PER_BOOSTER);
    }

    #[test]
    fn missing_rarity_is_skipped_without_panicking() {
        // Seulement des légendaires : tous les emplacements en tirent.
        let only_legendary = Catalog::new([(Rarity::Legendary, 1), (Rarity::Legendary, 2)]);
        let mut rng = seeded(3);
        for _ in 0..1000 {
            let cards = draw_booster(&only_legendary, &DrawRates::default(), &mut rng).unwrap();
            assert!(cards.iter().all(|id| *id == 1 || *id == 2));
        }
    }
}
