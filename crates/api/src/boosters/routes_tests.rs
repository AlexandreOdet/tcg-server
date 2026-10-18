//! Tests d'intégration des boosters : vrai routeur, vraie base.

use std::{collections::HashSet, sync::Arc};

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use chrono::{Duration, Utc};
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use super::service;
use crate::{
    app,
    auth::{
        tokens::TokenService,
        verifier::{TokenVerifier, VerifiedIdentity, VerifyError},
    },
    state::AppState,
};

const SECRET: &str = "un-secret-de-test-d-au-moins-32-caracteres";

struct NoApple;

#[async_trait::async_trait]
impl TokenVerifier for NoApple {
    async fn verify(&self, _: &str, _: &str) -> Result<VerifiedIdentity, VerifyError> {
        Err(VerifyError::Invalid("inutilisé".into()))
    }
}

fn app(pool: PgPool) -> Router {
    app::router(AppState {
        pool,
        tokens: Arc::new(TokenService::new(SECRET).unwrap()),
        apple: Arc::new(NoApple),
        catalog: Arc::default(),
    })
}

/// Crée un joueur et renvoie son id et son en-tête `Authorization`.
async fn user(pool: &PgPool) -> (Uuid, String) {
    let (id,): (Uuid,) = sqlx::query_as("INSERT INTO users DEFAULT VALUES RETURNING id")
        .fetch_one(pool)
        .await
        .unwrap();
    let token = TokenService::new(SECRET).unwrap().issue_access(id).unwrap();
    (id, format!("Bearer {token}"))
}

