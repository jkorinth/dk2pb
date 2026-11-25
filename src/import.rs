//! Import DigiKey line items into PartsBox, and roll imports back.
//!
//! Flow per import (one ledger transaction):
//!   for every line:   DigiKey product details → find or create part → set photo (new parts)
//!   per sales order:  order/create (vendor DigiKey, number = sales order id, entries)
//!                     → order/mark-ordered → order/receive into the chosen storage
//!                     → order/get-entries to learn the stock ids (for rollback)
//! On plans without the Orders feature (order/create answers 401 "forbidden"), stock is
//! added per line with stock/add instead, and rolled back with stock/remove.

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

use crate::config::Config;
use crate::digikey::{DigiKeyApi, LineItem, Product};
use crate::ledger::{ImportedLine, Op, SharedLedger, Txn, TxnStatus};
use crate::partsbox::{PartsBox, StockEntry, is_plan_limited};

#[derive(Debug, Clone)]
pub struct ImportLine {
    pub sales_order_id: i64,
    pub order_number: i64,
    pub po_number: Option<String>,
    pub order_date: Option<String>,
    pub item: LineItem,
    pub quantity: i64,
}

#[derive(Debug, Clone)]
pub struct ImportRequest {
    pub summary: String,
    pub storage_id: String,
    pub storage_name: String,
    pub lines: Vec<ImportLine>,
    /// lower-case MPN / name → PartsBox part id (snapshot from the UI)
    pub part_index: HashMap<String, String>,
}

pub type Progress<'a> = &'a dyn Fn(usize, usize, String);

fn record(ledger: &SharedLedger, txn: &mut Txn, op: Op) -> Result<()> {
    txn.ops.push(op);
    ledger.lock().unwrap().save(txn)
}

fn log(
    ledger: &SharedLedger,
    txn: &mut Txn,
    progress: Progress,
    done: usize,
    total: usize,
    msg: String,
) {
    txn.log.push(msg.clone());
    let _ = ledger.lock().unwrap().save(txn);
    progress(done, total, msg);
}

pub fn part_notes(p: Option<&Product>, item: &LineItem) -> String {
    let mut n = vec![format!("DigiKey: {}", item.digi_key_product_number)];
    if let Some(c) = p.map(|p| p.category.clone()).filter(|c| !c.is_empty()) {
        n.push(format!("Category: {c}"));
    }
    n.join("\n")
}

/// Link to the product on DigiKey: the one the API gave us, or a search for the part number.
pub fn product_url(p: Option<&Product>, item: &LineItem, cfg: &Config) -> String {
    p.and_then(|p| p.product_url.clone()).unwrap_or_else(|| {
        cfg.url(
            &cfg.url_digikey_product,
            &[("dk_pn", &item.digi_key_product_number)],
        )
    })
}

fn to_millis(date: &Option<String>) -> Option<i64> {
    date.as_deref().and_then(|d| {
        chrono::DateTime::parse_from_rfc3339(d)
            .ok()
            .map(|t| t.timestamp_millis())
            .or_else(|| {
                chrono::NaiveDateTime::parse_from_str(d, "%Y-%m-%dT%H:%M:%S%.f")
                    .ok()
                    .map(|t| t.and_utc().timestamp_millis())
            })
    })
}

pub fn run_import(
    req: ImportRequest,
    cfg: &Config,
    dk: &dyn DigiKeyApi,
    pb: &PartsBox,
    ledger: &SharedLedger,
    progress: Progress,
) -> Result<Txn> {
    let mut txn = Txn::new(req.summary.clone(), req.storage_name.clone());
    ledger.lock().unwrap().save(&txn)?;

    let result = import_inner(&req, cfg, dk, pb, ledger, &mut txn, progress);
    txn.status = if result.is_ok() {
        TxnStatus::Committed
    } else {
        TxnStatus::Failed
    };
    if let Err(e) = &result {
        txn.log.push(format!("ERROR: {e:#}"));
    }
    ledger.lock().unwrap().save(&txn)?;
    result.map(|_| txn)
}

