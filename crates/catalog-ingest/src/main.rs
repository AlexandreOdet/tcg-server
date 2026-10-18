//! Ingestion du catalogue de cartes : top mensuel des vues Wikipédia → table `cards`.
//!
//! Exemple : `cargo run -p catalog-ingest -- --month 2026-09 --limit 1000`

mod ingest;
mod store;
mod wiki;

use clap::Parser;
use domain::rarity::RarityShares;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

use crate::{
    ingest::{parse_year_month, previous_month, IngestParams},
    wiki::{WikiClient, DEFAULT_METRICS_BASE, DEFAULT_REST_BASE},
};

#[derive(Parser)]
#[command(about = "Importe les articles Wikipédia les plus lus comme cartes")]
struct Args {
    /// Mois du classement (AAAA-MM). Par défaut : le mois précédent.
    #[arg(long)]
    month: Option<String>,

    /// Nombre d'articles du classement à examiner (1 000 au maximum, limite de l'API).
    #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u16).range(1..=1000))]
    limit: u16,

    /// Requêtes simultanées vers Wikipédia. Restez raisonnable.
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u8).range(1..=20))]
    concurrency: u8,

    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,

    /// Wikimedia exige un User-Agent qui identifie l'application et un contact.
    #[arg(
        long,
        env = "INGEST_USER_AGENT",
        default_value = "tcg-server-ingest/0.1 (https://github.com/AlexandreOdet/tcg-server)"
    )]
    user_agent: String,

    /// Surcharges pour les tests avec un faux serveur.
    #[arg(long, env = "WIKI_REST_BASE", default_value = DEFAULT_REST_BASE, hide = true)]
    rest_base: String,
    #[arg(long, env = "WIKI_METRICS_BASE", default_value = DEFAULT_METRICS_BASE, hide = true)]
    metrics_base: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args = Args::parse();

    let (year, month) = match &args.month {
        Some(value) => parse_year_month(value)?,
        None => previous_month(chrono::Utc::now().date_naive()),
    };
    tracing::info!(year, month, limit = args.limit, "ingestion du catalogue");

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&args.database_url)
        .await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;

    let client = WikiClient::new(&args.user_agent, &args.rest_base, &args.metrics_base)?;
    let params = IngestParams {
        year,
        month,
        limit: usize::from(args.limit),
        concurrency: usize::from(args.concurrency),
        shares: RarityShares::default(),
    };
    let report = ingest::run(&client, &pool, &params).await?;

    let counts = report.counts;
    tracing::info!(
        examined = report.examined,
        skipped = report.skipped,
        failed = report.failed,
        catalogue = counts.total(),
        legendary = counts.legendary,
        super_rare = counts.super_rare,
        rare = counts.rare,
        common = counts.common,
        "ingestion terminée"
    );
    Ok(())
}