/// Catalogue de test : `n` cartes réparties sur les quatre raretés.
async fn seed_cards(pool: &PgPool, n: i64) {
    for i in 0..n {
        let rarity = match i % 10 {
            0 => "legendary",
            1 | 2 => "super_rare",
            3..=5 => "rare",
            _ => "common",
        };
        sqlx::query(
            "INSERT INTO cards (wiki_page_id, title, extract, page_url, avg_daily_views, rarity)
             VALUES ($1, $2, 'extrait', 'https://fr.wikipedia.org/wiki/X', 10, $3::rarity)",
        )
        .bind(i)
        .bind(format!("Carte {i}"))
        .bind(rarity)
        .execute(pool)
        .await
        .unwrap();
    }
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    authorization: Option<&str>,
    key: Option<&str>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(a) = authorization {
        req = req.header(header::AUTHORIZATION, a);
    }
    if let Some(k) = key {
        req = req.header("Idempotency-Key", k);
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn open(app: &Router, auth: &str, key: &str) -> (StatusCode, Value) {
    request(app, Method::POST, "/boosters/open", Some(auth), Some(key)).await
}

async fn owned_total(pool: &PgPool, user: Uuid) -> i64 {
    let (n,): (Option<i64>,) =
        sqlx::query_as("SELECT SUM(quantity)::bigint FROM user_cards WHERE user_id = $1")
            .bind(user)
            .fetch_one(pool)
            .await
            .unwrap();
    n.unwrap_or(0)
}

async fn stock_of(pool: &PgPool, user: Uuid) -> i32 {
    let (n,): (i32,) = sqlx::query_as("SELECT count FROM booster_stock WHERE user_id = $1")
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap();
    n
}

#[sqlx::test(migrations = "../../migrations")]
async fn new_player_sees_the_starting_stock(pool: PgPool) {
    let (_, auth) = user(&pool).await;
    let (status, body) = request(&app(pool), Method::GET, "/me/boosters", Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["available"], 3);
    assert_eq!(body["max"], 10);
    assert_eq!(body["interval_secs"], 600);
    assert!(body["next_at"].is_string());
}

#[sqlx::test(migrations = "../../migrations")]
async fn routes_require_authentication(pool: PgPool) {
    let app = app(pool);
    let (status, _) = request(&app, Method::GET, "/me/boosters", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = request(&app, Method::POST, "/boosters/open", None, Some("k")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn idempotency_key_is_required_and_validated(pool: PgPool) {
    let (_, auth) = user(&pool).await;
    seed_cards(&pool, 40).await;
    let app = app(pool);
    let (status, body) = request(&app, Method::POST, "/boosters/open", Some(&auth), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");
    let (status, _) = open(&app, &auth, &"x".repeat(129)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../../migrations")]
async fn opening_gives_five_cards_updates_collection_and_stock(pool: PgPool) {
    let (user_id, auth) = user(&pool).await;
    seed_cards(&pool, 40).await;
    let app = app(pool.clone());

    let (status, body) = open(&app, &auth, "k1").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let cards = body["cards"].as_array().unwrap();
    assert_eq!(cards.len(), 5);
    assert_eq!(body["boosters"]["available"], 2);
    assert!(body["opening_id"].is_string());
    for card in cards {
        assert!(card["title"].is_string() && card["page_url"].is_string());
        assert!(["common", "rare", "super_rare", "legendary"]
            .contains(&card["rarity"].as_str().unwrap()));
    }
    // Chaque carte distincte est « nouvelle » exactement une fois, à sa première apparition.
    let new_count = cards.iter().filter(|c| c["is_new"] == true).count();
    let distinct: HashSet<i64> = cards.iter().map(|c| c["id"].as_i64().unwrap()).collect();
    assert_eq!(new_count, distinct.len());
    assert_eq!(owned_total(&pool, user_id).await, 5);

    // Le garanti « rare ou mieux » se retrouve dans la réponse.
    assert!(cards.iter().any(|c| c["rarity"] != "common"));

    // Le second booster ne marque « nouvelles » que les cartes jamais vues.
    let (_, second) = open(&app, &auth, "k2").await;
    for card in second["cards"].as_array().unwrap() {
        let seen_before = cards.iter().any(|c| c["id"] == card["id"]);
        if seen_before {
            assert_eq!(card["is_new"], false);
        }
    }
    assert_eq!(owned_total(&pool, user_id).await, 10);
}

#[sqlx::test(migrations = "../../migrations")]
async fn replaying_the_same_key_returns_the_same_result_without_spending(pool: PgPool) {
    let (user_id, auth) = user(&pool).await;
    seed_cards(&pool, 40).await;
    let app = app(pool.clone());

    let (_, first) = open(&app, &auth, "same").await;
    let (status, replay) = open(&app, &auth, "same").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["opening_id"], replay["opening_id"]);
    assert_eq!(first["cards"], replay["cards"]);
    assert_eq!(stock_of(&pool, user_id).await, 2);
    assert_eq!(owned_total(&pool, user_id).await, 5);
}

#[sqlx::test(migrations = "../../migrations")]
async fn same_key_for_two_players_is_independent(pool: PgPool) {
    let (_, a) = user(&pool).await;
    let (_, b) = user(&pool).await;
    seed_cards(&pool, 40).await;
    let app = app(pool);
    let (_, first) = open(&app, &a, "shared").await;
    let (status, second) = open(&app, &b, "shared").await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(first["opening_id"], second["opening_id"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_empty_stock_is_refused_with_a_conflict(pool: PgPool) {
    let (user_id, auth) = user(&pool).await;
    seed_cards(&pool, 40).await;
    let app = app(pool.clone());
    for i in 0..3 {
        assert_eq!(open(&app, &auth, &format!("k{i}")).await.0, StatusCode::OK);
    }
    let (status, body) = open(&app, &auth, "k3").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "conflict");
    assert_eq!(stock_of(&pool, user_id).await, 0);
    assert_eq!(owned_total(&pool, user_id).await, 15);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_still_works_when_the_stock_is_empty(pool: PgPool) {
    let (_, auth) = user(&pool).await;
    seed_cards(&pool, 40).await;
    let app = app(pool);
    let (_, last) = open(&app, &auth, "k0").await;
    open(&app, &auth, "k1").await;
    open(&app, &auth, "k2").await;
    let (status, replay) = open(&app, &auth, "k0").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(last["cards"], replay["cards"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_openings_never_spend_more_than_the_stock(pool: PgPool) {
    let (user_id, auth) = user(&pool).await;
    seed_cards(&pool, 40).await;
    let app = app(pool.clone());

    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..16 {
        let (app, auth) = (app.clone(), auth.clone());
        tasks.spawn(async move { open(&app, &auth, &format!("race-{i}")).await.0 });
    }
    let mut ok = 0;
    let mut conflict = 0;
    while let Some(status) = tasks.join_next().await {
        match status.unwrap() {
            StatusCode::OK => ok += 1,
            StatusCode::CONFLICT => conflict += 1,
            other => panic!("statut inattendu : {other}"),
        }
    }
    assert_eq!((ok, conflict), (3, 13));
    assert_eq!(stock_of(&pool, user_id).await, 0);
    assert_eq!(owned_total(&pool, user_id).await, 15);
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_replays_of_one_key_spend_a_single_booster(pool: PgPool) {
    let (user_id, auth) = user(&pool).await;
    seed_cards(&pool, 40).await;
    // Ligne de stock déjà committée : sinon la création concurrente de la ligne
    // sérialise les appels à elle seule et masque l'absence de verrou.
    sqlx::query("INSERT INTO booster_stock (user_id, count, refill_anchor) VALUES ($1, 3, now())")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    let app = app(pool.clone());

    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let (app, auth) = (app.clone(), auth.clone());
        tasks.spawn(async move { open(&app, &auth, "double-tap").await });
    }
    let mut ids = HashSet::new();
    while let Some(result) = tasks.join_next().await {
        let (status, body) = result.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        ids.insert(body["opening_id"].as_str().unwrap().to_owned());
    }
    assert_eq!(ids.len(), 1);
    assert_eq!(stock_of(&pool, user_id).await, 2);
    assert_eq!(owned_total(&pool, user_id).await, 5);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_empty_catalog_is_unavailable_and_keeps_the_stock(pool: PgPool) {
    let (_, auth) = user(&pool).await;
    let app = app(pool.clone());
    let (status, body) = open(&app, &auth, "k").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "unavailable");
    let (_, stock) = request(&app, Method::GET, "/me/boosters", Some(&auth), None).await;
    assert_eq!(stock["available"], 3);
}

#[sqlx::test(migrations = "../../migrations")]
async fn stock_recharges_with_time(pool: PgPool) {
    let (user_id, auth) = user(&pool).await;
    seed_cards(&pool, 40).await;
    let app = app(pool.clone());
    open(&app, &auth, "k").await; // stock 2, minuteur lancé maintenant

    let later = Utc::now() + Duration::seconds(25 * 60);
    let info = service::stock_info(&pool, user_id, later).await.unwrap();
    assert_eq!(info.available, 4); // +2 en 25 minutes
    assert!(info.next_at.unwrap() > later);

    let far = Utc::now() + Duration::days(30);
    let info = service::stock_info(&pool, user_id, far).await.unwrap();
    assert_eq!((info.available, info.next_at), (10, None));
}

#[sqlx::test(migrations = "../../migrations")]
async fn opening_after_recharge_uses_the_recharged_stock(pool: PgPool) {
    let (user_id, auth) = user(&pool).await;
    seed_cards(&pool, 40).await;
    let app = app(pool.clone());
    for i in 0..3 {
        open(&app, &auth, &format!("k{i}")).await;
    }
    // Le service accepte l'horloge en paramètre : 20 minutes plus tard, 2 boosters de plus.
    let catalog = crate::boosters::catalog::CatalogCache::default()
        .get(&pool)
        .await
        .unwrap();
    let later = Utc::now() + Duration::seconds(20 * 60 + 5);
    let mut rng = |n: u32| n / 2;
    let opened = service::open(&pool, &catalog, user_id, "later", later, &mut rng)
        .await
        .unwrap();
    assert_eq!(opened.boosters.available, 1);
}
