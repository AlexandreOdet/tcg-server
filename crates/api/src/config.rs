use serde::Deserialize;

/// Configuration du serveur, lue depuis les variables d'environnement
/// (`DATABASE_URL`, `BIND_ADDR`).
#[derive(Clone, Deserialize)]
pub struct Config {
    #[expect(dead_code, reason = "utilisé à l'étape 4 (connexion Postgres)")]
    pub database_url: String,
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
}

fn default_bind_addr() -> String {
    "0.0.0.0:8080".to_owned()
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(envy::from_env()?)
    }
}

// Pas de `Debug` dérivé : `database_url` contient un mot de passe et ne doit pas finir dans les logs.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("database_url", &"<masquée>")
            .field("bind_addr", &self.bind_addr)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_pairs(pairs: &[(&str, &str)]) -> Result<Config, envy::Error> {
        envy::from_iter(pairs.iter().map(|(k, v)| (k.to_uppercase(), v.to_string())))
    }

    #[test]
    fn bind_addr_has_a_default() {
        let config = from_pairs(&[("database_url", "postgres://x")]).unwrap();
        assert_eq!(config.bind_addr, "0.0.0.0:8080");
    }

    #[test]
    fn bind_addr_can_be_overridden() {
        let config = from_pairs(&[
            ("database_url", "postgres://x"),
            ("bind_addr", "127.0.0.1:3000"),
        ])
        .unwrap();
        assert_eq!(config.bind_addr, "127.0.0.1:3000");
    }

    #[test]
    fn database_url_is_required() {
        assert!(from_pairs(&[]).is_err());
    }

    #[test]
    fn debug_output_hides_the_database_url() {
        let config = from_pairs(&[("database_url", "postgres://user:secret@host/db")]).unwrap();
        assert!(!format!("{config:?}").contains("secret"));
    }
}
