//! Construction du routeur HTTP, séparée de `main` pour pouvoir le tester.

use axum::{extract::State, routing::get, Json, Router};
use serde_json::{json, Value};
use tower_http::trace::TraceLayer;

use crate::{auth, error::AppResult, state::AppState};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .merge(auth::routes::router())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// `/health` vérifie aussi que la base répond : utile pour les sondes de déploiement.
async fn health(State(state): State<AppState>) -> AppResult<Json<Value>> {
    sqlx::query("SELECT 1").execute(&state.pool).await?;
    Ok(Json(json!({ "status": "ok" })))
}
