//! Minimal PartsBox API client (https://partsbox.com/api.html).
//! The API is RPC-style: POST https://api.partsbox.com/api/1/<operation> with a JSON body.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;

pub trait PartsBoxTransport: Send + Sync {
    /// Call an operation and return `data` on success.
    fn call(&self, op: &str, payload: Value) -> Result<Value>;
    fn set_image(
        &self,
        part_id: &str,
        bytes: Vec<u8>,
        filename: &str,
        content_type: &str,
    ) -> Result<()>;
}

// ------------------------------------------------------------------ models

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Part {
    #[serde(rename = "part/id")]
    pub id: String,
    #[serde(rename = "part/name")]
    pub name: String,
    #[serde(rename = "part/type")]
    pub kind: Option<String>,
    #[serde(rename = "part/description")]
    pub description: Option<String>,
    #[serde(rename = "part/footprint")]
    pub footprint: Option<String>,
    #[serde(rename = "part/notes")]
    pub notes: Option<String>,
    #[serde(rename = "part/tags")]
    pub tags: Vec<String>,
    #[serde(rename = "part/manufacturer")]
    pub manufacturer: Option<String>,
    #[serde(rename = "part/mpn")]
    pub mpn: Option<String>,
    /// Raw: the API documents the shape only as "object", so never fail to parse a part over it.
    #[serde(rename = "part/custom-fields")]
    pub custom_fields: Value,
    #[serde(rename = "part/created")]
    pub created: Option<i64>,
    #[serde(rename = "part/stock")]
    pub stock: Vec<StockEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StockEntry {
    #[serde(rename = "stock/id")]
    pub id: Option<String>,
    #[serde(rename = "stock/storage-id")]
    pub storage_id: Option<String>,
    #[serde(rename = "stock/quantity")]
    pub quantity: i64,
    #[serde(rename = "stock/price")]
    pub price: Option<f64>,
    #[serde(rename = "stock/currency")]
    pub currency: Option<String>,
    #[serde(rename = "stock/timestamp")]
    pub timestamp: Option<i64>,
    #[serde(rename = "stock/comments")]
    pub comments: Option<String>,
    #[serde(rename = "stock/status")]
    pub status: Option<String>,
    #[serde(rename = "stock/part-id")]
    pub part_id: Option<String>,
    #[serde(rename = "stock/order-id")]
    pub order_id: Option<String>,
    #[serde(rename = "stock/order-entry-id")]
    pub order_entry_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Storage {
    #[serde(rename = "storage/id")]
    pub id: String,
    #[serde(rename = "storage/name")]
    pub name: String,
    #[serde(rename = "storage/description")]
    pub description: Option<String>,
    #[serde(rename = "storage/tags")]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PbOrder {
    #[serde(rename = "order/id")]
    pub id: String,
    #[serde(rename = "order/vendor-name")]
    pub vendor_name: Option<String>,
    #[serde(rename = "order/number")]
    pub number: Option<String>,
    #[serde(rename = "order/status")]
    pub status: Option<String>,
    #[serde(rename = "order/invoice-number")]
    pub invoice_number: Option<String>,
    #[serde(rename = "order/comments")]
    pub comments: Option<String>,
    #[serde(rename = "order/created")]
    pub created: Option<i64>,
    #[serde(rename = "order/entries")]
    pub entries: Vec<OrderEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OrderEntry {
    #[serde(rename = "order-entry/id")]
    pub id: String,
    #[serde(rename = "order-entry/part-id")]
    pub part_id: Option<String>,
    #[serde(rename = "order-entry/stock-id")]
    pub stock_id: Option<String>,
    #[serde(rename = "order-entry/vendor-sku")]
    pub vendor_sku: Option<String>,
    #[serde(rename = "order-entry/vendor-quantity")]
    pub vendor_quantity: Option<i64>,
    #[serde(rename = "order-entry/received-quantity")]
    pub received_quantity: Option<i64>,
    #[serde(rename = "order-entry/price")]
    pub price: Option<f64>,
}

impl Part {
    /// On-hand stock (entries without a status; "ordered", "allocated" etc. excluded).
    pub fn on_hand(&self) -> i64 {
        self.stock
            .iter()
            .filter(|s| s.status.is_none())
            .map(|s| s.quantity)
            .sum()
    }
    pub fn on_hand_by_storage(&self) -> Vec<(String, i64)> {
        let mut m: HashMap<String, i64> = HashMap::new();
        for s in self.stock.iter().filter(|s| s.status.is_none()) {
            *m.entry(s.storage_id.clone().unwrap_or_default())
                .or_default() += s.quantity;
        }
        let mut v: Vec<_> = m.into_iter().filter(|(_, q)| *q != 0).collect();
        v.sort();
        v
    }
    /// Manufacturer from the part, or from the notes we write ("Manufacturer: X").
    pub fn manufacturer_guess(&self) -> Option<String> {
        if let Some(m) = self.manufacturer.as_ref().filter(|m| !m.is_empty()) {
            return Some(m.clone());
        }
        self.notes.as_deref().and_then(|n| {
            n.lines().find_map(|l| {
                l.strip_prefix("Manufacturer: ")
                    .map(|s| s.trim().to_string())
            })
        })
    }
}

// ------------------------------------------------------------------ typed wrapper

pub struct PartsBox {
    pub t: Box<dyn PartsBoxTransport>,
}

/// PartsBox answers 401 "forbidden / Feature not present in your plan" for operations the
/// account's plan does not include (Orders are such a feature).
pub fn is_plan_limited(e: &anyhow::Error) -> bool {
    let s = e.to_string();
    s.contains("not present in your plan") || s.contains("forbidden")
}

pub fn ok_category(cat: &str) -> bool {
    matches!(cat, "status/ok" | "ok")
}

impl PartsBox {
    pub fn parts(&self) -> Result<Vec<Part>> {
        Ok(serde_json::from_value(self.t.call("part/all", json!({}))?)?)
    }
    pub fn part(&self, id: &str) -> Result<Part> {
        Ok(serde_json::from_value(
            self.t.call("part/get", json!({"part/id": id}))?,
        )?)
    }
    pub fn storages(&self) -> Result<Vec<Storage>> {
        Ok(serde_json::from_value(
            self.t.call("storage/all", json!({}))?,
        )?)
    }
    pub fn orders(&self) -> Result<Vec<PbOrder>> {
        Ok(serde_json::from_value(
            self.t.call("order/all", json!({}))?,
        )?)
    }
    pub fn order(&self, id: &str) -> Result<PbOrder> {
        Ok(serde_json::from_value(
            self.t.call("order/get", json!({"order/id": id}))?,
        )?)
    }
    pub fn order_stock(&self, id: &str) -> Result<Vec<StockEntry>> {
        Ok(serde_json::from_value(
            self.t.call("order/get-entries", json!({"order/id": id}))?,
        )?)
    }
}

// ------------------------------------------------------------------ HTTP transport

pub struct HttpTransport {
    key: String,
    http: reqwest::blocking::Client,
}

const API: &str = "https://api.partsbox.com/api/1/";

impl HttpTransport {
    pub fn new(key: String) -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .user_agent("dk2pb/0.1")
            .build()
            .expect("http client");
        Self { key, http }
    }

    fn handle(op: &str, r: reqwest::blocking::Response) -> Result<Value> {
        let status = r.status();
        let text = r.text()?;
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let cat = body["partsbox.status/category"].as_str().unwrap_or("");
        if status.is_success() && ok_category(cat) {
            return Ok(body.get("data").cloned().unwrap_or(Value::Null));
        }
        let msg = body["partsbox.status/message"].as_str().unwrap_or("");
        bail!(
            "PartsBox {op}: HTTP {status} {cat} {msg} {}",
            if body.is_null() {
                text.chars().take(300).collect::<String>()
            } else {
                String::new()
            }
        )
    }
}

impl PartsBoxTransport for HttpTransport {
    fn call(&self, op: &str, payload: Value) -> Result<Value> {
        for attempt in 0..5 {
            let r = self
                .http
                .post(format!("{API}{op}"))
                .header("Authorization", format!("APIKey {}", self.key))
                .json(&payload)
                .send()?;
            if r.status().as_u16() == 429 {
                std::thread::sleep(Duration::from_millis(500 * (1 << attempt)));
                continue;
            }
            return Self::handle(op, r);
        }
        bail!("PartsBox {op}: rate limited")
    }

    fn set_image(
        &self,
        part_id: &str,
        bytes: Vec<u8>,
        filename: &str,
        content_type: &str,
    ) -> Result<()> {
        let part = reqwest::blocking::multipart::Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str(content_type)?;
        let form = reqwest::blocking::multipart::Form::new()
            .text("part/id", part_id.to_string())
            .part("file", part);
        let r = self
            .http
            .post(format!("{API}part/set-image"))
            .header("Authorization", format!("APIKey {}", self.key))
            .multipart(form)
            .send()?;
        Self::handle("part/set-image", r).map(|_| ())
    }
}
