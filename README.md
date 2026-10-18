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

## Organisation

- `crates/domain` : règles métier pures (sans web ni base)
- `crates/api` : serveur Axum
- `crates/catalog-ingest` : ingestion du catalogue de cartes
- `migrations/` : migrations SQL (`sqlx migrate`)

## Contrôles avant commit

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
