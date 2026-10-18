//! Refresh tokens : création, rotation avec détection de réutilisation, révocation.

use chrono::{DateTime, Duration, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::tokens::{hash_refresh_token, new_refresh_token};

/// Durée de vie d'un refresh token (renouvelée à chaque rotation).
pub const REFRESH_TTL_DAYS: i64 = 60;

/// Un jeton déjà utilisé qui revient dans ce délai est traité comme une course légitime
/// (deux requêtes simultanées de la même app) : refusé, mais sans révoquer la session.
/// Au-delà, c'est une réutilisation suspecte et toute la famille est révoquée.
const REUSE_GRACE_SECS: i64 = 10;

#[derive(Debug, thiserror::Error)]
pub enum RefreshError {
    #[error("refresh token invalide ou expiré")]
    Invalid,
    #[error("refresh token réutilisé : session révoquée")]
    Reused,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Ouvre une nouvelle session (nouvelle famille de refresh tokens) et renvoie le jeton.
pub async fn start_session(
    conn: &mut PgConnection,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> sqlx::Result<String> {
    insert_token(conn, user_id, Uuid::new_v4(), now).await
}

async fn insert_token(
    conn: &mut PgConnection,
    user_id: Uuid,
    family_id: Uuid,
    now: DateTime<Utc>,
) -> sqlx::Result<String> {
    let token = new_refresh_token();
    sqlx::query(
        "INSERT INTO refresh_tokens (user_id, family_id, token_hash, created_at, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(family_id)
    .bind(&hash_refresh_token(&token)[..])
    .bind(now)
    .bind(now + Duration::days(REFRESH_TTL_DAYS))
    .execute(conn)
    .await?;
    Ok(token)
}

type TokenRow = (
    Uuid,                  // id
    Uuid,                  // user_id
    Uuid,                  // family_id
    DateTime<Utc>,         // expires_at
    Option<DateTime<Utc>>, // used_at
    Option<DateTime<Utc>>, // revoked_at
);

/// Échange un refresh token contre un nouveau (même famille). L'ancien devient inutilisable.
/// Renvoie l'utilisateur et le nouveau refresh token.
pub async fn rotate(
    pool: &PgPool,
    presented: &str,
    now: DateTime<Utc>,
) -> Result<(Uuid, String), RefreshError> {
    let mut tx = pool.begin().await?;

    // Le verrou de ligne sérialise deux rotations simultanées du même jeton.
    let row: Option<TokenRow> = sqlx::query_as(
        "SELECT id, user_id, family_id, expires_at, used_at, revoked_at
         FROM refresh_tokens WHERE token_hash = $1 FOR UPDATE",
    )
    .bind(&hash_refresh_token(presented)[..])
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, user_id, family_id, expires_at, used_at, revoked_at)) = row else {
        return Err(RefreshError::Invalid);
    };

    if revoked_at.is_some() {
        return Err(RefreshError::Invalid);
    }
    if let Some(used_at) = used_at {
        if now - used_at <= Duration::seconds(REUSE_GRACE_SECS) {
            return Err(RefreshError::Invalid);
        }
        // Réutilisation d'un ancien jeton : on coupe toute la famille, et on valide
        // cette révocation (pas de rollback) avant de refuser.
        sqlx::query(
            "UPDATE refresh_tokens SET revoked_at = $2
             WHERE family_id = $1 AND revoked_at IS NULL",
        )
        .bind(family_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Err(RefreshError::Reused);
    }
    if expires_at <= now {
        return Err(RefreshError::Invalid);
    }

    sqlx::query("UPDATE refresh_tokens SET used_at = $2 WHERE id = $1")
        .bind(id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    let new_token = insert_token(&mut tx, user_id, family_id, now).await?;
    tx.commit().await?;
    Ok((user_id, new_token))
}

/// Termine la session à laquelle appartient ce refresh token (déconnexion).
/// Idempotent : un jeton inconnu n'est pas une erreur.
pub async fn revoke_session(
    pool: &PgPool,
    presented: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE refresh_tokens SET revoked_at = $2
         WHERE revoked_at IS NULL AND family_id IN (
             SELECT family_id FROM refresh_tokens WHERE token_hash = $1)",
    )
    .bind(&hash_refresh_token(presented)[..])
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn new_user(pool: &PgPool) -> Uuid {
        sqlx::query_scalar("INSERT INTO users DEFAULT VALUES RETURNING id")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn session(pool: &PgPool, user: Uuid, now: DateTime<Utc>) -> String {
        let mut conn = pool.acquire().await.unwrap();
        start_session(&mut conn, user, now).await.unwrap()
    }

    fn later(now: DateTime<Utc>, seconds: i64) -> DateTime<Utc> {
        now + Duration::seconds(seconds)
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rotation_returns_a_new_working_token_for_the_same_user(pool: PgPool) {
        let (user, now) = (new_user(&pool).await, Utc::now());
        let first = session(&pool, user, now).await;

        let (rotated_user, second) = rotate(&pool, &first, later(now, 60)).await.unwrap();
        assert_eq!(rotated_user, user);
        assert_ne!(first, second);

        let (_, third) = rotate(&pool, &second, later(now, 120)).await.unwrap();
        assert_ne!(second, third);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn only_the_hash_is_stored(pool: PgPool) {
        let user = new_user(&pool).await;
        let token = session(&pool, user, Utc::now()).await;

        let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM refresh_tokens")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, hash_refresh_token(&token));
        assert!(!stored
            .windows(token.len().min(8))
            .any(|w| w == token.as_bytes()));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn an_unknown_token_is_invalid(pool: PgPool) {
        let result = rotate(&pool, "inconnu", Utc::now()).await;
        assert!(matches!(result, Err(RefreshError::Invalid)));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn an_expired_token_is_invalid(pool: PgPool) {
        let (user, now) = (new_user(&pool).await, Utc::now());
        let token = session(&pool, user, now).await;

        let after_expiry = now + Duration::days(REFRESH_TTL_DAYS) + Duration::seconds(1);
        let result = rotate(&pool, &token, after_expiry).await;
        assert!(matches!(result, Err(RefreshError::Invalid)));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn replaying_an_old_token_revokes_the_whole_session(pool: PgPool) {
        let (user, now) = (new_user(&pool).await, Utc::now());
        let first = session(&pool, user, now).await;
        let (_, second) = rotate(&pool, &first, later(now, 60)).await.unwrap();

        // Plus tard, quelqu'un rejoue le premier jeton (déjà utilisé depuis longtemps).
        let replay = rotate(&pool, &first, later(now, 3600)).await;
        assert!(matches!(replay, Err(RefreshError::Reused)));

        // Le jeton légitime le plus récent est lui aussi révoqué.
        let result = rotate(&pool, &second, later(now, 3601)).await;
        assert!(matches!(result, Err(RefreshError::Invalid)));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_quick_replay_is_refused_without_killing_the_session(pool: PgPool) {
        let (user, now) = (new_user(&pool).await, Utc::now());
        let first = session(&pool, user, now).await;
        let (_, second) = rotate(&pool, &first, later(now, 60)).await.unwrap();

        // Deuxième requête partie en même temps que la première (course légitime).
        let replay = rotate(&pool, &first, later(now, 62)).await;
        assert!(matches!(replay, Err(RefreshError::Invalid)));

        // La session reste utilisable avec le jeton renvoyé par la première requête.
        assert!(rotate(&pool, &second, later(now, 63)).await.is_ok());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn sessions_of_the_same_user_are_independent(pool: PgPool) {
        let (user, now) = (new_user(&pool).await, Utc::now());
        let phone = session(&pool, user, now).await;
        let tablet = session(&pool, user, now).await;

        revoke_session(&pool, &phone, now).await.unwrap();

        assert!(rotate(&pool, &phone, later(now, 60)).await.is_err());
        assert!(rotate(&pool, &tablet, later(now, 60)).await.is_ok());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn logout_revokes_every_token_of_the_session_and_is_idempotent(pool: PgPool) {
        let (user, now) = (new_user(&pool).await, Utc::now());
        let first = session(&pool, user, now).await;
        let (_, second) = rotate(&pool, &first, later(now, 60)).await.unwrap();

        revoke_session(&pool, &second, later(now, 61))
            .await
            .unwrap();
        revoke_session(&pool, &second, later(now, 62))
            .await
            .unwrap();
        revoke_session(&pool, "inconnu", later(now, 62))
            .await
            .unwrap();

        assert!(rotate(&pool, &second, later(now, 120)).await.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn simultaneous_rotations_of_one_token_yield_exactly_one_success(pool: PgPool) {
        let (user, now) = (new_user(&pool).await, Utc::now());
        let token = session(&pool, user, now).await;
        let at = later(now, 60);

        // Beaucoup de rotations en parallèle sur des connexions distinctes : avec deux seulement,
        // la fenêtre de course est trop étroite pour qu'un verrou manquant se remarque.
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let (pool, token) = (pool.clone(), token.clone());
            tasks.spawn(async move { rotate(&pool, &token, at).await.is_ok() });
        }
        let mut successes = 0;
        while let Some(result) = tasks.join_next().await {
            successes += usize::from(result.unwrap());
        }

        assert_eq!(successes, 1);
    }
}
