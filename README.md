# tcg-server

Backend Rust d'un jeu de cartes à collectionner dont les cartes sont des articles Wikipédia.

Stack : Axum, sqlx, PostgreSQL. Le plan de développement est dans [`docs/PLAN.md`](docs/PLAN.md).

## Démarrage

```bash
cp .env.example .env
docker compose up -d db
cargo run -p api
curl localhost:8080/health
```

## Ingestion du catalogue

```bash
# Importe le top des 1 000 articles les plus lus du mois précédent (nécessite l'accès à Wikimedia)
cargo run -p catalog-ingest

# Options : un mois précis, moins d'articles, User-Agent avec votre contact
cargo run -p catalog-ingest -- --month 2026-09 --limit 200
INGEST_USER_AGENT="tcg-server-ingest/0.1 (vous@exemple.fr)" cargo run -p catalog-ingest
```

L'ingestion est relançable sans doublons. La rareté (plus un article est lu, plus la carte est rare)
est recalculée sur tout le catalogue à chaque exécution.

## Organisation

- `crates/domain` : règles métier pures (sans web ni base)
- `crates/api` : serveur Axum
- `crates/catalog-ingest` : ingestion du catalogue de cartes
- `migrations/` : migrations SQL (`sqlx migrate`)

## Contrôles avant commit

Les tests de base de données ont besoin de Postgres (`docker compose up -d db`) et de `DATABASE_URL`.

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
