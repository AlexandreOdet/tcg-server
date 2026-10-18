//! Écriture du catalogue en base : upsert des cartes puis recalcul des raretés.

use domain::rarity::{assign_rarities, Rarity, RarityShares};
use sqlx::PgPool;

/// Une carte prête à être enregistrée.
#[derive(Debug, Clone)]
pub struct NewCard {
    pub page_id: i64,
    pub title: String,
    pub extract: String,
    pub page_url: String,
    pub image_url: Option<String>,
    pub avg_daily_views: i32,
}

/// Nombre de cartes par rareté après recalcul.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RarityCounts {
    pub common: usize,
    pub rare: usize,
    pub super_rare: usize,
    pub legendary: usize,
}

impl RarityCounts {
    pub fn total(&self) -> usize {
        self.common + self.rare + self.super_rare + self.legendary
    }
}

/// Insère ou met à jour les cartes (clé : langue + id de page), puis recalcule la rareté de
/// **tout** le catalogue : la rareté est relative, elle dépend de l'ensemble des cartes.
/// Le tout dans une seule transaction : en cas d'échec, rien n'est modifié.
pub async fn upsert_and_rerank(
    pool: &PgPool,
    cards: &[NewCard],
    shares: RarityShares,
) -> anyhow::Result<RarityCounts> {
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO cards (lang, wiki_page_id, title, extract, page_url, image_url, \
                            avg_daily_views, rarity)
         SELECT 'fr', t.page_id, t.title, t.extract, t.page_url, t.image_url, t.views, 'common'
         FROM UNNEST($1::bigint[], $2::text[], $3::text[], $4::text[], $5::text[], $6::int[])
              AS t(page_id, title, extract, page_url, image_url, views)
         ON CONFLICT (lang, wiki_page_id) DO UPDATE SET
             title = EXCLUDED.title,
             extract = EXCLUDED.extract,
             page_url = EXCLUDED.page_url,
             image_url = EXCLUDED.image_url,
             avg_daily_views = EXCLUDED.avg_daily_views,
             updated_at = now()",
    )
    .bind(cards.iter().map(|c| c.page_id).collect::<Vec<_>>())
    .bind(cards.iter().map(|c| c.title.as_str()).collect::<Vec<_>>())
    .bind(cards.iter().map(|c| c.extract.as_str()).collect::<Vec<_>>())
    .bind(
        cards
            .iter()
            .map(|c| c.page_url.as_str())
            .collect::<Vec<_>>(),
    )
    .bind(
        cards
            .iter()
            .map(|c| c.image_url.as_deref())
            .collect::<Vec<_>>(),
    )
    .bind(cards.iter().map(|c| c.avg_daily_views).collect::<Vec<_>>())
    .execute(&mut *tx)
    .await?;

    let rows: Vec<(i64, i32)> =
        sqlx::query_as("SELECT id, avg_daily_views FROM cards WHERE lang = 'fr' ORDER BY id")
            .fetch_all(&mut *tx)
            .await?;
    // Les vues sont positives (contrainte CHECK en base).
    let views: Vec<u32> = rows.iter().map(|(_, v)| (*v).max(0) as u32).collect();
    let rarities = assign_rarities(&views, shares);

    sqlx::query(
        "UPDATE cards SET rarity = d.rarity::rarity
         FROM UNNEST($1::bigint[], $2::text[]) AS d(id, rarity)
         WHERE cards.id = d.id",
    )
    .bind(rows.iter().map(|(id, _)| *id).collect::<Vec<_>>())
    .bind(rarities.iter().map(|r| r.as_str()).collect::<Vec<_>>())
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    let mut counts = RarityCounts::default();
    for rarity in rarities {
        match rarity {
            Rarity::Common => counts.common += 1,
            Rarity::Rare => counts.rare += 1,
            Rarity::SuperRare => counts.super_rare += 1,
            Rarity::Legendary => counts.legendary += 1,
        }
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(page_id: i64, views: i32) -> NewCard {
        NewCard {
            page_id,
            title: format!("Article {page_id}"),
            extract: "Texte.".into(),
            page_url: format!("https://fr.wikipedia.org/wiki/Article_{page_id}"),
            image_url: None,
            avg_daily_views: views,
        }
    }

    async fn count_cards(pool: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM cards")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rerunning_the_ingestion_does_not_create_duplicates(pool: PgPool) {
        let cards = [card(1, 10), card(2, 20), card(3, 30)];
        upsert_and_rerank(&pool, &cards, RarityShares::default())
            .await
            .unwrap();
        upsert_and_rerank(&pool, &cards, RarityShares::default())
            .await
            .unwrap();
        assert_eq!(count_cards(&pool).await, 3);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn existing_cards_are_updated(pool: PgPool) {
        upsert_and_rerank(&pool, &[card(1, 10)], RarityShares::default())
            .await
            .unwrap();

        let mut updated = card(1, 99);
        updated.title = "Nouveau titre".into();
        updated.image_url = Some("https://upload.example/x.jpg".into());
        upsert_and_rerank(&pool, &[updated], RarityShares::default())
            .await
            .unwrap();

        let (title, views, image): (String, i32, Option<String>) = sqlx::query_as(
            "SELECT title, avg_daily_views, image_url FROM cards WHERE wiki_page_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(title, "Nouveau titre");
        assert_eq!(views, 99);
        assert_eq!(image.as_deref(), Some("https://upload.example/x.jpg"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_missing_image_is_stored_as_null(pool: PgPool) {
        upsert_and_rerank(&pool, &[card(1, 10)], RarityShares::default())
            .await
            .unwrap();
        let image: Option<String> = sqlx::query_scalar("SELECT image_url FROM cards")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(image, None);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rarities_follow_the_default_distribution(pool: PgPool) {
        let cards: Vec<NewCard> = (1..=100).map(|i| card(i, i as i32)).collect();
        let counts = upsert_and_rerank(&pool, &cards, RarityShares::default())
            .await
            .unwrap();

        assert_eq!(
            counts,
            RarityCounts {
                common: 65,
                rare: 25,
                super_rare: 8,
                legendary: 2
            }
        );
        let legendary: Vec<i32> = sqlx::query_scalar(
            "SELECT avg_daily_views FROM cards WHERE rarity = 'legendary' ORDER BY 1",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(legendary, [99, 100]);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rarity_is_recomputed_over_the_whole_catalogue(pool: PgPool) {
        let first: Vec<NewCard> = (1..=50).map(|i| card(i, i as i32)).collect();
        upsert_and_rerank(&pool, &first, RarityShares::default())
            .await
            .unwrap();

        // Un second lot, beaucoup plus lu, pousse les anciennes cartes vers le bas.
        let second: Vec<NewCard> = (51..=100).map(|i| card(i, 1000 + i as i32)).collect();
        let counts = upsert_and_rerank(&pool, &second, RarityShares::default())
            .await
            .unwrap();

        assert_eq!(counts.total(), 100);
        let old_legendary: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM cards WHERE rarity = 'legendary' AND wiki_page_id <= 50",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(old_legendary, 0);
    }
}
