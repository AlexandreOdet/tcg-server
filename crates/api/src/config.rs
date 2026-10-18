use serde::Deserialize;

/// Configuration du serveur, lue depuis les variables d'environnement
/// (`DATABASE_URL`, `BIND_ADDR`, `JWT_SECRET`, `APPLE_BUNDLE_ID`).
#[derive(Clone, Deserialize)]
pub struct Config {
    pub database_url: String,
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
    /// Secret de signature des access tokens (32 caractères minimum, vérifié au démarrage).
    pub jwt_secret: String,
    /// Identifiant de bundle de l'app iOS : valeur attendue dans l'audience des jetons Apple.
    pub apple_bundle_id: String,
}

fn default_bind_addr() -> String {
    "0.0.0.0:8080".to_owned()
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(envy::from_env()?)
    }
}

// Pas de `Debug` dérivé : l'URL de la base et le secret JWT ne doivent pas finir dans les logs.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("database_url", &"<masquée>")
            .field("bind_addr", &self.bind_addr)
            .field("jwt_secret", &"<masqué>")
            .field("apple_bundle_id", &self.apple_bundle_id)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRED: [(&str, &str); 3] = [
        ("database_url", "postgres://user:motdepasse@host/db"),
        ("jwt_secret", "un-secret-de-test-d-au-moins-32-caracteres"),
        ("apple_bundle_id", "fr.exemple.tcg"),
    ];

    fn from_pairs(pairs: &[(&str, &str)]) -> Result<Config, envy::Error> {
        envy::from_iter(pairs.iter().map(|(k, v)| (k.to_uppercase(), v.to_string())))
    }

    fn with(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        REQUIRED.iter().chain(extra).copied().collect()
    }

    #[test]
    fn bind_addr_has_a_default() {
        let config = from_pairs(&REQUIRED).unwrap();
        assert_eq!(config.bind_addr, "0.0.0.0:8080");
    }

    #[test]
    fn bind_addr_can_be_overridden() {
        let config = from_pairs(&with(&[("bind_addr", "127.0.0.1:3000")])).unwrap();
        assert_eq!(config.bind_addr, "127.0.0.1:3000");
    }

    #[test]
    fn every_required_variable_is_enforced() {
        for missing in REQUIRED.map(|(name, _)| name) {
            let pairs: Vec<_> = REQUIRED
                .iter()
                .filter(|(n, _)| *n != missing)
                .copied()
                .collect();
            assert!(
                from_pairs(&pairs).is_err(),
                "{missing} devrait être obligatoire"
            );
        }
    }

    #[test]
    fn debug_output_hides_secrets() {
        let output = format!("{:?}", from_pairs(&REQUIRED).unwrap());
        assert!(!output.contains("motdepasse"));
        assert!(!output.contains("un-secret-de-test"));
        assert!(output.contains("fr.exemple.tcg"));
    }
}
