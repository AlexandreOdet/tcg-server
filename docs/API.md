# Contrat d'API (v1 : authentification et boosters)

Toutes les requêtes et réponses sont en JSON. Les routes protégées demandent
`Authorization: Bearer <access_token>`.

> Ce contrat est écrit à la main en attendant l'OpenAPI généré (phase 4). Il décrit ce que le
> serveur fait aujourd'hui.

## Format d'erreur

Toute erreur applicative a le même corps :

```json
{ "error": { "code": "unauthorized", "message": "non authentifié" } }
```

| Statut | `code` | Quand |
|---|---|---|
| 400 | `bad_request` | Champ obligatoire vide |
| 401 | `unauthorized` | Jeton absent, invalide, expiré, révoqué, ou connexion Apple refusée |
| 404 | `not_found` | Ressource inexistante (pas encore utilisé) |
| 409 | `conflict` | Aucun booster disponible |
| 503 | `unavailable` | Catalogue de cartes vide (ingestion non lancée) |
| 500 | `internal` | Erreur serveur ; aucun détail n'est renvoyé |

Le `code` est stable : l'app peut faire un `switch` dessus. Le `message` est destiné aux logs, pas à
l'utilisateur.

⚠️ Un corps JSON mal formé (ou un champ manquant) est refusé par le framework avec un statut 4xx
**sans** ce format (texte brut). L'app ne doit pas supposer que tout 4xx contient un `error`.

## `POST /auth/{provider}`

`provider` vaut `apple` ou `google`. Un fournisseur non configuré sur le serveur répond `404 not_found`.
Échange un jeton d'identité OIDC contre une session.

```json
{ "identity_token": "<JWT renvoyé par le fournisseur>", "nonce": "<nonce brut>" }
```

**Nonce** : l'app génère une chaîne aléatoire (le *nonce brut*) et envoie ici la valeur **brute**.
- Apple : envoyer à Apple le **SHA-256 en hexadécimal minuscule** (`request.nonce`).
- Google : envoyer à Google le nonce brut (paramètre `nonce:` de GoogleSignIn) ; Google le recopie dans le jeton.

Un même `sub` chez deux fournisseurs donne deux identités distinctes, donc deux joueurs distincts
(pas de liaison automatique de comptes pour l'instant).

Réponse `200` :

```json
{
  "access_token": "<JWT, 15 min>",
  "refresh_token": "<opaque, 60 jours, à usage unique>",
  "token_type": "Bearer",
  "expires_in": 900,
  "is_new_user": true
}
```

`is_new_user` vaut `true` à la toute première connexion : l'app peut alors proposer de choisir un pseudo.

## `POST /auth/refresh`

```json
{ "refresh_token": "<refresh token courant>" }
```

Réponse `200` : même forme que `/auth/{provider}` (`is_new_user` est toujours `false`).

**Règles à respecter côté app :**

- Le refresh token est **à usage unique** : chaque appel en renvoie un nouveau, à stocker immédiatement
  (Keychain) à la place de l'ancien.
- **Un seul refresh à la fois.** Si plusieurs requêtes reçoivent un 401 en même temps, une seule appelle
  `/auth/refresh` et les autres attendent son résultat.
- Rejouer un ancien refresh token (plus de 10 s après son usage) révoque **toute la session** : l'app
  reçoit alors 401 partout et doit renvoyer l'utilisateur vers l'écran de connexion.
- `401` sur `/auth/refresh` = session terminée : repasser par Sign in with Apple.

## `POST /auth/logout`

```json
{ "refresh_token": "<refresh token courant>" }
```

Réponse `204`, sans corps. Idempotent : un jeton inconnu ou déjà révoqué répond aussi `204`.
L'access token en cours reste valide jusqu'à son expiration (15 min maximum).

## `GET /me` (protégée)

Réponse `200` :

```json
{
  "id": "8f0c2a55-1f6f-4a53-9d8e-6c1b5f0b1f10",
  "pseudo": null,
  "created_at": "2026-10-18T15:19:13.563255Z"
}
```

`pseudo` est `null` tant que le joueur n'en a pas choisi (la route pour le définir n'existe pas encore).

## `GET /me/boosters` (protégée)

```json
{ "available": 3, "max": 10, "next_at": "2026-10-18T15:30:00Z", "interval_secs": 600 }
```

Un nouveau joueur démarre avec 3 boosters. Palier gratuit : +1 booster toutes les 10 minutes,
dans la limite de 10 en stock. `next_at` vaut `null` quand le stock est plein (le minuteur est
suspendu). Le stock se recalcule à chaque appel : aucun traitement planifié.

## `POST /boosters/open` (protégée)

En-tête **obligatoire** : `Idempotency-Key: <chaîne unique par ouverture>` (1 à 128 caractères ASCII,
par exemple un UUID généré par l'app **avant** l'envoi).

Réponse `200` :

```json
{
  "opening_id": "…",
  "cards": [
    {
      "id": 42, "title": "Paris", "extract": "…", "page_url": "https://fr.wikipedia.org/wiki/Paris",
      "image_url": "https://…", "rarity": "rare", "is_new": true
    }
  ],
  "boosters": { "available": 2, "max": 10, "next_at": "…", "interval_secs": 600 }
}
```

- 5 cartes, **tirées par le serveur** avant toute animation. Doublons possibles ; la dernière carte
  est garantie « rare » ou mieux. `rarity` : `common`, `rare`, `super_rare`, `legendary`.
- Taux par emplacement : 1 % légendaire, 4 % super rare, 20 % rare, 75 % commune.
- `is_new` : la carte n'était pas dans la collection du joueur (une seule fois par carte et par booster).
- **Idempotence** : renvoyer la même clé (coup de réseau perdu, double appui) renvoie **le même tirage**
  sans consommer de booster. Une nouvelle ouverture exige une nouvelle clé. Un rejeu fonctionne
  même quand le stock est vide.
- `409 conflict` : plus de booster. `400` : clé absente ou invalide. `503` : catalogue vide.

## `GET /health`

`200 {"status":"ok"}` si le serveur et la base répondent, `500` sinon. Sans authentification.

## Identifiants

- `id` des utilisateurs : UUID v4.
- Dates : RFC 3339, UTC.
