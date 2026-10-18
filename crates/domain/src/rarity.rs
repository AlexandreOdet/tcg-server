//! Rareté d'une carte, déduite de la fréquentation de l'article Wikipédia.
//!
//! Règle : **plus un article est lu, plus la carte est rare**. La rareté est relative au
//! catalogue (quantiles), pas à des seuils absolus : elle reste cohérente quand les
//! audiences de Wikipédia évoluent.

use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rarity {
    Common,
    Rare,
    SuperRare,
    Legendary,
}

impl Rarity {
    /// Valeur stockée en base (type enum Postgres `rarity`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Common => "common",
            Self::Rare => "rare",
            Self::SuperRare => "super_rare",
            Self::Legendary => "legendary",
        }
    }
}

impl FromStr for Rarity {
    type Err = InvalidRarity;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "common" => Ok(Self::Common),
            "rare" => Ok(Self::Rare),
            "super_rare" => Ok(Self::SuperRare),
            "legendary" => Ok(Self::Legendary),
            other => Err(InvalidRarity(other.to_owned())),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("rareté inconnue : {0}")]
pub struct InvalidRarity(String);

/// Part du catalogue occupée par chaque rareté (le reste est `Common`).
/// Exemple : 2 % de légendaires, 8 % de super rares, 25 % de rares, 65 % de communes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RarityShares {
    legendary: f64,
    super_rare: f64,
    rare: f64,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum InvalidShares {
    #[error("chaque part doit être comprise entre 0 et 1")]
    OutOfRange,
    #[error("la somme des parts ne doit pas dépasser 1")]
    SumTooLarge,
}

impl RarityShares {
    pub fn new(legendary: f64, super_rare: f64, rare: f64) -> Result<Self, InvalidShares> {
        let parts = [legendary, super_rare, rare];
        if parts.iter().any(|p| !(0.0..=1.0).contains(p)) {
            return Err(InvalidShares::OutOfRange);
        }
        if parts.iter().sum::<f64>() > 1.0 + f64::EPSILON {
            return Err(InvalidShares::SumTooLarge);
        }
        Ok(Self {
            legendary,
            super_rare,
            rare,
        })
    }
}

impl Default for RarityShares {
    fn default() -> Self {
        Self {
            legendary: 0.02,
            super_rare: 0.08,
            rare: 0.25,
        }
    }
}

/// Attribue une rareté à chaque article à partir de ses vues moyennes.
/// Le résultat est dans le même ordre que `views`.
///
/// Les ex æquo reçoivent toujours la même rareté : si plusieurs articles partagent
/// la valeur-seuil, la catégorie supérieure peut donc dépasser sa part théorique
/// (cas extrême : tous les articles ont les mêmes vues, ils sont tous `Legendary`).
pub fn assign_rarities(views: &[u32], shares: RarityShares) -> Vec<Rarity> {
    let n = views.len();
    let mut sorted = views.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));

    // Valeur-seuil des vues pour entrer dans une catégorie, ou `None` si elle est vide.
    let cutoff = |cumulative_share: f64| -> Option<u32> {
        let count = ((n as f64) * cumulative_share).round() as usize;
        (count > 0).then(|| sorted[count.min(n) - 1])
    };
    let legendary = cutoff(shares.legendary);
    let super_rare = cutoff(shares.legendary + shares.super_rare);
    let rare = cutoff(shares.legendary + shares.super_rare + shares.rare);

    let reaches = |cutoff: Option<u32>, v: u32| cutoff.is_some_and(|c| v >= c);
    views
        .iter()
        .map(|&v| {
            if reaches(legendary, v) {
                Rarity::Legendary
            } else if reaches(super_rare, v) {
                Rarity::SuperRare
            } else if reaches(rare, v) {
                Rarity::Rare
            } else {
                Rarity::Common
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(rarities: &[Rarity], wanted: Rarity) -> usize {
        rarities.iter().filter(|&&r| r == wanted).count()
    }

    #[test]
    fn empty_catalogue_gives_empty_result() {
        assert!(assign_rarities(&[], RarityShares::default()).is_empty());
    }

    #[test]
    fn default_shares_split_a_catalogue_as_expected() {
        // 100 articles aux vues toutes différentes : 1..=100.
        let views: Vec<u32> = (1..=100).collect();
        let rarities = assign_rarities(&views, RarityShares::default());

        assert_eq!(count(&rarities, Rarity::Legendary), 2);
        assert_eq!(count(&rarities, Rarity::SuperRare), 8);
        assert_eq!(count(&rarities, Rarity::Rare), 25);
        assert_eq!(count(&rarities, Rarity::Common), 65);
    }

    #[test]
    fn more_views_means_rarer() {
        let views: Vec<u32> = (1..=100).collect();
        let rarities = assign_rarities(&views, RarityShares::default());

        assert_eq!(rarities[99], Rarity::Legendary); // 100 vues
        assert_eq!(rarities[0], Rarity::Common); // 1 vue
    }

    #[test]
    fn result_follows_the_input_order() {
        let rarities =
            assign_rarities(&[1, 100, 1, 100], RarityShares::new(0.5, 0.0, 0.0).unwrap());
        assert_eq!(
            rarities,
            [
                Rarity::Common,
                Rarity::Legendary,
                Rarity::Common,
                Rarity::Legendary
            ]
        );
    }

    #[test]
    fn ties_always_share_the_same_rarity() {
        // Le seuil légendaire (2 sur 10) tombe au milieu d'un lot d'ex æquo.
        let views = [50, 50, 50, 10, 10, 10, 10, 10, 10, 10];
        let rarities = assign_rarities(&views, RarityShares::new(0.2, 0.0, 0.0).unwrap());
        assert!(rarities[..3].iter().all(|&r| r == Rarity::Legendary));
        assert!(rarities[3..].iter().all(|&r| r == Rarity::Common));
    }

    #[test]
    fn empty_categories_stay_empty() {
        let views: Vec<u32> = (1..=10).collect();
        let rarities = assign_rarities(&views, RarityShares::new(0.0, 0.0, 0.0).unwrap());
        assert!(rarities.iter().all(|&r| r == Rarity::Common));
    }

    #[test]
    fn invalid_shares_are_rejected() {
        assert_eq!(
            RarityShares::new(-0.1, 0.1, 0.1),
            Err(InvalidShares::OutOfRange)
        );
        assert_eq!(
            RarityShares::new(1.5, 0.0, 0.0),
            Err(InvalidShares::OutOfRange)
        );
        assert_eq!(
            RarityShares::new(0.5, 0.4, 0.2),
            Err(InvalidShares::SumTooLarge)
        );
    }

    #[test]
    fn rarity_round_trips_through_its_database_value() {
        for r in [
            Rarity::Common,
            Rarity::Rare,
            Rarity::SuperRare,
            Rarity::Legendary,
        ] {
            assert_eq!(r.as_str().parse::<Rarity>(), Ok(r));
        }
        assert!("mythique".parse::<Rarity>().is_err());
    }
}
