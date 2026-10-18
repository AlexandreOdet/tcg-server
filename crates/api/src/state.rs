use std::sync::Arc;

use sqlx::PgPool;

use crate::{
    auth::{tokens::TokenService, verifier::TokenVerifier},
    boosters::catalog::CatalogCache,
};

/// État partagé injecté dans les handlers via `State<AppState>`.
/// Tous les champs sont peu coûteux à cloner (handles, `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub tokens: Arc<TokenService>,
    /// Vérificateur des jetons Sign in with Apple (un faux en test).
    pub apple: Arc<dyn TokenVerifier>,
    /// Catalogue de tirage en mémoire.
    pub catalog: Arc<CatalogCache>,
}
