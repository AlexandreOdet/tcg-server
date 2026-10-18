//! Client des API Wikimedia : top des vues (Pageviews) et résumé d'un article (REST Wikipédia).

use std::time::Duration;

use anyhow::{bail, Context};
use reqwest::{header::RETRY_AFTER, StatusCode, Url};
use serde::Deserialize;

pub const DEFAULT_REST_BASE: &str = "https://fr.wikipedia.org/api/rest_v1";
pub const DEFAULT_METRICS_BASE: &str = "https://wikimedia.org/api/rest_v1";

/// Un article du classement mensuel des vues. `title` garde les underscores de l'API.
#[derive(Debug, Clone, PartialEq)]
pub struct TopArticle {
    pub title: String,
    pub views: u64,
}

/// Ce qu'il faut d'un article pour fabriquer une carte.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub page_id: i64,
    pub title: String,
    pub extract: String,
    pub page_url: String,
    pub image_url: Option<String>,
}

pub struct WikiClient {
    http: reqwest::Client,
    rest_base: String,
    metrics_base: String,
    max_attempts: u32,
    backoff_base: Duration,
}

impl WikiClient {
    /// `user_agent` doit identifier l'application et un contact : c'est exigé par Wikimedia.
    pub fn new(user_agent: &str, rest_base: &str, metrics_base: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(user_agent)
            .timeout(Duration::from_secs(20))
            .build()
            .context("client HTTP")?;
        Ok(Self {
            http,
            rest_base: rest_base.trim_end_matches('/').to_owned(),
            metrics_base: metrics_base.trim_end_matches('/').to_owned(),
            max_attempts: 4,
            backoff_base: Duration::from_secs(1),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_retry(mut self, max_attempts: u32, backoff_base: Duration) -> Self {
        self.max_attempts = max_attempts;
        self.backoff_base = backoff_base;
        self
    }

    /// Classement mensuel des articles les plus lus (1 000 au maximum, limite de l'API).
    pub async fn top_articles(&self, year: i32, month: u32) -> anyhow::Result<Vec<TopArticle>> {
        let url = format!(
            "{}/metrics/pageviews/top/fr.wikipedia/all-access/{year}/{month:02}/all-days",
            self.metrics_base
        );
        let url = Url::parse(&url).context("URL Pageviews")?;
        let Some(body) = self.get_text(url).await? else {
            bail!("pas de données Pageviews pour {year}-{month:02} (mois pas encore publié ?)");
        };
        parse_top(&body)
    }

    /// Résumé d'un article. `None` si la page n'existe pas ou n'est pas un article standard
    /// exploitable (pages spéciales, homonymies, page d'accueil, article sans texte).
    pub async fn summary(&self, title: &str) -> anyhow::Result<Option<Summary>> {
        let mut url = Url::parse(&self.rest_base).context("URL REST")?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("URL REST invalide"))?
            .pop_if_empty()
            .extend(["page", "summary", title]); // le titre est encodé (ex. `AC/DC` → `AC%2FDC`)
        match self.get_text(url).await? {
            Some(body) => parse_summary(&body),
            None => Ok(None),
        }
    }

    /// GET avec reprise sur 429, 5xx et erreurs réseau (attente croissante, `Retry-After` respecté).
    /// `Ok(None)` pour un 404.
    async fn get_text(&self, url: Url) -> anyhow::Result<Option<String>> {
        let mut delay = self.backoff_base;
        for attempt in 1..=self.max_attempts {
            let is_last = attempt == self.max_attempts;
            match self.http.get(url.clone()).send().await {
                Ok(response) => {
                    let status = response.status();
                    if status == StatusCode::NOT_FOUND {
                        return Ok(None);
                    }
                    if status.is_success() {
                        return Ok(Some(response.text().await?));
                    }
                    let retryable =
                        status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
                    if !retryable || is_last {
                        bail!("GET {url} : HTTP {status}");
                    }
                    let wait = response
                        .headers()
                        .get(RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(|secs| Duration::from_secs(secs).min(Duration::from_secs(60)))
                        .unwrap_or(delay);
                    tracing::debug!(%url, %status, ?wait, "nouvelle tentative");
                    tokio::time::sleep(wait).await;
                }
                Err(error) => {
                    if is_last {
                        return Err(error).with_context(|| format!("GET {url}"));
                    }
                    tracing::debug!(%url, %error, ?delay, "nouvelle tentative");
                    tokio::time::sleep(delay).await;
                }
            }
            delay *= 2;
        }
        bail!("GET {url} : aucune tentative autorisée")
    }
}

#[derive(Deserialize)]
struct TopResponse {
    items: Vec<TopItem>,
}

#[derive(Deserialize)]
struct TopItem {
    articles: Vec<TopEntry>,
}

#[derive(Deserialize)]
struct TopEntry {
    article: String,
    views: u64,
}

pub fn parse_top(json: &str) -> anyhow::Result<Vec<TopArticle>> {
    let response: TopResponse =
        serde_json::from_str(json).context("réponse Pageviews (top) illisible")?;
    Ok(response
        .items
        .into_iter()
        .flat_map(|item| item.articles)
        .map(|entry| TopArticle {
            title: entry.article,
            views: entry.views,
        })
        .collect())
}

#[derive(Deserialize)]
struct SummaryResponse {
    #[serde(rename = "type")]
    kind: String,
    pageid: Option<i64>,
    title: Option<String>,
    extract: Option<String>,
    thumbnail: Option<Image>,
    content_urls: Option<ContentUrls>,
}

#[derive(Deserialize)]
struct Image {
    source: String,
}

#[derive(Deserialize)]
struct ContentUrls {
    desktop: Option<DesktopUrl>,
}

#[derive(Deserialize)]
struct DesktopUrl {
    page: String,
}

pub fn parse_summary(json: &str) -> anyhow::Result<Option<Summary>> {
    let response: SummaryResponse =
        serde_json::from_str(json).context("réponse « summary » illisible")?;
    if response.kind != "standard" {
        return Ok(None);
    }
    let (Some(page_id), Some(title), Some(extract)) =
        (response.pageid, response.title, response.extract)
    else {
        return Ok(None);
    };
    let Some(page_url) = response
        .content_urls
        .and_then(|urls| urls.desktop)
        .map(|desktop| desktop.page)
    else {
        return Ok(None);
    };
    if extract.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(Summary {
        page_id,
        title,
        extract,
        page_url,
        image_url: response.thumbnail.map(|image| image.source),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    const TOP_JSON: &str = r#"{"items":[{"project":"fr.wikipedia","access":"all-access",
        "year":"2026","month":"09","day":"all-days","articles":[
        {"article":"Chat","views":3000,"rank":1},
        {"article":"Ada_Lovelace","views":600,"rank":2}]}]}"#;

    const SUMMARY_JSON: &str = r#"{"type":"standard","title":"Chat","displaytitle":"<i>Chat</i>",
        "pageid":42,"extract":"Le chat est un mammifère.",
        "thumbnail":{"source":"https://upload.example/chat.jpg","width":320,"height":240},
        "content_urls":{"desktop":{"page":"https://fr.wikipedia.org/wiki/Chat"}}}"#;

    fn client(server: &MockServer) -> WikiClient {
        WikiClient::new("tcg-test", &server.uri(), &server.uri())
            .unwrap()
            .with_retry(3, Duration::from_millis(1))
    }

    #[test]
    fn parses_the_top_response() {
        let top = parse_top(TOP_JSON).unwrap();
        assert_eq!(
            top,
            [
                TopArticle {
                    title: "Chat".into(),
                    views: 3000
                },
                TopArticle {
                    title: "Ada_Lovelace".into(),
                    views: 600
                },
            ]
        );
    }

    #[test]
    fn rejects_an_unreadable_top_response() {
        assert!(parse_top("pas du json").is_err());
    }

    #[test]
    fn parses_a_standard_summary() {
        let summary = parse_summary(SUMMARY_JSON).unwrap().unwrap();
        assert_eq!(summary.page_id, 42);
        assert_eq!(summary.title, "Chat");
        assert_eq!(summary.extract, "Le chat est un mammifère.");
        assert_eq!(summary.page_url, "https://fr.wikipedia.org/wiki/Chat");
        assert_eq!(
            summary.image_url.as_deref(),
            Some("https://upload.example/chat.jpg")
        );
    }

    #[test]
    fn a_summary_without_image_is_still_valid() {
        let json = r#"{"type":"standard","title":"Origami","pageid":7,"extract":"Pliage.",
            "content_urls":{"desktop":{"page":"https://fr.wikipedia.org/wiki/Origami"}}}"#;
        assert_eq!(parse_summary(json).unwrap().unwrap().image_url, None);
    }

