//! Extracteur `AuthUser` : protège une route en une ligne.
//!
//! ```ignore
//! async fn handler(user: AuthUser) -> ... { user.user_id }
//! ```

use axum::{extract::FromRequestParts, http::header::AUTHORIZATION, http::request::Parts};
use uuid::Uuid;

use crate::{error::AppError, state::AppState};

/// Utilisateur authentifié par un access token `Authorization: Bearer ...`.
pub struct AuthUser {
    pub user_id: Uuid,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(AppError::Unauthorized)?;

        let user_id = state
            .tokens
            .verify_access(token)
            .map_err(|_| AppError::Unauthorized)?;
        Ok(Self { user_id })
    }
}
