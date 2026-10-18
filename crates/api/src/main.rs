mod config;

use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

use crate::config::Config;

// TODO(phase 0) : suite des étapes, dans l'ordre suggéré dans docs/PLAN.md
//  2. `error.rs` : enum `AppError` (crate `thiserror`) qui implémente `IntoResponse`,
//     pour que les handlers renvoient `Result<_, AppError>`.
//  3. `state.rs` : `AppState { pool: PgPool, config: Config }` injecté avec `with_state`.
//  4. Initialiser `tracing_subscriber`, se connecter à Postgres, lancer `sqlx::migrate!()`.
//  5. Faire vérifier la base par `/health` (un `SELECT 1`).
//  6. Arrêt propre sur Ctrl+C via `with_graceful_shutdown` (feature `signal` de tokio déjà activée).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Charge `.env` s'il existe (développement local) ; en production, l'environnement suffit.
    let _ = dotenvy::dotenv();
    let config = Config::from_env()?;

    let app = Router::new().route("/health", get(health));

    let listener = tokio::net::TcpListener::bind(&config.bind_addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}
