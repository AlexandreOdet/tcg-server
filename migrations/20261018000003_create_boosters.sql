-- Boosters : stock par joueur, collection, historique d'ouvertures.

-- Stock « paresseux » : `refill_anchor` est l'instant de départ du minuteur de recharge.
-- Le stock réel se recalcule à la lecture (voir crate `domain`, module `boosters`).
CREATE TABLE booster_stock (
    user_id       UUID        PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    count         INTEGER     NOT NULL CHECK (count >= 0),
    refill_anchor TIMESTAMPTZ NOT NULL
);

-- Collection : une ligne par carte possédée, avec le nombre d'exemplaires.
CREATE TABLE user_cards (
    user_id           UUID        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    card_id           BIGINT      NOT NULL REFERENCES cards (id),
    quantity          INTEGER     NOT NULL CHECK (quantity > 0),
    first_obtained_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, card_id)
);

-- Historique et idempotence : rejouer la même `Idempotency-Key` renvoie le même tirage.
CREATE TABLE booster_openings (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    idempotency_key TEXT        NOT NULL,
    card_ids        BIGINT[]    NOT NULL,
    new_card_ids    BIGINT[]    NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, idempotency_key)
);
