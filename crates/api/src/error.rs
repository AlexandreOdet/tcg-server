use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Résultat standard des handlers : `async fn handler() -> AppResult<Json<T>>`.
pub type AppResult<T> = Result<T, AppError>;

/// Erreur applicative. Les messages affichés (`Display`) sont destinés au client :
/// ils ne contiennent jamais de détail interne. Les causes (`sqlx`, `anyhow`) sont
/// uniquement journalisées.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("requête invalide : {0}")]
    BadRequest(String),
    #[error("non authentifié")]
    Unauthorized,
    #[error("ressource introuvable")]
    NotFound,
    #[error("conflit : {0}")]
    Conflict(String),
    #[error("service indisponible : {0}")]
    Unavailable(String),
    #[error("erreur de base de données")]
    Database(#[from] sqlx::Error),
    #[error("erreur interne")]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Database(_) | Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Code stable, lisible par l'app iOS (pour les `switch` côté client).
    fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad_request",
            Self::Unauthorized => "unauthorized",
            Self::NotFound => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Unavailable(_) => "unavailable",
            Self::Database(_) | Self::Internal(_) => "internal",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            // La cause complète reste dans les logs, jamais dans la réponse.
            tracing::error!(error = ?self, "erreur serveur");
        }
        let body = json!({ "error": { "code": self.code(), "message": self.to_string() } });
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use serde_json::Value;

    async fn render(error: AppError) -> (StatusCode, Value) {
        let response = error.into_response();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn client_errors_map_to_their_status_and_code() {
        let (status, body) = render(AppError::NotFound).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "not_found");

        let (status, body) = render(AppError::BadRequest("pseudo vide".into())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["message"], "requête invalide : pseudo vide");

        let (status, _) = render(AppError::Unauthorized).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _) = render(AppError::Conflict("déjà ouvert".into())).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn server_errors_do_not_leak_internal_details() {
        let (status, body) =
            render(AppError::Internal(anyhow::anyhow!("mot de passe: hunter2"))).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"]["code"], "internal");
        assert!(!body.to_string().contains("hunter2"));

        let (status, body) = render(AppError::Database(sqlx::Error::PoolTimedOut)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"]["message"], "erreur de base de données");
    }
}