fn import_inner(
    req: &ImportRequest,
    cfg: &Config,
    dk: &dyn DigiKeyApi,
    pb: &PartsBox,
    ledger: &SharedLedger,
    txn: &mut Txn,
    progress: Progress,
) -> Result<()> {
    let mut index = req.part_index.clone();
    let lines: Vec<&ImportLine> = req.lines.iter().filter(|l| l.quantity > 0).collect();
    let n_so = lines
        .iter()
        .map(|l| l.sales_order_id)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    // steps: per line (lookup+part+image) + per sales order (order+receive)
    let total = lines.len() + n_so;
    let mut done = 0;

    let mut by_so: BTreeMap<i64, Vec<(&ImportLine, String)>> = BTreeMap::new();

    for l in &lines {
        let it = &l.item;
        let mpn = if it.manufacturer_product_number.is_empty() {
            it.digi_key_product_number.clone()
        } else {
            it.manufacturer_product_number.clone()
        };
        progress(
            done,
            total,
            format!("Looking up {}", it.digi_key_product_number),
        );
        let product = match dk.product(&it.digi_key_product_number) {
            Ok(p) => p,
            Err(e) => {
                log(
                    ledger,
                    txn,
                    progress,
                    done,
                    total,
                    format!(
                        "{}: DigiKey lookup failed: {e:#}",
                        it.digi_key_product_number
                    ),
                );
                None
            }
        };

        let key = mpn.to_lowercase();
        let part_id = if let Some(id) = index.get(&key) {
            log(
                ledger,
                txn,
                progress,
                done,
                total,
                format!("{mpn}: using existing part"),
            );
            id.clone()
        } else {
            progress(done, total, format!("Creating part {mpn}"));
            let desc = product
                .as_ref()
                .map(|p| {
                    if p.detailed_description.is_empty() {
                        p.description.clone()
                    } else {
                        p.detailed_description.clone()
                    }
                })
                .filter(|d| !d.is_empty())
                .or_else(|| it.description.clone())
                .unwrap_or_default();
            let mut payload = json!({
                "part/type": "local",
                "part/name": mpn,
                "part/description": desc,
                "part/notes": part_notes(product.as_ref(), it),
                "part/tags": cfg.new_part_tags,
            });
            if let Some(fp) = product.as_ref().and_then(|p| p.package.clone()) {
                payload["part/footprint"] = json!(fp);
            }
            let data = pb.t.call("part/create", payload)?;
            let id = data["part/id"]
                .as_str()
                .ok_or_else(|| anyhow!("part/create returned no id"))?
                .to_string();
            record(
                ledger,
                txn,
                Op::CreatePart {
                    part_id: id.clone(),
                    name: mpn.clone(),
                },
            )?;
            index.insert(key.clone(), id.clone());

            // Everything below is best-effort: part/create answers 400 "Invalid part created" for
            // custom fields, and part/mpn / part/manufacturer are documented read-only. The part
            // exists at this point, so a reject gets logged and the import goes on.
            let mut fields = json!({ "part/id": id });
            if !it.manufacturer_product_number.is_empty() {
                fields["part/mpn"] = json!(it.manufacturer_product_number);
            }
            if let Some(m) = product
                .as_ref()
                .map(|p| p.manufacturer.clone())
                .filter(|m| !m.is_empty())
            {
                fields["part/manufacturer"] = json!(m);
            }
            if fields.as_object().is_some_and(|f| f.len() > 1)
                && let Err(e) = pb.t.call("part/update", fields)
            {
                log(
                    ledger,
                    txn,
                    progress,
                    done,
                    total,
                    format!("{mpn}: MPN/manufacturer not set: {e:#}"),
                );
            }

            let mut cf =
                vec![json!({"key": "Vendor URL", "value": product_url(product.as_ref(), it, cfg)})];
            if let Some(ds) = product.as_ref().and_then(|p| p.datasheet_url.clone()) {
                cf.push(json!({"key": "Datasheet", "value": ds}));
            }
            let cf = json!({"table": "parts", "id": id, "custom-fields": cf});
            if let Err(e) = pb.t.call("custom-field/update", cf) {
                log(
                    ledger,
                    txn,
                    progress,
                    done,
                    total,
                    format!("{mpn}: custom fields not set: {e:#}"),
                );
            }

            if let Some(url) = product.as_ref().and_then(|p| p.photo_url.clone()) {
                progress(done, total, format!("Uploading photo for {mpn}"));
                match dk.fetch(&url).and_then(|(bytes, ct)| {
                    let ext = match ct.as_str() {
                        "image/png" => "png",
                        "image/gif" => "gif",
                        "image/webp" => "webp",
                        _ => "jpg",
                    };
                    pb.t.set_image(
                        &id,
                        bytes,
                        &format!("{}.{ext}", it.digi_key_product_number),
                        &ct,
                    )
                }) {
                    Ok(()) => record(
                        ledger,
                        txn,
                        Op::SetImage {
                            part_id: id.clone(),
                        },
                    )?,
                    Err(e) => log(
                        ledger,
                        txn,
                        progress,
                        done,
                        total,
                        format!("{mpn}: photo failed: {e:#}"),
                    ),
                }
            } else {
                log(
                    ledger,
                    txn,
                    progress,
                    done,
                    total,
                    format!("{mpn}: no photo on DigiKey"),
                );
            }
            id
        };
        by_so
            .entry(l.sales_order_id)
            .or_default()
            .push((l, part_id));
        done += 1;
        progress(done, total, format!("{mpn} ready"));
    }

    for (so, items) in by_so {
        progress(
            done,
            total,
            format!("Creating PartsBox order for sales order {so}"),
        );
        let first = items[0].0;
        let invoices: Vec<String> = {
            let mut v: Vec<i64> = items
                .iter()
                .flat_map(|(l, _)| l.item.item_shipments.iter().filter_map(|s| s.invoice_id))
                .collect();
            v.sort();
            v.dedup();
            v.into_iter().map(|i| i.to_string()).collect()
        };
        let mut order = json!({
            "order/vendor-name": "DigiKey",
            "order/number": so.to_string(),
            "order/comments": format!("Imported by dk2pb, transaction {} (DigiKey order {})", txn.id, first.order_number),
            "order/tags": ["dk2pb"],
        });
        if let Some(ms) = to_millis(&first.order_date) {
            order["order/created"] = json!(ms);
        }
        if let Some(po) = first.po_number.as_ref().filter(|p| !p.is_empty()) {
            order["order/po-number"] = json!(po);
        }
        if !invoices.is_empty() {
            order["order/invoice-number"] = json!(invoices.join(", "));
        }
        let entries: Vec<Value> = items
            .iter()
            .map(|(l, pid)| {
                let mut e = json!({
                    "order-entry/part-id": pid,
                    "order-entry/vendor-quantity": l.quantity,
                    "order-entry/vendor-sku": l.item.digi_key_product_number,
                });
                if let Some(p) = l.item.unit_price {
                    e["order-entry/price"] = json!(p);
                    e["order-entry/currency"] = json!(cfg.partsbox_currency);
                }
                e
            })
            .collect();
        match pb
            .t
            .call("order/create", json!({"order": order, "entries": entries}))
        {
            Err(e) if is_plan_limited(&e) => {
                log(
                    ledger,
                    txn,
                    progress,
                    done,
                    total,
                    format!("no PartsBox order ({e:#}); adding stock directly"),
                );
                for (l, pid) in &items {
                    let mut payload = json!({
                        "stock/part-id": pid,
                        "stock/storage-id": req.storage_id,
                        "stock/quantity": l.quantity,
                        "stock/comments": format!("DigiKey {} sales order {so} (dk2pb {})", l.item.digi_key_product_number, txn.id),
                    });
                    if let Some(p) = l.item.unit_price {
                        payload["stock/price"] = json!(p);
                        payload["stock/currency"] = json!(cfg.partsbox_currency);
                    }
                    progress(
                        done,
                        total,
                        format!(
                            "Adding {} × {} to {}",
                            l.quantity, l.item.digi_key_product_number, req.storage_name
                        ),
                    );
                    pb.t.call("stock/add", payload)?;
                    record(
                        ledger,
                        txn,
                        Op::AddStock {
                            part_id: pid.clone(),
                            storage_id: req.storage_id.clone(),
                            quantity: l.quantity,
                        },
                    )?;
                }
            }
            r => {
                let data = r?;
                let order_id = data["order/id"]
                    .as_str()
                    .ok_or_else(|| anyhow!("order/create returned no id"))?
                    .to_string();
                record(
                    ledger,
                    txn,
                    Op::CreateOrder {
                        order_id: order_id.clone(),
                        number: so.to_string(),
                    },
                )?;

                let created = pb.order(&order_id).context("reading new order")?;
                if let Err(e) =
                    pb.t.call("order/mark-ordered", json!({"order/id": order_id}))
                {
                    log(
                        ledger,
                        txn,
                        progress,
                        done,
                        total,
                        format!("mark-ordered: {e:#} (continuing)"),
                    );
                }
                // entry id per part
                let mut receive = vec![];
                for (l, pid) in &items {
                    if let Some(e) = created.entries.iter().find(|e| {
                        e.part_id.as_deref() == Some(pid.as_str())
                            && e.vendor_sku.as_deref()
                                == Some(l.item.digi_key_product_number.as_str())
                    }) {
                        receive.push(json!({"order-entry/id": e.id, "order-entry/received-quantity": l.quantity}));
                    }
                }
                progress(
                    done,
                    total,
                    format!(
                        "Receiving {} entries into {}",
                        receive.len(),
                        req.storage_name
                    ),
                );
                pb.t.call(
                    "order/receive",
                    json!({
                        "order/id": order_id,
                        "storage/id": req.storage_id,
                        "order-entries": receive,
                        "stock/comments": format!("DigiKey sales order {so} (dk2pb {})", txn.id),
                    }),
                )?;
                for s in pb.order_stock(&order_id)? {
                    if let (Some(sid), Some(pid)) = (s.id.clone(), s.part_id.clone())
                        && s.status.is_none()
                    {
                        record(
                            ledger,
                            txn,
                            Op::ReceiveStock {
                                order_id: order_id.clone(),
                                stock_id: sid,
                                part_id: pid,
                                quantity: s.quantity,
                            },
                        )?;
                    }
                }
            }
        }
        for (l, pid) in &items {
            txn.lines.push(ImportedLine {
                sales_order_id: so,
                detail_id: l.item.detail_id,
                dk_pn: l.item.digi_key_product_number.clone(),
                mpn: l.item.manufacturer_product_number.clone(),
                quantity: l.quantity,
                part_id: pid.clone(),
            });
        }
        ledger.lock().unwrap().save(txn)?;
        done += 1;
        progress(done, total, format!("Sales order {so} received"));
    }
    Ok(())
}