    #[test]
    fn non_article_pages_are_ignored() {
        for kind in ["disambiguation", "mainpage", "no-extract"] {
            let json = format!(r#"{{"type":"{kind}","title":"X","pageid":1,"extract":"texte"}}"#);
            assert_eq!(parse_summary(&json).unwrap(), None, "type {kind}");
        }
    }

    #[test]
    fn an_empty_extract_is_ignored() {
        let json = r#"{"type":"standard","title":"X","pageid":1,"extract":"  ",
            "content_urls":{"desktop":{"page":"https://fr.wikipedia.org/wiki/X"}}}"#;
        assert_eq!(parse_summary(json).unwrap(), None);
    }

    #[tokio::test]
    async fn fetches_the_monthly_top() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/metrics/pageviews/top/fr.wikipedia/all-access/2026/09/all-days",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(TOP_JSON))
            .mount(&server)
            .await;

        let top = client(&server).top_articles(2026, 9).await.unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].title, "Chat");
    }

    #[tokio::test]
    async fn a_missing_month_is_an_explicit_error() {
        let server = MockServer::start().await; // aucun mock : 404
        let error = client(&server).top_articles(2026, 9).await.unwrap_err();
        assert!(error.to_string().contains("pas de données"));
    }

    #[tokio::test]
    async fn unknown_page_gives_none() {
        let server = MockServer::start().await;
        assert_eq!(client(&server).summary("Page_absente").await.unwrap(), None);
    }

    #[tokio::test]
    async fn retries_after_a_429_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/page/summary/Chat"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/page/summary/Chat"))
            .respond_with(ResponseTemplate::new(200).set_body_string(SUMMARY_JSON))
            .mount(&server)
            .await;

        let summary = client(&server).summary("Chat").await.unwrap();
        assert_eq!(summary.unwrap().page_id, 42);
    }

    #[tokio::test]
    async fn gives_up_after_the_maximum_number_of_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .expect(3)
            .mount(&server)
            .await;

        let error = client(&server).summary("Chat").await.unwrap_err();
        assert!(error.to_string().contains("500"));
    }

    #[tokio::test]
    async fn other_client_errors_are_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403))
            .expect(1)
            .mount(&server)
            .await;

        assert!(client(&server).summary("Chat").await.is_err());
    }

    #[tokio::test]
    async fn titles_are_percent_encoded_in_the_path() {
        let server = MockServer::start().await;
        let _ = client(&server).summary("AC/DC").await;

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests[0].url.path(), "/page/summary/AC%2FDC");
    }
}
