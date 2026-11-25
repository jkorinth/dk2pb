use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration, read from `~/.config/dk2pb/config.toml`.
/// Every secret can also come from an environment variable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub partsbox_api_key: String,
    pub digikey_client_id: String,
    pub digikey_client_secret: String,
    /// Optional DigiKey account number, only needed if your login has several.
    /// Empty = the default account of the API credentials.
    pub digikey_account_id: String,
    /// Locale site, e.g. "DE", "US", "UK", "AT", "CH".
    pub digikey_site: String,
    pub digikey_language: String,
    pub digikey_currency: String,
    /// Domain for browser links, e.g. "www.digikey.de".
    pub digikey_web_host: String,
    /// Show orders from this many days back.
    pub order_history_days: i64,
    /// Include orders placed by co-workers on the same account.
    pub shared_orders: bool,

    /// Storage location pre-selected in the import dialog (name).
    pub default_storage: String,
    /// Tags added to newly created parts.
    pub new_part_tags: Vec<String>,
    /// "shipped" (default): only import quantities DigiKey reports as shipped.
    /// "ordered": import the ordered quantity even if parts are still on backorder.
    pub import_quantity: String,
    /// Currency code as PartsBox expects it (lower-case keyword, e.g. "eur").
    pub partsbox_currency: String,

    /// Browser URL templates. Placeholders: {host} {sales_order} {order} {dk_pn} {part_id}
    pub url_digikey_order: String,
    pub url_digikey_product: String,
    pub url_partsbox_part: String,
    pub url_partsbox_order: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            partsbox_api_key: String::new(),
            digikey_client_id: String::new(),
            digikey_client_secret: String::new(),
            digikey_account_id: String::new(),
            digikey_site: "DE".into(),
            digikey_language: "en".into(),
            digikey_currency: "EUR".into(),
            digikey_web_host: "www.digikey.de".into(),
            order_history_days: 365,
            shared_orders: false,
            default_storage: String::new(),
            new_part_tags: vec!["digikey".into()],
            import_quantity: "shipped".into(),
            partsbox_currency: "eur".into(),
            url_digikey_order: "https://{host}/OrderHistory/Details?salesorderid={sales_order}"
                .into(),
            url_digikey_product: "https://{host}/en/products/result?keywords={dk_pn}".into(),
            url_partsbox_part: "https://partsbox.com/parts/{part_id}".into(),
            url_partsbox_order: "https://partsbox.com/orders/{order}".into(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dk2pb")
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dk2pb")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dk2pb")
}

impl Config {
    pub fn load(path: Option<PathBuf>) -> Result<(Self, PathBuf)> {
        let path = path.unwrap_or_else(|| config_dir().join("config.toml"));
        let mut cfg: Config = if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
        } else {
            Config::default()
        };
        // Secrets, lowest to highest precedence: config file, <NAME>_FILE, <NAME>.
        let env = |name: &str, field: &mut String| -> Result<()> {
            if let Ok(f) = std::env::var(format!("{name}_FILE"))
                && !f.is_empty()
            {
                *field = read_secret_file(f.as_ref())?;
            }
            if let Ok(v) = std::env::var(name)
                && !v.is_empty()
            {
                *field = v;
            }
            Ok(())
        };
        env("PARTSBOX_API_KEY", &mut cfg.partsbox_api_key)?;
        env("DIGIKEY_CLIENT_ID", &mut cfg.digikey_client_id)?;
        env("DIGIKEY_CLIENT_SECRET", &mut cfg.digikey_client_secret)?;
        env("DIGIKEY_ACCOUNT_ID", &mut cfg.digikey_account_id)?;
        Ok((cfg, path))
    }

    pub fn write_template(path: &PathBuf) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(&Config::default())?;
        std::fs::write(path, text)?;
        Ok(())
    }

    pub fn missing(&self) -> Vec<&'static str> {
        let mut m = vec![];
        if self.partsbox_api_key.is_empty() {
            m.push("partsbox_api_key / PARTSBOX_API_KEY");
        }
        if self.digikey_client_id.is_empty() {
            m.push("digikey_client_id / DIGIKEY_CLIENT_ID");
        }
        if self.digikey_client_secret.is_empty() {
            m.push("digikey_client_secret / DIGIKEY_CLIENT_SECRET");
        }
        m
    }

    pub fn url(&self, template: &str, vars: &[(&str, &str)]) -> String {
        let mut s = template.replace("{host}", &self.digikey_web_host);
        for (k, v) in vars {
            s = s.replace(&format!("{{{k}}}"), &urlencode(v));
        }
        s
    }
}

/// A secret stored in its own file (`pass`, sops, systemd credentials, docker secrets).
/// Only the first line is used, so a trailing newline is fine.
fn read_secret_file(path: &std::path::Path) -> Result<String> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading secret from {}", path.display()))?;
    Ok(text.lines().next().unwrap_or("").trim().to_string())
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
