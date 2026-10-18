-- Authentification : un utilisateur peut avoir plusieurs moyens de connexion (identités).

CREATE TABLE users (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Choisi par le joueur après l'inscription (Apple ne fournit pas toujours le nom).
    pseudo     TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Une ligne par moyen de connexion. `subject` est l'identifiant stable chez le fournisseur
-- (claim `sub`) : jamais l'e-mail, qui peut être un relais privé ou absent.
CREATE TABLE identities (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id        UUID        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    provider       TEXT        NOT NULL,
    subject        TEXT        NOT NULL,
    email          TEXT,
    email_verified BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (provider, subject)
);

CREATE INDEX identities_user_id_idx ON identities (user_id);

-- Refresh tokens opaques : seul le hash SHA-256 est stocké.
-- Rotation : chaque usage marque le jeton `used_at` et en émet un nouveau dans la même famille.
-- Un jeton déjà utilisé qui revient = vol probable : toute la famille est révoquée.
CREATE TABLE refresh_tokens (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    family_id  UUID        NOT NULL,
    token_hash BYTEA       NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    used_at    TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);

CREATE INDEX refresh_tokens_family_id_idx ON refresh_tokens (family_id);
CREATE INDEX refresh_tokens_user_id_idx ON refresh_tokens (user_id);
-- Nettoyage périodique des jetons expirés.
CREATE INDEX refresh_tokens_expires_at_idx ON refresh_tokens (expires_at);
