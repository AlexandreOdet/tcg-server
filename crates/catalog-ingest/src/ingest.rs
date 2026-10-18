//! Orchestration : top des vues → résumés → cartes en base.

use std::collections::HashMap;

use anyhow::{bail, Context};
use chrono::{Datelike, NaiveDate};
use domain::rarity::RarityShares;
use futures::{stream, StreamExt};
use sqlx::PgPool;

use crate::{
    store::{upsert_and_rerank, NewCard, RarityCounts},
    wiki::WikiClient,
};

pub struct IngestParams {
    pub year: i32,
    pub month: u32,
    /// Nombre d'articles du top à examiner (l'API en fournit 1 000 au maximum).
    pub limit: usize,
    /// Nombre de requêtes de résumé simultanées.
    pub concurrency: usize,
    pub shares: RarityShares,
}

#[derive(Debug, PartialEq, Eq)]
pub struct IngestReport {
    pub examined: usize,
    /// Pages ignorées : absentes, ou qui ne sont pas des articles standard.
    pub skipped: usize,
    /// Résumés dont la récupération a échoué malgré les reprises.
    pub failed: usize,
    pub counts: RarityCounts,
}

pub async fn run(
    client: &WikiClient,
    pool: &PgPool,
    params: &IngestParams,
) -> anyhow::Result<IngestReport> {
    let days = days_in_month(params.year, params.month)?;
    let mut top = client.top_articles(params.year, params.month).await?;
    top.truncate(params.limit);
    let examined = top.len();
    tracing::info!(examined, "classement récupéré, lecture des résumés");

    let results: Vec<_> = stream::iter(top)
        .map(|article| async move {
            let summary = client.summary(&article.title).await;
            (article, summary)
        })
        .buffer_unordered(params.concurrency.max(1))
        .collect()
        .await;

    let mut skipped = 0;
    let mut failed = 0;
    // Plusieurs titres du top peuvent mener à la même page (redirections) : on garde le plus lu.
    let mut by_page: HashMap<i64, NewCard> = HashMap::new();
    for (article, summary) in results {
        match summary {
            Ok(Some(summary)) => {
                let avg_daily_views =
                    i32::try_from(article.views / u64::from(days)).unwrap_or(i32::MAX);
                let card = NewCard {
                    page_id: summary.page_id,
                    title: summary.title,
                    extract: summary.extract,
                    page_url: summary.page_url,
                    image_url: summary.image_url,
                    avg_daily_views,
                };
                by_page
                    .entry(card.page_id)
                    .and_modify(|known| {
                        if card.avg_daily_views > known.avg_daily_views {
                            *known = card.clone();
                        }
                    })
                    .or_insert(card);
            }
            Ok(None) => skipped += 1,
            Err(error) => {
                failed += 1;
                tracing::warn!(title = %article.title, %error, "résumé impossible à récupérer");
            }
        }
    }

    // Trop d'échecs = problème d'API ou de réseau : on n'écrit pas un catalogue tronqué.
    if examined > 0 && failed * 2 > examined {
        bail!("{failed} résumés en échec sur {examined} : ingestion annulée, base inchangée");
    }

    let cards: Vec<NewCard> = by_page.into_values().collect();
    let counts = upsert_and_rerank(pool, &cards, params.shares).await?;
    Ok(IngestReport {
        examined,
        skipped,
        failed,
        counts,
    })
}

pub fn days_in_month(year: i32, month: u32) -> anyhow::Result<u32> {
    let first = NaiveDate::from_ymd_opt(year, month, 1).context("mois invalide")?;
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .context("mois invalide")?;
    Ok((next - first).num_days() as u32)
}

/// Lit un mois au format `AAAA-MM`.
pub fn parse_year_month(value: &str) -> anyhow::Result<(i32, u32)> {
    let (year, month) = value
        .split_once('-')
        .context("format attendu : AAAA-MM (ex. 2026-09)")?;
    let year: i32 = year.parse().context("année invalide")?;
    let month: u32 = month.parse().context("mois invalide")?;
    days_in_month(year, month)?; // valide le couple
    Ok((year, month))
}

