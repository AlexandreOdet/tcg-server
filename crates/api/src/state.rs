use std::sync::Arc;

use sqlx::PgPool;

use crate::config::Config;

/// État partagé injecté dans les handlers via `State<AppState>`.
/// Les deux champs sont peu coûteux à cloner (`PgPool` et `Arc` sont des handles).
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    #[expect(dead_code, reason = "lu par l'authentification (phase 2)")]
    pub config: Arc<Config>,
}
