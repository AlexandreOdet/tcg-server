//! Stock de boosters « paresseux » : aucun cron, le stock est recalculé à la demande
//! à partir de (nombre en stock, instant de référence du minuteur).
//!
//! Tout est exprimé en secondes Unix pour que ce module reste indépendant de toute
//! bibliothèque de dates.

/// Palier d'un joueur. Aujourd'hui seul le palier gratuit existe ; le premium s'ajoutera ici.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Free,
}

/// Paramètres de recharge d'un palier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierParams {
    /// Délai entre deux boosters gagnés.
    pub interval_secs: i64,
    /// Stock maximal : au-delà, le minuteur est suspendu.
    pub cap: i32,
    /// Stock offert à la création du compte.
    pub initial: i32,
}

impl Tier {
    pub const fn params(self) -> TierParams {
        match self {
            Self::Free => TierParams {
                interval_secs: 10 * 60,
                cap: 10,
                initial: 3,
            },
        }
    }
}

/// État stocké : `anchor_secs` est l'instant à partir duquel le prochain booster se charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stock {
    pub count: i32,
    pub anchor_secs: i64,
}

impl Stock {
    pub fn new(count: i32, anchor_secs: i64) -> Self {
        Self { count, anchor_secs }
    }

    /// Stock d'un nouveau joueur.
    pub fn initial(params: TierParams, now_secs: i64) -> Self {
        Self::new(params.initial, now_secs)
    }

    /// Applique le temps écoulé. Gère l'horloge qui recule (ancre dans le futur) et un stock
    /// déjà au-dessus du plafond (ancien palier supérieur) : il n'est jamais réduit.
    pub fn refill(self, now_secs: i64, params: TierParams) -> Self {
        let anchor = self.anchor_secs.min(now_secs);
        if self.count >= params.cap {
            return Self::new(self.count, now_secs);
        }
        let gained = (now_secs - anchor) / params.interval_secs;
        let missing = i64::from(params.cap - self.count);
        if gained >= missing {
            Self::new(params.cap, now_secs)
        } else {
            // `gained < missing <= cap` : la conversion ne peut pas échouer.
            Self::new(
                self.count + gained as i32,
                anchor + gained * params.interval_secs,
            )
        }
    }

    /// Retire un booster d'un stock **déjà rechargé**. `None` s'il est vide.
    /// Le minuteur repart de `now_secs` quand le stock était plein (il était suspendu).
    pub fn consume(self) -> Option<Self> {
        (self.count > 0).then(|| Self::new(self.count - 1, self.anchor_secs))
    }

    /// Instant du prochain booster, `None` si le stock est plein.
    pub fn next_at(self, params: TierParams) -> Option<i64> {
        (self.count < params.cap).then(|| self.anchor_secs + params.interval_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: TierParams = TierParams {
        interval_secs: 600,
        cap: 10,
        initial: 3,
    };

    #[test]
    fn nothing_gained_before_one_interval() {
        let s = Stock::new(2, 1000).refill(1599, P);
        assert_eq!(s, Stock::new(2, 1000));
        assert_eq!(s.next_at(P), Some(1600));
    }

    #[test]
    fn gains_whole_intervals_and_keeps_the_remainder() {
        // 2 intervalles et 100 s écoulés : +2, le minuteur garde les 100 s.
        let s = Stock::new(2, 1000).refill(1000 + 1300, P);
        assert_eq!(s, Stock::new(4, 2200));
        assert_eq!(s.next_at(P), Some(2800));
    }

    #[test]
    fn caps_at_the_maximum_and_suspends_the_timer() {
        let s = Stock::new(8, 0).refill(1_000_000, P);
        assert_eq!(s, Stock::new(10, 1_000_000));
        assert_eq!(s.next_at(P), None);
    }

    #[test]
    fn exactly_reaching_the_cap_is_a_full_stock() {
        let s = Stock::new(8, 0).refill(1200, P);
        assert_eq!(s.count, 10);
        assert_eq!(s.next_at(P), None);
    }

    #[test]
    fn timer_restarts_at_consumption_when_the_stock_was_full() {
        let full = Stock::new(10, 0).refill(5000, P);
        let after = full.consume().unwrap();
        assert_eq!(after, Stock::new(9, 5000));
        assert_eq!(after.next_at(P), Some(5600));
    }

    #[test]
    fn consuming_does_not_reset_a_running_timer() {
        let s = Stock::new(5, 1000).refill(1300, P).consume().unwrap();
        assert_eq!(s, Stock::new(4, 1000));
    }

    #[test]
    fn empty_stock_cannot_be_consumed() {
        assert_eq!(Stock::new(0, 0).consume(), None);
    }

    #[test]
    fn clock_going_backwards_never_grants_or_removes_boosters() {
        let s = Stock::new(4, 10_000).refill(5_000, P);
        assert_eq!(s, Stock::new(4, 5_000));
        // Et le minuteur repart de l'instant courant, pas d'un futur lointain.
        assert_eq!(s.next_at(P), Some(5_600));
    }

    #[test]
    fn stock_above_the_cap_is_kept() {
        let s = Stock::new(25, 0).refill(10_000, P);
        assert_eq!(s.count, 25);
        assert_eq!(s.next_at(P), None);
    }

    #[test]
    fn free_tier_params() {
        let p = Tier::Free.params();
        assert_eq!((p.interval_secs, p.cap, p.initial), (600, 10, 3));
    }
}
