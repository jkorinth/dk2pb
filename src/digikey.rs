//! DigiKey Order Status v4 + Product Information v4 (2-legged OAuth).

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::{Config, cache_dir};

// ------------------------------------------------------------------ models

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct OrderResponse {
    pub total_orders: i64,
    pub orders: Vec<Order>,
}

/// One checkout. DigiKey splits it into one or more sales orders
/// (e.g. backorders, marketplace/3rd-party items, scheduled releases).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Order {
    pub order_number: i64,
    pub date_entered: Option<String>,
    pub currency: Option<String>,
    #[serde(rename = "PONumber")]
    pub po_number: Option<String>,
    pub entire_order_status: Option<StatusInfo>,
    pub sales_orders: Vec<SalesOrder>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct StatusInfo {
    #[serde(alias = "SalesOrderStatus")]
    pub order_status: Option<String>,
    pub short_description: Option<String>,
    pub long_description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SalesOrder {
    pub sales_order_id: i64,
    pub status: Option<StatusInfo>,
    pub purchase_order: Option<String>,
    pub total_price: Option<f64>,
    pub date_entered: Option<String>,
    pub order_number: i64,
    pub ship_method: Option<String>,
    pub currency: Option<String>,
    pub line_items: Vec<LineItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct LineItem {
    pub sales_order_id: i64,
    pub detail_id: i64,
    pub total_price: Option<f64>,
    pub customer_reference: Option<String>,
    pub country_of_origin: Option<String>,
    pub digi_key_product_number: String,
    pub manufacturer_product_number: String,
    pub description: Option<String>,
    pub pack_type: Option<String>,
    pub quantity_ordered: i64,
    pub quantity_shipped: i64,
    pub quantity_reserved: i64,
    pub quantity_back_order: i64,
    pub unit_price: Option<f64>,
    pub item_shipments: Vec<ItemShipment>,
    pub schedules: Vec<Schedule>,
}

/// One physical delivery of (part of) a line item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ItemShipment {
    pub quantity_shipped: i64,
    pub invoice_id: Option<i64>,
    pub shipped_date: Option<String>,
    pub tracking_number: Option<String>,
    pub expected_delivery_date: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Schedule {
    pub quantity_scheduled: i64,
    pub scheduled_date: Option<String>,
    pub digi_key_release_date: Option<String>,
}

/// The fields we use from Product Information v4 ProductDetails.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Product {
    pub dk_pn: String,
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    pub detailed_description: String,
    pub category: String,
    pub package: Option<String>,
    pub photo_url: Option<String>,
    pub datasheet_url: Option<String>,
    pub product_url: Option<String>,
    pub parameters: Vec<(String, String)>,
}

impl Product {
    pub fn from_details(dk_pn: &str, v: &Value) -> Product {
        let p = v.get("Product").unwrap_or(v);
        let s = |v: Option<&Value>| v.and_then(|x| x.as_str()).unwrap_or("").to_string();
        let url = |v: Option<&Value>| {
            v.and_then(|x| x.as_str())
                .filter(|x| !x.is_empty())
                .map(|u| {
                    if u.starts_with("//") {
                        format!("https:{u}")
                    } else {
                        u.to_string()
                    }
                })
        };
        let parameters: Vec<(String, String)> = p
            .get("Parameters")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .map(|x| (s(x.get("ParameterText")), s(x.get("ValueText"))))
                    .filter(|(k, v)| !k.is_empty() && !v.is_empty() && v != "-")
                    .collect()
            })
            .unwrap_or_default();
        let find = |names: &[&str]| {
            parameters
                .iter()
                .find(|(k, _)| names.contains(&k.as_str()))
                .map(|(_, v)| v.clone())
        };
        Product {
            dk_pn: dk_pn.to_string(),
            mpn: s(p.get("ManufacturerProductNumber")),
            manufacturer: s(p.pointer("/Manufacturer/Name")),
            description: s(p.pointer("/Description/ProductDescription")),
            detailed_description: s(p.pointer("/Description/DetailedDescription")),
            category: s(p.pointer("/Category/Name")),
            package: find(&["Supplier Device Package", "Package / Case"]),
            photo_url: url(p.get("PhotoUrl")),
            datasheet_url: url(p.get("DatasheetUrl")),
            product_url: url(p.get("ProductUrl")),
            parameters,
        }
    }
}

// ------------------------------------------------------------------ trait

pub trait DigiKeyApi: Send + Sync {
    fn orders(&self, days_back: i64, shared: bool) -> Result<Vec<Order>>;
    fn product(&self, dk_pn: &str) -> Result<Option<Product>>;
    /// Download a binary (photo). Returns bytes + content type.
    fn fetch(&self, url: &str) -> Result<(Vec<u8>, String)>;
}

// ------------------------------------------------------------------ real client

pub struct DigiKey {
    cfg: Config,
    http: reqwest::blocking::Client,
    token: Mutex<Option<(String, Instant)>>,
}

const API: &str = "https://api.digikey.com";

