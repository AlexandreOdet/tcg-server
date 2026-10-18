# Plan de développement du backend

Backend Rust (Axum + sqlx + PostgreSQL) d'un TCG dont les cartes viennent d'articles Wikipédia.
Périmètre v1 : **authentification OIDC** et **boosters**. Duels, échanges, premium et packs viennent après.

## Décisions actées

- Le tirage des boosters se fait **côté serveur** (anti-triche, transaction atomique, probabilités auditables).
- Authentification **OIDC** : Apple en premier fournisseur, les autres s'ajoutent par configuration.
- Modèle `users` / `identities` : une ligne par moyen de connexion, jamais d'identifiant de fournisseur dans `users`.
- Session : access JWT court (15 min) + refresh token opaque, haché en base, avec rotation et révocation de famille.
- Boosters : stock calculé à la demande (pas de cron), ouverture en un seul appel, `Idempotency-Key` obligatoire.
- La source du booster (stock temporisé aujourd'hui, inventaire de packs demain) et la fonction « palier → intervalle et plafond » restent isolées pour brancher le premium plus tard.

## Organisation du workspace

| Crate | Rôle |
|---|---|
| `crates/domain` | Règles métier pures (stock de boosters, rareté, tirage). Aucune dépendance web ni base : testable sans serveur. |
| `crates/api` | Serveur Axum : routes, extracteurs, accès base, auth. |
| `crates/catalog-ingest` | Binaire d'ingestion du catalogue (Wikipédia + Pageviews). |

## Phases

Chaque phase se termine par un point de contrôle vérifiable.

### Phase 0 : Fondations (squelette fourni)

- [x] Workspace Cargo, `docker-compose` Postgres, `.env.example`, CI (fmt, clippy, test).
- [x] Serveur Axum minimal avec `/health`.
- [x] Configuration typée depuis l'environnement.
- [x] Enum d'erreur applicatif (`thiserror`) qui implémente `IntoResponse`.
- [x] `PgPool` dans l'état partagé, connexion et migrations au démarrage.
- [x] Logs structurés (`tracing`) et middleware de trace.

**Point de contrôle** : le serveur démarre, se connecte à Postgres, `/health` répond en vérifiant la base, la CI passe. *(Vérifié en local : 200 base up, 500 base down, arrêt propre sur SIGTERM.)*

### Phase 1 : Catalogue de cartes

- [x] Migration `cards` (index sur `rarity`).
- [x] Ingestion : top mensuel des vues (API Pageviews, 1 000 articles maximum), résumé via l'API REST Wikipédia, concurrence limitée, reprises sur 429/5xx, User-Agent identifiable, upsert sur `(lang, wiki_page_id)`, redirections fusionnées, rareté recalculée sur tout le catalogue. Sans filtre éditorial pour l'instant (voir ci-dessous).
- [x] Fonction pure de rareté par quantiles dans `domain`, avec tests.
- [ ] Lancer l'ingestion réelle depuis un poste ayant accès à Wikimedia (non testé contre les vraies API, voir README).

**Point de contrôle** : catalogue peuplé, répartition des raretés cohérente.

**Décisions** : démarrage sans filtre éditorial ; un fichier d'exclusions (préfixes, titres, expressions régulières) sera ajouté avant la bêta TestFlight, avec une option `--prune` pour retirer les cartes déjà créées. Le top mensuel est limité à 1 000 articles : pour un catalogue plus large, il faudra une autre source (liste de titres, Wikidata).

### Phase 2 : Authentification OIDC

- [ ] Migrations `users`, `identities`, `refresh_tokens`.
- [ ] Trait `TokenVerifier` + implémentation OIDC générique (issuer, JWKS, audience, nonce), cache des clés.
- [ ] `POST /auth/apple`, `POST /auth/refresh`, `POST /auth/logout`, `GET /me`.
- [ ] Access JWT, refresh opaque haché avec rotation, extracteur `AuthUser`.
- [ ] Faux `TokenVerifier` pour les tests d'intégration.

**Point de contrôle** : tests sur token expiré, mauvais `aud`, nonce invalide, rejeu d'un ancien refresh (qui révoque la famille).

### Phase 3 : Boosters

- [ ] Migrations `booster_stock`, `user_cards`, `booster_openings`.
- [ ] Calcul du stock dans `domain` (plafond, plusieurs intervalles, horloge qui recule).
- [ ] Catalogue par rareté chargé en mémoire au démarrage.
- [ ] Tirage pondéré avec générateur aléatoire injectable.
- [ ] `GET /me/boosters`, `POST /boosters/open` (une transaction, verrou de ligne, idempotence).

**Point de contrôle** : test de concurrence, test d'idempotence, simulation de tirages conforme à la configuration.

### Phase 4 : Contrat d'API et robustesse

- [ ] OpenAPI avec `utoipa`, client Swift généré sans retouche.
- [ ] Limitation de débit sur `/auth/*`, timeouts, taille maximale des requêtes.
- [ ] Nettoyage périodique des refresh tokens expirés.

### Phase 5 : Mise en production

- [ ] Image Docker multi-étapes, Postgres avec sauvegardes, secrets, environnements séparés.
- [ ] CI/CD, logs centralisés, alerte sur les 5xx.

## Hors périmètre pour l'instant

Abonnement premium, packs payants, amis et échanges, duels, enchères.

## À trancher avant la phase 3

- Raretés (commune / rare / super rare / légendaire ?) et probabilités.
- Nombre de cartes par booster (5 prévu) et paramètres du palier gratuit (10 min, stock max 10).
