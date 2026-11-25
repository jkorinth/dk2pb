//! Append-only transaction ledger (JSON lines). Every change dk2pb makes to PartsBox
//! is recorded *as it happens*, so even a half-finished import can be rolled back.
//! The file holds snapshots; the last line per transaction id wins.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    CreatePart {
        part_id: String,
        name: String,
    },
    SetImage {
        part_id: String,
    },
    CreateOrder {
        order_id: String,
        number: String,
    },
    ReceiveStock {
        order_id: String,
        stock_id: String,
        part_id: String,
        quantity: i64,
    },
    /// Stock added without an order (PartsBox plans without the Orders feature).
    AddStock {
        part_id: String,
        storage_id: String,
        quantity: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TxnStatus {
    Running,
    Committed,
    Failed,
    RolledBack,
    RollbackFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedLine {
    pub sales_order_id: i64,
    pub detail_id: i64,
    pub dk_pn: String,
    pub mpn: String,
    pub quantity: i64,
    pub part_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Txn {
    pub id: String,
    pub created: String,
    pub summary: String,
    pub storage: String,
    pub status: TxnStatus,
    pub lines: Vec<ImportedLine>,
    pub ops: Vec<Op>,
    #[serde(default)]
    pub log: Vec<String>,
    #[serde(default)]
    pub rolled_back: Option<String>,
}

impl Txn {
    pub fn new(summary: String, storage: String) -> Txn {
        let now = chrono::Local::now();
        Txn {
            id: now.format("%Y%m%d-%H%M%S%.3f").to_string(),
            created: now.to_rfc3339(),
            summary,
            storage,
            status: TxnStatus::Running,
            lines: vec![],
            ops: vec![],
            log: vec![],
            rolled_back: None,
        }
    }
    pub fn active(&self) -> bool {
        !matches!(self.status, TxnStatus::RolledBack)
    }
}

pub struct Ledger {
    path: PathBuf,
    pub txns: Vec<Txn>,
}

impl Ledger {
    pub fn open(path: PathBuf) -> Result<Ledger> {
        let mut map: HashMap<String, Txn> = HashMap::new();
        let mut order = vec![];
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                if let Ok(t) = serde_json::from_str::<Txn>(line) {
                    if !map.contains_key(&t.id) {
                        order.push(t.id.clone());
                    }
                    map.insert(t.id.clone(), t);
                }
            }
        }
        let mut txns: Vec<Txn> = order.into_iter().filter_map(|id| map.remove(&id)).collect();
        txns.reverse(); // newest first
        Ok(Ledger { path, txns })
    }

    /// Persist a snapshot of `t` and update the in-memory list.
    pub fn save(&mut self, t: &Txn) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(f, "{}", serde_json::to_string(t)?)?;
        f.sync_data()?;
        match self.txns.iter_mut().find(|x| x.id == t.id) {
            Some(x) => *x = t.clone(),
            None => self.txns.insert(0, t.clone()),
        }
        Ok(())
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

/// A shared handle so the worker thread can write while the UI reads.
pub type SharedLedger = std::sync::Arc<std::sync::Mutex<Ledger>>;