impl DigiKey {
    pub fn new(cfg: Config) -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent("dk2pb/0.1")
            .build()
            .expect("http client");
        Self {
            cfg,
            http,
            token: Mutex::new(None),
        }
    }

    fn token(&self, force: bool) -> Result<String> {
        let mut t = self.token.lock().unwrap();
        if !force
            && let Some((tok, exp)) = t.as_ref()
            && Instant::now() < *exp
        {
            return Ok(tok.clone());
        }
        let r = self
            .http
            .post(format!("{API}/v1/oauth2/token"))
            .form(&[
                ("client_id", self.cfg.digikey_client_id.as_str()),
                ("client_secret", self.cfg.digikey_client_secret.as_str()),
                ("grant_type", "client_credentials"),
            ])
            .send()?;
        let status = r.status();
        let body: Value = r.json().unwrap_or(Value::Null);
        if !status.is_success() {
            bail!("DigiKey token request failed ({status}): {body}");
        }
        let tok = body["access_token"]
            .as_str()
            .ok_or_else(|| anyhow!("no access_token"))?
            .to_string();
        let secs = body["expires_in"]
            .as_u64()
            .unwrap_or(600)
            .saturating_sub(30);
        *t = Some((tok.clone(), Instant::now() + Duration::from_secs(secs)));
        Ok(tok)
    }

    fn get(
        &self,
        url: &str,
        query: &[(&str, String)],
        with_account: bool,
    ) -> Result<Option<Value>> {
        for attempt in 0..4 {
            let tok = self.token(attempt > 0)?;
            let mut req = self
                .http
                .get(url)
                .query(query)
                .bearer_auth(tok)
                .header("X-DIGIKEY-Client-Id", &self.cfg.digikey_client_id)
                .header("X-DIGIKEY-Locale-Site", &self.cfg.digikey_site)
                .header("X-DIGIKEY-Locale-Language", &self.cfg.digikey_language)
                .header("X-DIGIKEY-Locale-Currency", &self.cfg.digikey_currency);
            if with_account && !self.cfg.digikey_account_id.is_empty() {
                req = req.header("X-DIGIKEY-Account-Id", &self.cfg.digikey_account_id);
            }
            let r = req.send()?;
            match r.status().as_u16() {
                200 => return Ok(Some(r.json()?)),
                404 => return Ok(None),
                401 => continue, // refresh token and retry
                429 => {
                    let wait = r
                        .headers()
                        .get("Retry-After")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(5u64);
                    std::thread::sleep(Duration::from_secs(wait.min(60)));
                }
                s => {
                    let text = r.text().unwrap_or_default();
                    bail!(
                        "DigiKey {url} → HTTP {s}: {}",
                        text.chars().take(400).collect::<String>()
                    );
                }
            }
        }
        bail!("DigiKey {url}: giving up after retries")
    }

    fn product_cache_path(dk_pn: &str) -> PathBuf {
        let safe: String = dk_pn
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        cache_dir().join("products").join(format!("{safe}.json"))
    }
}

impl DigiKeyApi for DigiKey {
    fn orders(&self, days_back: i64, shared: bool) -> Result<Vec<Order>> {
        let end = chrono::Utc::now();
        let start = end - chrono::Duration::days(days_back);
        let mut all = vec![];
        let mut page = 1;
        loop {
            let q = vec![
                ("Shared", shared.to_string()),
                ("StartDate", start.format("%Y-%m-%dT00:00:00Z").to_string()),
                ("EndDate", end.format("%Y-%m-%dT23:59:59Z").to_string()),
                ("PageNumber", page.to_string()),
                ("PageSize", "25".to_string()),
            ];
            let v = self
                .get(&format!("{API}/orderstatus/v4/orders"), &q, true)?
                .unwrap_or(Value::Null);
            let resp: OrderResponse =
                serde_json::from_value(v).context("parsing DigiKey orders")?;
            let n = resp.orders.len();
            all.extend(resp.orders);
            if n < 25 || all.len() as i64 >= resp.total_orders || page > 200 {
                break;
            }
            page += 1;
        }
        all.sort_by(|a, b| b.date_entered.cmp(&a.date_entered));
        Ok(all)
    }

    fn product(&self, dk_pn: &str) -> Result<Option<Product>> {
        let path = Self::product_cache_path(dk_pn);
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(p) = serde_json::from_str::<Product>(&text)
        {
            return Ok(Some(p));
        }
        let enc: String = dk_pn
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect();
        let url = format!("{API}/products/v4/search/{enc}/productdetails");
        let Some(v) = self.get(&url, &[], false)? else {
            return Ok(None);
        };
        let p = Product::from_details(dk_pn, &v);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(&path, serde_json::to_string_pretty(&p)?);
        }
        Ok(Some(p))
    }

    fn fetch(&self, url: &str) -> Result<(Vec<u8>, String)> {
        let r = self
            .http
            .get(url)
            .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) dk2pb")
            .send()?;
        if !r.status().is_success() {
            bail!("download {url}: HTTP {}", r.status());
        }
        let ct = r
            .headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .split(';')
            .next()
            .unwrap_or("image/jpeg")
            .to_string();
        Ok((r.bytes()?.to_vec(), ct))
    }
}
