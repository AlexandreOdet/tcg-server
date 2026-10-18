use axum::{
    extract::State,
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;

use super::service::{self, OpenError, Opened, StockInfo};
use crate::{
    auth::extractor::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me/boosters", get(stock))
        .route("/boosters/open", post(open))
}

async fn stock(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<StockInfo>> {
    Ok(Json(
        service::stock_info(&state.pool, user.user_id, Utc::now()).await?,
    ))
}

async fn open(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
) -> AppResult<Json<Opened>> {
    let key = idempotency_key(&headers)?;
    let catalog = state.catalog.get(&state.pool).await?;
    // Le générateur est créé et consommé dans `open`, jamais tenu à travers un `await` ailleurs.
    let mut rng = |n: u32| rand::random_range(0..n);
    let opened = service::open(
        &state.pool,
        &catalog,
        user.user_id,
        &key,
        Utc::now(),
        &mut rng,
    )
    .await
    .map_err(|e| match e {
        OpenError::NoStock => AppError::Conflict("aucun booster disponible".into()),
        OpenError::EmptyCatalog => AppError::Unavailable("catalogue vide".into()),
        OpenError::Database(e) => AppError::Database(e),
    })?;
    Ok(Json(opened))
}

fn idempotency_key(headers: &HeaderMap) -> AppResult<String> {
    let value = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .ok_or_else(|| AppError::BadRequest("en-tête Idempotency-Key obligatoire".into()))?;
    if value.is_empty() || value.len() > 128 || !value.chars().all(|c| c.is_ascii_graphic()) {
        return Err(AppError::BadRequest(
            "Idempotency-Key invalide (1 à 128 caractères ASCII visibles)".into(),
        ));
    }
    Ok(value.to_owned())
}
