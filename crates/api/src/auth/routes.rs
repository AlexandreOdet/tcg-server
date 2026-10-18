//! Routes d'authentification : `/auth/apple`, `/auth/refresh`, `/auth/logout`, `/me`.

use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    extractor::AuthUser,
    sessions::{self, RefreshError},
    tokens::ACCESS_TTL_SECS,
    users,
    verifier::VerifyError,
};
use crate::{
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/apple", post(login_with_apple))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
        .route("/me", get(me))
}

#[derive(Deserialize)]
struct AppleLoginRequest {
    /// Jeton d'identité (JWT) renvoyé par Apple à l'app.
    identity_token: String,
    /// Nonce **brut** généré par l'app (Apple a reçu son SHA-256).
    nonce: String,
}

#[derive(Deserialize)]
struct RefreshRequest {
    refresh_token: String,
}

#[derive(Serialize)]
struct SessionResponse {
    access_token: String,
    refresh_token: String,
    token_type: &'static str,
    /// Durée de validité de l'access token, en secondes.
    expires_in: u64,
    /// `true` à la première connexion : l'app peut proposer de choisir un pseudo.
    is_new_user: bool,
}

impl SessionResponse {
    fn new(access_token: String, refresh_token: String, is_new_user: bool) -> Self {
        Self {
            access_token,
            refresh_token,
            token_type: "Bearer",
            expires_in: ACCESS_TTL_SECS,
            is_new_user,
        }
    }
}

async fn login_with_apple(
    State(state): State<AppState>,
    Json(request): Json<AppleLoginRequest>,
) -> AppResult<Json<SessionResponse>> {
    if request.identity_token.is_empty() || request.nonce.is_empty() {
        return Err(AppError::BadRequest(
            "identity_token et nonce sont obligatoires".into(),
        ));
    }

    let identity = state
        .apple
        .verify(&request.identity_token, &request.nonce)
        .await
        .map_err(|error| match error {
            VerifyError::Invalid(reason) => {
                // La raison précise reste dans les logs : elle n'aide qu'un attaquant.
                tracing::warn!(%reason, "connexion Apple refusée");
                AppError::Unauthorized
            }
            VerifyError::KeysUnavailable(source) => AppError::Internal(source),
        })?;

    let login = users::find_or_create(&state.pool, &identity).await?;
    let mut conn = state.pool.acquire().await?;
    let refresh_token = sessions::start_session(&mut conn, login.user_id, Utc::now()).await?;
    let access_token = state.tokens.issue_access(login.user_id)?;

    Ok(Json(SessionResponse::new(
        access_token,
        refresh_token,
        login.is_new,
    )))
}

async fn refresh(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> AppResult<Json<SessionResponse>> {
    let (user_id, refresh_token) =
        sessions::rotate(&state.pool, &request.refresh_token, Utc::now())
            .await
            .map_err(|error| match error {
                RefreshError::Invalid => AppError::Unauthorized,
                RefreshError::Reused => {
                    tracing::warn!("refresh token réutilisé : session révoquée");
                    AppError::Unauthorized
                }
                RefreshError::Database(error) => AppError::Database(error),
            })?;
    let access_token = state.tokens.issue_access(user_id)?;

    Ok(Json(SessionResponse::new(
        access_token,
        refresh_token,
        false,
    )))
}

async fn logout(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> AppResult<StatusCode> {
    sessions::revoke_session(&state.pool, &request.refresh_token, Utc::now()).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct MeResponse {
    id: Uuid,
    pseudo: Option<String>,
    created_at: DateTime<Utc>,
}

async fn me(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<MeResponse>> {
    // Un access token valide pour un utilisateur supprimé depuis ne doit plus ouvrir de porte.
    let profile = users::get(&state.pool, user.user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    Ok(Json(MeResponse {
        id: profile.id,
        pseudo: profile.pseudo,
        created_at: profile.created_at,
    }))
}
