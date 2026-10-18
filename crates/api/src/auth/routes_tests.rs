//! Tests d'intégration des routes d'authentification : vrai routeur, vraie base,
//! faux vérificateur Apple.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use super::{
    tokens::{TokenService, ACCESS_TTL_SECS},
    verifier::{TokenVerifier, VerifiedIdentity, VerifyError},
};
use crate::{app, state::AppState};

const SECRET: &str = "un-secret-de-test-d-au-moins-32-caracteres";
const NONCE: &str = "nonce-ok";

/// Faux Apple : `ok:<sub>` est valide, `outage` simule une panne des clés, le reste est refusé.
struct StubVerifier(&'static str);

#[async_trait]
impl TokenVerifier for StubVerifier {
    async fn verify(
        &self,
        id_token: &str,
        raw_nonce: &str,
    ) -> Result<VerifiedIdentity, VerifyError> {
        if id_token == "outage" {
            return Err(VerifyError::KeysUnavailable(anyhow::anyhow!(
                "détail interne secret"
            )));
        }
        match id_token.strip_prefix("ok:") {
            Some(subject) if raw_nonce == NONCE => Ok(VerifiedIdentity {
                provider: self.0.into(),
                subject: subject.into(),
                email: Some(format!("{subject}@example.com")),
                email_verified: true,
            }),
            _ => Err(VerifyError::Invalid(
                "refusé par le faux vérificateur".into(),
            )),
        }
    }
}

fn app(pool: PgPool) -> Router {
    app::router(AppState {
        pool,
        tokens: Arc::new(TokenService::new(SECRET).unwrap()),
        verifiers: Arc::new(
            ["apple", "google"]
                .into_iter()
                .map(|name| {
                    (
                        name.to_owned(),
                        Arc::new(StubVerifier(name)) as Arc<dyn TokenVerifier>,
                    )
                })
                .collect(),
        ),
        catalog: Arc::default(),
    })
}

async fn call(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    authorization: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(value) = authorization {
        request = request.header(header::AUTHORIZATION, value);
    }
    let body = match body {
        Some(json) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&json).unwrap())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn login(app: &Router, subject: &str) -> Value {
    let (status, body) = call(
        app,
        Method::POST,
        "/auth/apple",
        Some(json!({ "identity_token": format!("ok:{subject}"), "nonce": NONCE })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn bearer(token: &Value) -> String {
    format!("Bearer {}", token.as_str().unwrap())
}

async fn me(app: &Router, authorization: Option<&str>) -> (StatusCode, Value) {
    call(app, Method::GET, "/me", None, authorization).await
}

async fn refresh(app: &Router, refresh_token: &Value) -> (StatusCode, Value) {
    call(
        app,
        Method::POST,
        "/auth/refresh",
        Some(json!({ "refresh_token": refresh_token })),
        None,
    )
    .await
}

#[sqlx::test(migrations = "../../migrations")]
async fn health_reports_ok(pool: PgPool) {
    let (status, body) = call(&app(pool), Method::GET, "/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[sqlx::test(migrations = "../../migrations")]
async fn first_login_creates_a_user_and_returns_a_working_session(pool: PgPool) {
    let app = app(pool);
    let session = login(&app, "sub-1").await;

    assert_eq!(session["token_type"], "Bearer");
    assert_eq!(session["expires_in"], ACCESS_TTL_SECS);
    assert_eq!(session["is_new_user"], true);

    let (status, profile) = me(&app, Some(&bearer(&session["access_token"]))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(Uuid::parse_str(profile["id"].as_str().unwrap()).is_ok());
    assert_eq!(profile["pseudo"], Value::Null);
}

#[sqlx::test(migrations = "../../migrations")]
async fn logging_in_again_finds_the_same_user(pool: PgPool) {
    let app = app(pool);
    let first = login(&app, "sub-1").await;
    let second = login(&app, "sub-1").await;

    assert_eq!(second["is_new_user"], false);
    assert_ne!(first["refresh_token"], second["refresh_token"]);

    let id = |session: &Value| {
        let token = bearer(&session["access_token"]);
        let app = app.clone();
        async move { me(&app, Some(&token)).await.1["id"].clone() }
    };
    assert_eq!(id(&first).await, id(&second).await);

    let other = login(&app, "sub-2").await;
    assert_ne!(id(&first).await, id(&other).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_rejects_invalid_credentials_without_details(pool: PgPool) {
    let app = app(pool);
    for (token, nonce) in [("mauvais-jeton", NONCE), ("ok:sub-1", "mauvais-nonce")] {
        let (status, body) = call(
            &app,
            Method::POST,
            "/auth/apple",
            Some(json!({ "identity_token": token, "nonce": nonce })),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "unauthorized");
        assert!(!body.to_string().contains("faux vérificateur"));
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_requires_both_fields(pool: PgPool) {
    let app = app(pool);
    let (status, body) = call(
        &app,
        Method::POST,
        "/auth/apple",
        Some(json!({ "identity_token": "", "nonce": NONCE })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");

    let (status, _) = call(
        &app,
        Method::POST,
        "/auth/apple",
        Some(json!({ "identity_token": "ok:sub-1" })),
        None,
    )
    .await;
    assert!(status.is_client_error());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_provider_outage_is_a_server_error_that_leaks_nothing(pool: PgPool) {
    let (status, body) = call(
        &app(pool),
        Method::POST,
        "/auth/apple",
        Some(json!({ "identity_token": "outage", "nonce": NONCE })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"]["code"], "internal");
    assert!(!body.to_string().contains("secret"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn me_requires_a_valid_access_token(pool: PgPool) {
    let app = app(pool);
    let session = login(&app, "sub-1").await;
    let access = session["access_token"].as_str().unwrap();

    let expired = TokenService::new(SECRET)
        .unwrap()
        .issue_access_at(Uuid::new_v4(), jsonwebtoken::get_current_timestamp() - 3600)
        .unwrap();

    let attempts = [
        None,
        Some("Basic abc".to_owned()),
        Some(format!("Bearer{access}")), // sans espace
        Some("Bearer pas.un.jwt".to_owned()),
        Some(format!("Bearer {expired}")),
        // Un refresh token n'est pas un access token.
        Some(bearer(&session["refresh_token"])),
    ];
    for authorization in attempts {
        let (status, body) = me(&app, authorization.as_deref()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{authorization:?}");
        assert_eq!(body["error"]["code"], "unauthorized");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_deleted_user_can_no_longer_use_a_still_valid_access_token(pool: PgPool) {
    let app = app(pool.clone());
    let session = login(&app, "sub-1").await;
    sqlx::query("DELETE FROM users")
        .execute(&pool)
        .await
        .unwrap();

    let (status, _) = me(&app, Some(&bearer(&session["access_token"]))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn refresh_rotates_the_session(pool: PgPool) {
    let app = app(pool);
    let first = login(&app, "sub-1").await;

    let (status, second) = refresh(&app, &first["refresh_token"]).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["is_new_user"], false);
    assert_ne!(second["refresh_token"], first["refresh_token"]);

    // Le nouvel access token fonctionne.
    let (status, _) = me(&app, Some(&bearer(&second["access_token"]))).await;
    assert_eq!(status, StatusCode::OK);

    // L'ancien refresh token est grillé, le nouveau fonctionne.
    let (status, _) = refresh(&app, &first["refresh_token"]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = refresh(&app, &second["refresh_token"]).await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn refresh_rejects_an_unknown_token(pool: PgPool) {
    let (status, body) = refresh(&app(pool), &json!("inconnu")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[sqlx::test(migrations = "../../migrations")]
async fn logout_ends_the_session(pool: PgPool) {
    let app = app(pool);
    let session = login(&app, "sub-1").await;
    let logout = |token: Value| {
        let app = app.clone();
        async move {
            call(
                &app,
                Method::POST,
                "/auth/logout",
                Some(json!({ "refresh_token": token })),
                None,
            )
            .await
            .0
        }
    };

    assert_eq!(
        logout(session["refresh_token"].clone()).await,
        StatusCode::NO_CONTENT
    );
    let (status, _) = refresh(&app, &session["refresh_token"]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Idempotent, et sans erreur pour un jeton inconnu.
    assert_eq!(
        logout(session["refresh_token"].clone()).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(logout(json!("inconnu")).await, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrations = "../../migrations")]
async fn google_login_works_and_the_same_subject_is_a_different_player_per_provider(pool: PgPool) {
    let app = app(pool);
    let body = json!({ "identity_token": "ok:same-sub", "nonce": NONCE });

    let (status, google) = call(&app, Method::POST, "/auth/google", Some(body.clone()), None).await;
    assert_eq!(status, StatusCode::OK, "{google}");
    assert_eq!(google["is_new_user"], true);

    // Same `sub` at Apple: a different identity, hence a different player.
    let (_, apple) = call(&app, Method::POST, "/auth/apple", Some(body), None).await;
    assert_eq!(apple["is_new_user"], true);

    let (_, a) = me(&app, Some(&bearer(&google["access_token"]))).await;
    let (_, b) = me(&app, Some(&bearer(&apple["access_token"]))).await;
    assert_ne!(a["id"], b["id"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_unconfigured_provider_is_not_found(pool: PgPool) {
    let body = json!({ "identity_token": "ok:x", "nonce": NONCE });
    let (status, body) = call(&app(pool), Method::POST, "/auth/facebook", Some(body), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

#[sqlx::test(migrations = "../../migrations")]
async fn refresh_and_logout_are_not_swallowed_by_the_provider_route(pool: PgPool) {
    // `/auth/refresh` must stay the refresh route, not be read as a "provider".
    let (status, body) = call(
        &app(pool),
        Method::POST,
        "/auth/refresh",
        Some(json!({ "refresh_token": "inconnu" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}
