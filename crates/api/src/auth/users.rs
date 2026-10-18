//! Utilisateurs et identités.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::verifier::VerifiedIdentity;

#[derive(Debug, PartialEq, Eq)]
pub struct Login {
    pub user_id: Uuid,
    /// `true` à la toute première connexion (l'app peut alors demander un pseudo).
    pub is_new: bool,
}

/// Retrouve l'utilisateur lié à cette identité, ou le crée à la première connexion.
pub async fn find_or_create(pool: &PgPool, identity: &VerifiedIdentity) -> sqlx::Result<Login> {
    if let Some(user_id) = existing_user(pool, identity).await? {
        return Ok(Login {
            user_id,
            is_new: false,
        });
    }

    let mut tx = pool.begin().await?;
    let user_id: Uuid = sqlx::query_scalar("INSERT INTO users DEFAULT VALUES RETURNING id")
        .fetch_one(&mut *tx)
        .await?;
    let inserted = sqlx::query(
        "INSERT INTO identities (user_id, provider, subject, email, email_verified)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (provider, subject) DO NOTHING",
    )
    .bind(user_id)
    .bind(&identity.provider)
    .bind(&identity.subject)
    .bind(&identity.email)
    .bind(identity.email_verified)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if inserted == 1 {
        tx.commit().await?;
        return Ok(Login {
            user_id,
            is_new: true,
        });
    }

    // Une connexion simultanée a créé l'identité entre-temps : on abandonne notre utilisateur
    // tout neuf (rollback) et on reprend celui qui existe.
    tx.rollback().await?;
    let user_id = existing_user(pool, identity)
        .await?
        .ok_or(sqlx::Error::RowNotFound)?;
    Ok(Login {
        user_id,
        is_new: false,
    })
}

async fn existing_user(pool: &PgPool, identity: &VerifiedIdentity) -> sqlx::Result<Option<Uuid>> {
    // Apple ne fournit l'e-mail qu'à la première connexion : une valeur absente ne doit
    // jamais effacer celle qu'on connaît (COALESCE).
    sqlx::query_scalar(
        "UPDATE identities
         SET email = COALESCE($3, email),
             email_verified = CASE WHEN $3 IS NULL THEN email_verified ELSE $4 END
         WHERE provider = $1 AND subject = $2
         RETURNING user_id",
    )
    .bind(&identity.provider)
    .bind(&identity.subject)
    .bind(&identity.email)
    .bind(identity.email_verified)
    .fetch_optional(pool)
    .await
}

#[derive(Debug, PartialEq, Eq)]
pub struct UserProfile {
    pub id: Uuid,
    pub pseudo: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub async fn get(pool: &PgPool, user_id: Uuid) -> sqlx::Result<Option<UserProfile>> {
    let row: Option<(Uuid, Option<String>, DateTime<Utc>)> =
        sqlx::query_as("SELECT id, pseudo, created_at FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(id, pseudo, created_at)| UserProfile {
        id,
        pseudo,
        created_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(subject: &str, email: Option<&str>) -> VerifiedIdentity {
        VerifiedIdentity {
            provider: "apple".into(),
            subject: subject.into(),
            email: email.map(str::to_owned),
            email_verified: email.is_some(),
        }
    }

    async fn stored_email(pool: &PgPool, subject: &str) -> Option<String> {
        sqlx::query_scalar("SELECT email FROM identities WHERE subject = $1")
            .bind(subject)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn first_login_creates_the_user_then_later_logins_find_it(pool: PgPool) {
        let id = identity("sub-1", Some("a@example.com"));

        let first = find_or_create(&pool, &id).await.unwrap();
        assert!(first.is_new);

        let second = find_or_create(&pool, &id).await.unwrap();
        assert!(!second.is_new);
        assert_eq!(second.user_id, first.user_id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn different_subjects_are_different_users(pool: PgPool) {
        let a = find_or_create(&pool, &identity("sub-a", None))
            .await
            .unwrap();
        let b = find_or_create(&pool, &identity("sub-b", None))
            .await
            .unwrap();
        assert_ne!(a.user_id, b.user_id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn the_same_subject_at_another_provider_is_another_identity(pool: PgPool) {
        let apple = find_or_create(&pool, &identity("same", None))
            .await
            .unwrap();
        let google = VerifiedIdentity {
            provider: "google".into(),
            ..identity("same", None)
        };
        let other = find_or_create(&pool, &google).await.unwrap();
        assert_ne!(apple.user_id, other.user_id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_missing_email_never_erases_the_known_one(pool: PgPool) {
        find_or_create(&pool, &identity("sub-1", Some("a@example.com")))
            .await
            .unwrap();
        // Apple n'envoie plus l'e-mail aux connexions suivantes.
        find_or_create(&pool, &identity("sub-1", None))
            .await
            .unwrap();
        assert_eq!(
            stored_email(&pool, "sub-1").await.as_deref(),
            Some("a@example.com")
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_new_email_replaces_the_old_one(pool: PgPool) {
        find_or_create(&pool, &identity("sub-1", Some("a@example.com")))
            .await
            .unwrap();
        find_or_create(&pool, &identity("sub-1", Some("b@example.com")))
            .await
            .unwrap();
        assert_eq!(
            stored_email(&pool, "sub-1").await.as_deref(),
            Some("b@example.com")
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn simultaneous_first_logins_create_a_single_user(pool: PgPool) {
        let id = identity("sub-race", Some("a@example.com"));

        // Beaucoup de premières connexions simultanées : voir le test équivalent dans `sessions`.
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let (pool, id) = (pool.clone(), id.clone());
            tasks.spawn(async move { find_or_create(&pool, &id).await.unwrap() });
        }
        let mut logins = Vec::new();
        while let Some(login) = tasks.join_next().await {
            logins.push(login.unwrap());
        }

        assert!(logins.iter().all(|l| l.user_id == logins[0].user_id));
        assert_eq!(logins.iter().filter(|l| l.is_new).count(), 1);
        let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            users, 1,
            "les utilisateurs abandonnés doivent avoir été annulés"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn get_returns_the_profile_or_none(pool: PgPool) {
        let login = find_or_create(&pool, &identity("sub-1", None))
            .await
            .unwrap();

        let profile = get(&pool, login.user_id).await.unwrap().unwrap();
        assert_eq!(profile.id, login.user_id);
        assert_eq!(profile.pseudo, None);
        assert!(get(&pool, Uuid::new_v4()).await.unwrap().is_none());
    }
}
