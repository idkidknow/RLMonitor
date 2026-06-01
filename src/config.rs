use anyhow::{Context, Result, ensure};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
};

pub struct Config {
    pub realitylink_address: String,
    pub http_url: url::Url,
    pub ws_url: String,
    pub listen_addr: SocketAddr,
    pub database_path: PathBuf,
    pub frontend_dir: PathBuf,
    pub server_name: String,
    pub retention_days: u32,
    pub log_filter: String,
}

impl Config {
    pub fn load() -> Result<Self> {
        Self::from_sources(Path::new(".env"), std::env::vars())
    }

    fn from_sources(
        path: &Path,
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self> {
        // Read without mutating the process environment (Rust 2024 makes set_var unsafe).
        let mut values: HashMap<String, String> = match dotenvy::from_path_iter(path) {
            Ok(entries) => entries
                .collect::<Result<_, _>>()
                .context("Cannot parse .env")?,
            Err(dotenvy::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                HashMap::new()
            }
            Err(error) => return Err(error).context("Cannot read .env"),
        };
        values.extend(environment);
        ensure!(
            values.contains_key("REALITYLINK_ADDRESS")
                || !values.contains_key("REALITYLINK_WS_URL"),
            "REALITYLINK_WS_URL has been replaced by REALITYLINK_ADDRESS=host:port"
        );
        let value =
            |key: &str, default: &str| values.get(key).cloned().unwrap_or_else(|| default.into());
        let realitylink_address = value("REALITYLINK_ADDRESS", "localhost:39244");
        let (http_url, ws_url) = endpoints(&realitylink_address)?;
        let retention_days = value("RETENTION_DAYS", "3")
            .parse()
            .context("RETENTION_DAYS must be an integer")?;
        ensure!(
            (1..=3650).contains(&retention_days),
            "RETENTION_DAYS must be between 1 and 3650"
        );
        Ok(Self {
            realitylink_address,
            http_url,
            ws_url,
            listen_addr: value("LISTEN_ADDR", "127.0.0.1:3000")
                .parse()
                .context("Invalid LISTEN_ADDR")?,
            database_path: value("DATABASE_PATH", "chat_history.db").into(),
            frontend_dir: value("FRONTEND_DIR", "frontend/dist").into(),
            server_name: value("SERVER_NAME", "Minecraft"),
            retention_days,
            log_filter: value("RUST_LOG", "info"),
        })
    }
}

fn endpoints(address: &str) -> Result<(url::Url, String)> {
    ensure!(
        !address
            .chars()
            .any(|c| c.is_whitespace() || "/?#@".contains(c)),
        "REALITYLINK_ADDRESS must be host:port (IPv6: [address]:port), without a scheme or path"
    );
    let port: u16 = address
        .rsplit_once(':')
        .context("REALITYLINK_ADDRESS requires a port")?
        .1
        .parse()
        .context("Invalid RealityLink port")?;
    ensure!(port != 0, "RealityLink port must be between 1 and 65535");
    let http_url =
        url::Url::parse(&format!("http://{address}/")).context("Invalid REALITYLINK_ADDRESS")?;
    ensure!(
        http_url.host_str().is_some() && http_url.port_or_known_default() == Some(port),
        "Invalid REALITYLINK_ADDRESS"
    );
    let mut ws = http_url.clone();
    ws.set_scheme("ws")
        .map_err(|_| anyhow::anyhow!("Invalid WebSocket scheme"))?;
    ws.set_path("/minecraft-chat");
    Ok((http_url, ws.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn derives_both_endpoints_and_rejects_urls() {
        for address in ["localhost:39244", "127.0.0.1:80", "[::1]:39244"] {
            let (http, ws) = endpoints(address).unwrap();
            assert_eq!(http.scheme(), "http");
            assert_eq!(url::Url::parse(&ws).unwrap().path(), "/minecraft-chat");
        }
        for address in [
            "http://localhost:39244",
            "host",
            "host:0",
            "host:65536",
            "host:39244/path",
            "user@host:80",
            "::1:39244",
        ] {
            assert!(endpoints(address).is_err(), "{address}");
        }
    }
    #[test]
    fn reads_dotenv_and_environment_overrides_without_setting_process_variables() {
        let path =
            std::env::temp_dir().join(format!("rlmonitor-config-{}.env", std::process::id()));
        std::fs::write(&path, "REALITYLINK_ADDRESS=mc.local:39244\nSERVER_NAME=\"测试服务器\"\nRETENTION_DAYS=14\nRUST_LOG=debug\n").unwrap();
        let config = Config::from_sources(&path, [("RETENTION_DAYS".into(), "30".into())]).unwrap();
        assert_eq!(config.server_name, "测试服务器");
        assert_eq!(config.retention_days, 30);
        assert_eq!(config.log_filter, "debug");
        assert_eq!(config.ws_url, "ws://mc.local:39244/minecraft-chat");
        std::fs::write(&path, "BROKEN=\"unterminated\n").unwrap();
        assert!(Config::from_sources(&path, []).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn allows_missing_dotenv_but_reports_the_old_ws_configuration() {
        let path = Path::new("/tmp/rlmonitor-missing-config-directory/.env");
        let config = Config::from_sources(path, []).unwrap();
        assert_eq!(config.realitylink_address, "localhost:39244");
        assert_eq!(config.retention_days, 3);
        let old = [(
            "REALITYLINK_WS_URL".into(),
            "ws://old:39244/minecraft-chat".into(),
        )];
        assert!(
            Config::from_sources(path, old.clone())
                .err()
                .unwrap()
                .to_string()
                .contains("REALITYLINK_ADDRESS=host:port")
        );
        let config = Config::from_sources(
            path,
            old.into_iter()
                .chain([("REALITYLINK_ADDRESS".into(), "new:39244".into())]),
        )
        .unwrap();
        assert_eq!(config.ws_url, "ws://new:39244/minecraft-chat");
    }
}