/// Mois précédant `today` : le dernier mois entièrement publié par Pageviews.
pub fn previous_month(today: NaiveDate) -> (i32, u32) {
    if today.month() == 1 {
        (today.year() - 1, 12)
    } else {
        (today.year(), today.month() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[test]
    fn counts_the_days_of_a_month() {
        assert_eq!(days_in_month(2026, 9).unwrap(), 30);
        assert_eq!(days_in_month(2026, 2).unwrap(), 28);
        assert_eq!(days_in_month(2024, 2).unwrap(), 29);
        assert_eq!(days_in_month(2026, 12).unwrap(), 31);
        assert!(days_in_month(2026, 13).is_err());
    }

    #[test]
    fn parses_a_year_month() {
        assert_eq!(parse_year_month("2026-09").unwrap(), (2026, 9));
        assert!(parse_year_month("2026").is_err());
        assert!(parse_year_month("2026-13").is_err());
        assert!(parse_year_month("abc-09").is_err());
    }

    #[test]
    fn previous_month_handles_january() {
        let day = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert_eq!(previous_month(day(2026, 10, 18)), (2026, 9));
        assert_eq!(previous_month(day(2026, 1, 3)), (2025, 12));
    }

    fn summary_json(page_id: i64, title: &str, kind: &str) -> String {
        format!(
            r#"{{"type":"{kind}","title":"{title}","pageid":{page_id},"extract":"Texte de {title}.",
            "content_urls":{{"desktop":{{"page":"https://fr.wikipedia.org/wiki/{title}"}}}}}}"#
        )
    }

    async fn mock_summary(server: &MockServer, wiki_title: &str, body: String) {
        Mock::given(method("GET"))
            .and(path(format!("/page/summary/{wiki_title}")))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }

    fn params(limit: usize) -> IngestParams {
        IngestParams {
            year: 2026,
            month: 9,
            limit,
            concurrency: 3,
            shares: RarityShares::new(0.34, 0.33, 0.33).unwrap(),
        }
    }

    async fn server_with_top() -> MockServer {
        let server = MockServer::start().await;
        let top = r#"{"items":[{"articles":[
            {"article":"Chat","views":3000,"rank":1},
            {"article":"Felis_catus","views":300,"rank":2},
            {"article":"Origami","views":900,"rank":3},
            {"article":"Main_page","views":800,"rank":4},
            {"article":"Page_absente","views":700,"rank":5},
            {"article":"Ada_Lovelace","views":600,"rank":6}]}]}"#;
        Mock::given(method("GET"))
            .and(path(
                "/metrics/pageviews/top/fr.wikipedia/all-access/2026/09/all-days",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(top))
            .mount(&server)
            .await;
        mock_summary(&server, "Chat", summary_json(1, "Chat", "standard")).await;
        // Redirection : même page que « Chat », moins lue.
        mock_summary(&server, "Felis_catus", summary_json(1, "Chat", "standard")).await;
        mock_summary(&server, "Origami", summary_json(2, "Origami", "standard")).await;
        mock_summary(&server, "Main_page", summary_json(3, "Accueil", "mainpage")).await;
        mock_summary(
            &server,
            "Ada_Lovelace",
            summary_json(4, "Ada Lovelace", "standard"),
        )
        .await;
        // « Page_absente » n'est pas mocké : 404.
        server
    }

    fn client(server: &MockServer) -> WikiClient {
        WikiClient::new("tcg-test", &server.uri(), &server.uri())
            .unwrap()
            .with_retry(2, Duration::from_millis(1))
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn ingests_a_month_end_to_end(pool: PgPool) {
        let server = server_with_top().await;
        let report = run(&client(&server), &pool, &params(1000)).await.unwrap();

        // 6 articles examinés ; Main_page (mainpage) et Page_absente (404) sont ignorés.
        assert_eq!(report.examined, 6);
        assert_eq!(report.skipped, 2);
        assert_eq!(report.failed, 0);
        // 3 cartes distinctes : la redirection est fusionnée avec « Chat ».
        assert_eq!(report.counts.total(), 3);

        let cards: Vec<(String, i32, String)> = sqlx::query_as(
            "SELECT title, avg_daily_views, rarity::text FROM cards ORDER BY avg_daily_views DESC",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            cards,
            [
                ("Chat".to_owned(), 100, "legendary".to_owned()), // 3000 vues / 30 jours
                ("Origami".to_owned(), 30, "super_rare".to_owned()),
                ("Ada Lovelace".to_owned(), 20, "rare".to_owned()),
            ]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn the_limit_caps_the_examined_articles(pool: PgPool) {
        let server = server_with_top().await;
        let report = run(&client(&server), &pool, &params(2)).await.unwrap();
        assert_eq!(report.examined, 2);
        assert_eq!(report.counts.total(), 1); // Chat et sa redirection
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn too_many_failures_leave_the_database_untouched(pool: PgPool) {
        let server = MockServer::start().await;
        let top = r#"{"items":[{"articles":[
            {"article":"A","views":300,"rank":1},{"article":"B","views":200,"rank":2}]}]}"#;
        Mock::given(method("GET"))
            .and(path(
                "/metrics/pageviews/top/fr.wikipedia/all-access/2026/09/all-days",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(top))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/page/summary/A"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/page/summary/B"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let error = run(&client(&server), &pool, &params(1000))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("ingestion annulée"));
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM cards")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
