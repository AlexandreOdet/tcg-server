use std::{collections::HashMap, sync::Arc};

use sqlx::PgPool;

use crate::{
    auth::{tokens::TokenService, verifier::TokenVerifier},
    boosters::catalog::CatalogCache,
};

pub type Verifiers = HashMap<String, Arc<dyn TokenVerifier>>;

/// État partagé injecté dans les handlers via `State<AppState>`.
/// Tous les champs sont peu coûteux à cloner (handles, `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub tokens: Arc<TokenService>,
    /// Identity token verifiers keyed by provider (`apple`, `google`...), configured at startup;
    /// fakes in tests.
    pub verifiers: Arc<Verifiers>,
    /// Catalogue de tirage en mémoire.
    pub catalog: Arc<CatalogCache>,
}
