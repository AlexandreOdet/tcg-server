//! Catalogue de tirage gardé en mémoire (ids par rareté), rechargé périodiquement :
//! l'ingestion tourne dans un autre processus et peut ajouter des cartes à tout moment.

use std::{
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use domain::{draw::Catalog, rarity::Rarity};
use sqlx::PgPool;

const TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
pub struct CatalogCache {
    inner: RwLock<Option<(Instant, Arc<Catalog>)>>,
}

impl CatalogCache {
    /// Renvoie le catalogue (rechargé s'il est périmé). Un catalogue vide n'est pas mis en cache.
    pub async fn get(&self, pool: &PgPool) -> Result<Arc<Catalog>, sqlx::Error> {
        if let Some((loaded_at, catalog)) = self.inner.read().expect("verrou sain").as_ref() {
            if loaded_at.elapsed() < TTL {
                return Ok(Arc::clone(catalog));
            }
        }
        let catalog = Arc::new(load(pool).await?);
        if !catalog.is_empty() {
            *self.inner.write().expect("verrou sain") =
                Some((Instant::now(), Arc::clone(&catalog)));
        }
        Ok(catalog)
    }
}

async fn load(pool: &PgPool) -> Result<Catalog, sqlx::Error> {
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, rarity::text FROM cards")
        .fetch_all(pool)
        .await?;
    Ok(Catalog::new(rows.into_iter().filter_map(|(id, rarity)| {
        // Une rareté inconnue ne devrait pas exister (enum Postgres) ; on l'ignore plutôt que de planter.
        rarity.parse::<Rarity>().ok().map(|r| (r, id))
    })))
}
