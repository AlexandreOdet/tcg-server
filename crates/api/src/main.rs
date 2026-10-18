mod config;
// Les variantes d'erreur non encore utilisées sont signalées en build normal ;
// à retirer quand toutes seront employées.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "variantes utilisées dans les phases suivantes")
)]
mod error;
mod state;

use std::{sync::Arc, time::Duration};

use axum::{extract::State, routing::get, Json, Router};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::{config::Config, error::AppResult, state::AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Charge `.env` s'il existe (développement local) ; en production, l'environnement suffit.
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::from_env()?;

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&config.database_url)
        .await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;
    tracing::info!("migrations appliquées");

    let bind_addr = config.bind_addr.clone();
    let state = AppState {
        pool,
        config: Arc::new(config),
    };

    let app = Router::new()
        .route("/health", get(health))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(%bind_addr, "serveur démarré");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("serveur arrêté");
    Ok(())
}

/// `/health` vérifie aussi que la base répond : utile pour les sondes de déploiement.
async fn health(State(state): State<AppState>) -> AppResult<Json<Value>> {
    sqlx::query("SELECT 1").execute(&state.pool).await?;
    Ok(Json(json!({ "status": "ok" })))
}

/// Attend Ctrl+C ou SIGTERM (envoyé par Docker / l'orchestrateur à l'arrêt).
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("impossible d'installer le gestionnaire Ctrl+C");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("impossible d'installer le gestionnaire SIGTERM")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("signal d'arrêt reçu");
}