/// Undo every recorded operation of a transaction, newest first.
pub fn run_rollback(
    txn_id: &str,
    pb: &PartsBox,
    ledger: &SharedLedger,
    progress: Progress,
) -> Result<Txn> {
    let mut txn = ledger
        .lock()
        .unwrap()
        .txns
        .iter()
        .find(|t| t.id == txn_id)
        .cloned()
        .ok_or_else(|| anyhow!("transaction {txn_id} not found"))?;
    let total = txn.ops.len();
    let mut failures = 0;
    let ops: Vec<Op> = txn.ops.iter().rev().cloned().collect();
    for (i, op) in ops.iter().enumerate() {
        let res: Result<String> = match op {
            Op::ReceiveStock {
                stock_id,
                part_id,
                quantity,
                ..
            } => (|| {
                let part = pb.part(part_id)?;
                if part.on_hand() < *quantity {
                    anyhow::bail!(
                        "part {} has only {} on hand, {} were imported – stock was used; not reverting",
                        part.name,
                        part.on_hand(),
                        quantity
                    );
                }
                pb.t.call(
                    "order/unreceive",
                    json!({"stock/id": stock_id, "stock/part-id": part_id}),
                )?;
                Ok(format!("unreceived {quantity} × {}", part.name))
            })(),
            Op::AddStock {
                part_id,
                storage_id,
                quantity,
            } => (|| {
                let part = pb.part(part_id)?;
                let here = part
                    .on_hand_by_storage()
                    .iter()
                    .find(|(s, _)| s == storage_id)
                    .map(|(_, q)| *q)
                    .unwrap_or(0);
                if here < *quantity {
                    anyhow::bail!(
                        "part {} has only {} in that location, {} were imported – stock was used; not reverting",
                        part.name,
                        here,
                        quantity
                    );
                }
                pb.t.call("stock/remove", json!({
                    "stock/source": {"source/part-id": part_id, "source/storage-id": storage_id},
                    "stock/quantity": quantity,
                    "stock/comments": format!("rollback (dk2pb {txn_id})"),
                }))?;
                Ok(format!("removed {quantity} × {}", part.name))
            })(),
            Op::CreateOrder { order_id, number } => (|| {
                let _ = pb.t.call("order/cancel", json!({"order/id": order_id}));
                pb.t.call("order/delete", json!({"order/id": order_id}))?;
                Ok(format!("deleted PartsBox order for sales order {number}"))
            })(),
            Op::SetImage { part_id } => match pb
                .t
                .call("part/delete-image", json!({"part/id": part_id}))
            {
                Ok(_) => Ok("removed image".into()),
                Err(e) if e.to_string().contains("not-found") => Ok("image already gone".into()),
                Err(e) => Err(e),
            },
            Op::CreatePart { part_id, name } => (|| {
                let part = pb.part(part_id)?;
                // stock/remove leaves our own +/- movements behind: those must not block the delete
                let ours =
                    |s: &StockEntry| s.comments.as_deref().is_some_and(|c| c.contains(txn_id));
                if part.on_hand() != 0
                    || part
                        .stock
                        .iter()
                        .any(|s| s.status.is_none() && s.quantity != 0 && !ours(s))
                {
                    anyhow::bail!("part {name} has other stock history; kept");
                }
                pb.t.call("part/delete", json!({"part/id": part_id}))?;
                Ok(format!("deleted part {name}"))
            })(),
        };
        let msg = match res {
            Ok(m) => m,
            Err(e) => {
                failures += 1;
                format!("FAILED: {e:#}")
            }
        };
        txn.log.push(format!("rollback: {msg}"));
        progress(i + 1, total, msg);
    }
    txn.status = if failures == 0 {
        TxnStatus::RolledBack
    } else {
        TxnStatus::RollbackFailed
    };
    txn.rolled_back = Some(chrono::Local::now().to_rfc3339());
    ledger.lock().unwrap().save(&txn)?;
    Ok(txn)
}
