//! Règles métier pures : aucune dépendance web ni base de données.
//!
//! Modules prévus (voir docs/PLAN.md) :
//! - `rarity`   : calcul de la rareté à partir des vues Wikipédia (phase 1)
//! - `boosters` : stock paresseux et paramètres par palier (phase 3)
//! - `draw`     : tirage pondéré avec générateur aléatoire injectable (phase 3)

pub mod rarity;
