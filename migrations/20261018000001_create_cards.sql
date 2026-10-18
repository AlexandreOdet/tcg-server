-- Catalogue de cartes : une carte par article Wikipédia.

CREATE TYPE rarity AS ENUM ('common', 'rare', 'super_rare', 'legendary');

CREATE TABLE cards (
    id               BIGSERIAL PRIMARY KEY,
    lang             TEXT        NOT NULL DEFAULT 'fr',
    wiki_page_id     BIGINT      NOT NULL,
    title            TEXT        NOT NULL,
    extract          TEXT        NOT NULL,
    -- Lien vers l'article : requis pour l'attribution CC BY-SA.
    page_url         TEXT        NOT NULL,
    image_url        TEXT,
    avg_daily_views  INTEGER     NOT NULL CHECK (avg_daily_views >= 0),
    rarity           rarity      NOT NULL,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Clé de l'upsert de l'ingestion : permet de la relancer sans doublons.
    UNIQUE (lang, wiki_page_id)
);

-- Chargement du catalogue par rareté (tirage des boosters).
CREATE INDEX cards_rarity_idx ON cards (rarity);
