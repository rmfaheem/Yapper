use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipSeed {
    pub endpoint: String,
    pub port: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub cluster: bool,
    pub gossip_seed: Vec<GossipSeed>,
    pub tls: bool,
    pub tls_verify_cert: bool,
    #[serde(default)]
    pub root_ca_path: String,
    pub node_preference: String,
    pub username: String,
    pub password: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            cluster: false,
            gossip_seed: vec![GossipSeed {
                endpoint: "127.0.0.1".to_string(),
                port: "2113".to_string(),
            }],
            tls: false,
            tls_verify_cert: false,
            root_ca_path: String::new(),
            node_preference: "random".to_string(),
            username: "admin".to_string(),
            password: "changeit".to_string(),
        }
    }
}

impl Config {
    /// Build the KurrentDB connection string from the config.
    ///
    /// Mirrors the Go `BuildConnectionString`, but uses the `kurrentdb` scheme
    /// (`kurrentdb+discover` for clusters).
    pub fn build_connection_string(&self) -> String {
        let mut s = String::from("kurrentdb");
        if self.cluster {
            s.push_str("+discover");
        }
        s.push_str("://");
        s.push_str(&self.username);
        s.push(':');
        s.push_str(&self.password);
        s.push('@');

        for (i, node) in self.gossip_seed.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&node.endpoint);
            s.push(':');
            s.push_str(&node.port);
        }

        // Query params are separated by '&' (URI query style).
        let mut opts: Vec<String> = Vec::new();
        // TLS is on by default in the scheme, so we only emit tls=false.
        if !self.tls {
            opts.push("tls=false".to_string());
        }
        if self.tls && !self.tls_verify_cert {
            opts.push("tlsVerifyCert=false".to_string());
        }
        if self.tls && !self.root_ca_path.is_empty() {
            opts.push(format!("tlsCaFile={}", self.root_ca_path));
        }
        if !self.node_preference.is_empty() {
            opts.push(format!("nodePreference={}", self.node_preference));
        }

        if !opts.is_empty() {
            s.push('?');
            s.push_str(&opts.join("&"));
        }
        s
    }

    /// HTTP `/stats` URL for the first gossip seed, used by the server-stats poller.
    pub fn http_stats_url(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        let node = self
            .gossip_seed
            .first()
            .map(|s| format!("{}:{}", s.endpoint, s.port))
            .unwrap_or_else(|| "127.0.0.1:2113".to_string());
        format!("{scheme}://{node}/stats")
    }
}

/// Default config path: `$HOME/.yapper.json`.
pub fn default_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not determine home directory")?;
    Ok(home.join(".yapper.json"))
}

/// Load the config from `path`, or create a default one there if it does not exist.
///
/// Mirrors the Go `initConfig` flow.
pub fn load_or_create(path: &Path) -> Result<Config> {
    if path.exists() {
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        let config: Config = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing config file {}", path.display()))?;
        Ok(config)
    } else {
        println!(
            "No config file found. Creating default config file at: {}",
            path.display()
        );
        let config = Config::default();
        let json = serde_json::to_vec_pretty(&config)?;
        std::fs::write(path, json)
            .with_context(|| format!("writing default config to {}", path.display()))?;
        Ok(config)
    }
}

/// Resolve the config path from an optional `--config` override, then load/create it.
pub fn load(config_flag: Option<&Path>) -> Result<Config> {
    let path = match config_flag {
        Some(p) => {
            println!("Using config file: {}", p.display());
            p.to_path_buf()
        }
        None => default_config_path()?,
    };
    load_or_create(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insecure_connection_string() {
        let c = Config::default();
        assert_eq!(
            c.build_connection_string(),
            "kurrentdb://admin:changeit@127.0.0.1:2113?tls=false&nodePreference=random"
        );
    }

    #[test]
    fn cluster_multiple_seeds_tls_no_verify() {
        let c = Config {
            cluster: true,
            tls: true,
            tls_verify_cert: false,
            node_preference: "leader".to_string(),
            gossip_seed: vec![
                GossipSeed { endpoint: "a".to_string(), port: "1".to_string() },
                GossipSeed { endpoint: "b".to_string(), port: "2".to_string() },
            ],
            ..Config::default()
        };
        assert_eq!(
            c.build_connection_string(),
            "kurrentdb+discover://admin:changeit@a:1,b:2?tlsVerifyCert=false&nodePreference=leader"
        );
    }

    #[test]
    fn tls_with_root_ca() {
        let c = Config {
            tls: true,
            tls_verify_cert: true,
            root_ca_path: "/certs/ca.pem".to_string(),
            node_preference: String::new(),
            ..Config::default()
        };
        assert_eq!(
            c.build_connection_string(),
            "kurrentdb://admin:changeit@127.0.0.1:2113?tlsCaFile=/certs/ca.pem"
        );
    }

    #[test]
    fn tls_verified_no_node_pref_has_no_query() {
        let c = Config {
            tls: true,
            tls_verify_cert: true,
            node_preference: String::new(),
            ..Config::default()
        };
        assert_eq!(
            c.build_connection_string(),
            "kurrentdb://admin:changeit@127.0.0.1:2113"
        );
    }

    #[test]
    fn http_stats_url_switches_scheme_on_tls() {
        let mut c = Config::default();
        assert_eq!(c.http_stats_url(), "http://127.0.0.1:2113/stats");
        c.tls = true;
        assert_eq!(c.http_stats_url(), "https://127.0.0.1:2113/stats");
    }

    #[test]
    fn deserializes_and_round_trips_legacy_json_keys() {
        let json = r#"{
            "cluster": false,
            "gossipSeed": [{"endpoint":"10.0.0.1","port":"2113"}],
            "tls": true,
            "tlsVerifyCert": false,
            "rootCaPath": "/x.pem",
            "nodePreference": "follower",
            "username": "u",
            "password": "p"
        }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.gossip_seed[0].endpoint, "10.0.0.1");
        assert!(c.tls);
        assert!(!c.tls_verify_cert);
        assert_eq!(c.root_ca_path, "/x.pem");
        assert_eq!(c.node_preference, "follower");

        let back = serde_json::to_string(&c).unwrap();
        assert!(back.contains("\"gossipSeed\""));
        assert!(back.contains("\"tlsVerifyCert\""));
        assert!(back.contains("\"nodePreference\""));
        assert!(back.contains("\"rootCaPath\""));
    }

    #[test]
    fn root_ca_path_defaults_when_absent() {
        // The Go default config omits rootCaPath; it must deserialize via #[serde(default)].
        let json = r#"{
            "cluster": false,
            "gossipSeed": [{"endpoint":"127.0.0.1","port":"2113"}],
            "tls": false,
            "tlsVerifyCert": false,
            "nodePreference": "random",
            "username": "admin",
            "password": "changeit"
        }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.root_ca_path, "");
    }
}
