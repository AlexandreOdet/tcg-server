//! Logique d'accès aux données des boosters : stock et ouverture.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use domain::{
    boosters::{Stock, Tier, TierParams},
    draw::{draw_booster, Catalog, DrawError, DrawRates, Rng},
};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

/// Palier du joueur. Point d'extension du premium : aujourd'hui tout le monde est gratuit.
fn tier_of(_user_id: Uuid) -> Tier {
    Tier::Free
}

#[derive(Debug, Serialize, PartialEq)]
pub struct StockInfo {
    pub available: i32,
    pub max: i32,
    /// Instant du prochain booster, `null` si le stock est plein.
    pub next_at: Option<DateTime<Utc>>,
    pub interval_secs: i64,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CardView {
    pub id: i64,
    pub title: String,
    pub extract: String,
    pub page_url: String,
    pub image_url: Option<String>,
    pub rarity: String,
}

#[derive(Debug, Serialize)]
pub struct OpenedCard {
    #[serde(flatten)]
    pub card: CardView,
    /// `true` si le joueur ne possédait pas encore cette carte avant ce booster.
    pub is_new: bool,
}

#[derive(Debug, Serialize)]
pub struct Opened {
    pub opening_id: Uuid,
    pub cards: Vec<OpenedCard>,
    pub boosters: StockInfo,
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("aucun booster disponible")]
    NoStock,
    #[error("catalogue vide")]
    EmptyCatalog,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl From<DrawError> for OpenError {
    fn from(_: DrawError) -> Self {
        Self::EmptyCatalog
    }
}

fn info(stock: Stock, params: TierParams) -> StockInfo {
    StockInfo {
        available: stock.count,
        max: params.cap,
        next_at: stock
            .next_at(params)
            .and_then(|secs| DateTime::from_timestamp(secs, 0)),
        interval_secs: params.interval_secs,
    }
}

/// Stock courant, sans rien écrire.
pub async fn stock_info(
    pool: &PgPool,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> sqlx::Result<StockInfo> {
    let params = tier_of(user_id).params();
    let row: Option<(i32, DateTime<Utc>)> =
        sqlx::query_as("SELECT count, refill_anchor FROM booster_stock WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    let stock = match row {
        Some((count, anchor)) => Stock::new(count, anchor.timestamp()),
        None => Stock::initial(params, now.timestamp()),
    };
    Ok(info(stock.refill(now.timestamp(), params), params))
}

/// Ouvre un booster en une seule transaction. Le verrou de ligne sur le stock sérialise
/// les appels d'un même joueur : pas de double dépense, et l'idempotence est vérifiée
/// une fois le verrou pris.
pub async fn open(
    pool: &PgPool,
    catalog: &Catalog,
    user_id: Uuid,
    idempotency_key: &str,
    now: DateTime<Utc>,
    rng: Rng<'_>,
) -> Result<Opened, OpenError> {
    let params = tier_of(user_id).params();
    let now_secs = now.timestamp();
    let mut tx = pool.begin().await?;

    let initial = Stock::initial(params, now_secs);
    sqlx::query(
        "INSERT INTO booster_stock (user_id, count, refill_anchor)
         VALUES ($1, $2, to_timestamp($3)) ON CONFLICT (user_id) DO NOTHING",
    )
    .bind(user_id)
    .bind(initial.count)
    .bind(initial.anchor_secs as f64)
    .execute(&mut *tx)
    .await?;

    let (count, anchor): (i32, DateTime<Utc>) = sqlx::query_as(
        "SELECT count, refill_anchor FROM booster_stock WHERE user_id = $1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    let stock = Stock::new(count, anchor.timestamp()).refill(now_secs, params);

    // Rejeu : même clé => même résultat, sans toucher au stock.
    let previous: Option<(Uuid, Vec<i64>, Vec<i64>)> = sqlx::query_as(
        "SELECT id, card_ids, new_card_ids FROM booster_openings
         WHERE user_id = $1 AND idempotency_key = $2",
    )
    .bind(user_id)
    .bind(idempotency_key)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((opening_id, card_ids, new_ids)) = previous {
        tx.rollback().await?;
        let cards = cards_view(pool, &card_ids, &new_ids).await?;
        return Ok(Opened {
            opening_id,
            cards,
            boosters: info(stock, params),
        });
    }

    let stock = stock.consume().ok_or(OpenError::NoStock)?;
    let card_ids = draw_booster(catalog, &DrawRates::default(), rng)?;

    let mut quantities: BTreeMap<i64, i32> = BTreeMap::new();
    for id in &card_ids {
        *quantities.entry(*id).or_default() += 1;
    }
    let distinct: Vec<i64> = quantities.keys().copied().collect();
    let owned: Vec<(i64,)> =
        sqlx::query_as("SELECT card_id FROM user_cards WHERE user_id = $1 AND card_id = ANY($2)")
            .bind(user_id)
            .bind(&distinct)
            .fetch_all(&mut *tx)
            .await?;
    let new_ids: Vec<i64> = distinct
        .iter()
        .copied()
        .filter(|id| !owned.iter().any(|(o,)| o == id))
        .collect();
    let amounts: Vec<i32> = distinct.iter().map(|id| quantities[id]).collect();

    sqlx::query(
        "INSERT INTO user_cards (user_id, card_id, quantity)
         SELECT $1, card_id, quantity FROM UNNEST($2::bigint[], $3::int[]) AS t(card_id, quantity)
         ON CONFLICT (user_id, card_id) DO UPDATE SET quantity = user_cards.quantity + EXCLUDED.quantity",
    )
    .bind(user_id)
    .bind(&distinct)
    .bind(&amounts)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "UPDATE booster_stock SET count = $2, refill_anchor = to_timestamp($3) WHERE user_id = $1",
    )
    .bind(user_id)
    .bind(stock.count)
    .bind(stock.anchor_secs as f64)
    .execute(&mut *tx)
    .await?;

    let (opening_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO booster_openings (user_id, idempotency_key, card_ids, new_card_ids)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(user_id)
    .bind(idempotency_key)
    .bind(&card_ids)
    .bind(&new_ids)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    let cards = cards_view(pool, &card_ids, &new_ids).await?;
    Ok(Opened {
        opening_id,
        cards,
        boosters: info(stock, params),
    })
}

/// Détail des cartes, dans l'ordre du tirage (doublons conservés).
async fn cards_view(
    pool: &PgPool,
    card_ids: &[i64],
    new_ids: &[i64],
) -> sqlx::Result<Vec<OpenedCard>> {
    let rows: Vec<CardView> = sqlx::query_as(
        "SELECT id, title, extract, page_url, image_url, rarity::text AS rarity
         FROM cards WHERE id = ANY($1)",
    )
    .bind(card_ids)
    .fetch_all(pool)
    .await?;
    let by_id: BTreeMap<i64, &CardView> = rows.iter().map(|c| (c.id, c)).collect();
    let mut shown_new: Vec<i64> = Vec::new();
    let mut result = Vec::with_capacity(card_ids.len());
    for id in card_ids {
        let Some(card) = by_id.get(id) else { continue };
        // Un doublon dans le même booster n'est « nouveau » qu'à sa première apparition.
        let is_new = new_ids.contains(id) && !shown_new.contains(id);
        if is_new {
            shown_new.push(*id);
        }
        result.push(OpenedCard {
            card: CardView {
                id: card.id,
                title: card.title.clone(),
                extract: card.extract.clone(),
                page_url: card.page_url.clone(),
                image_url: card.image_url.clone(),
                rarity: card.rarity.clone(),
            },
            is_new,
        });
    }
    Ok(result)
}
