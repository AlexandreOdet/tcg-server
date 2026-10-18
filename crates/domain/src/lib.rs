//! Règles métier pures : aucune dépendance web ni base de données.
//!
//! - `rarity`   : calcul de la rareté à partir des vues Wikipédia
//! - `boosters` : stock paresseux et paramètres par palier
//! - `draw`     : tirage pondéré avec générateur aléatoire injectable

pub mod boosters;
pub mod draw;
pub mod rarity;
