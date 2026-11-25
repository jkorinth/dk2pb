//! Demo mode: fake DigiKey orders and an in-memory PartsBox, so the UI can be tried
//! without credentials (`dk2pb --demo`). Nothing leaves your machine in demo mode.

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::sync::Mutex;

use crate::digikey::{DigiKeyApi, ItemShipment, LineItem, Order, Product, SalesOrder, StatusInfo};
use crate::partsbox::{OrderEntry, Part, PartsBoxTransport, PbOrder, StockEntry, Storage};

// ------------------------------------------------------------------ DigiKey

pub struct MockDigiKey;

#[allow(clippy::too_many_arguments)]
fn li(
    so: i64,
    detail: i64,
    dk: &str,
    mpn: &str,
    desc: &str,
    ordered: i64,
    shipped: i64,
    price: f64,
    invoice: i64,
    date: &str,
) -> LineItem {
    LineItem {
        sales_order_id: so,
        detail_id: detail,
        digi_key_product_number: dk.into(),
        manufacturer_product_number: mpn.into(),
        description: Some(desc.into()),
        quantity_ordered: ordered,
        quantity_shipped: shipped,
        quantity_back_order: ordered - shipped,
        unit_price: Some(price),
        total_price: Some(price * ordered as f64),
        pack_type: Some("CutTape".into()),
        country_of_origin: Some("US".into()),
        item_shipments: if shipped > 0 {
            vec![ItemShipment {
                quantity_shipped: shipped,
                invoice_id: Some(invoice),
                shipped_date: Some(format!("{date}T15:04:00")),
                tracking_number: Some(format!("1Z999AA1{invoice}")),
                expected_delivery_date: None,
            }]
        } else {
            vec![]
        },
        ..Default::default()
    }
}

fn status(s: &str, d: &str) -> Option<StatusInfo> {
    Some(StatusInfo {
        order_status: Some(s.into()),
        short_description: Some(d.into()),
        long_description: None,
    })
}

pub fn demo_orders() -> Vec<Order> {
    let so1 = 90123456;
    let o1 = Order {
        order_number: 81234567,
        date_entered: Some("2026-11-18T10:12:00".into()),
        currency: Some("EUR".into()),
        entire_order_status: status("Shipped", "Shipped"),
        sales_orders: vec![SalesOrder {
            sales_order_id: so1,
            order_number: 81234567,
            status: status("Shipped", "Shipped"),
            date_entered: Some("2026-11-18T10:12:00".into()),
            currency: Some("EUR".into()),
            ship_method: Some("UPS Express".into()),
            total_price: Some(80.66),
            line_items: vec![
                li(
                    so1,
                    1,
                    "296-39435-1-ND",
                    "TPS61222DCKR",
                    "IC REG BOOST 5V 200MA SC70-6",
                    10,
                    10,
                    0.859,
                    71000001,
                    "2026-11-18",
                ),
                li(
                    so1,
                    2,
                    "54-3305-0-15-80-47-27-10-0-ND",
                    "3305-0-15-80-47-27-10-0",
                    ".105\" LONG RECEPTACLE; ACCEPTS .",
                    100,
                    100,
                    0.3116,
                    71000001,
                    "2026-11-18",
                ),
                li(
                    so1,
                    3,
                    "1727-5192-1-ND",
                    "PMEG2010ER,115",
                    "DIODE SCHOTTKY 20V 1A SOD123W",
                    50,
                    50,
                    0.1634,
                    71000001,
                    "2026-11-18",
                ),
                li(
                    so1,
                    4,
                    "SX1509QBIULTRTCT-ND",
                    "SX1509QBIULTRT",
                    "IC XPNDR 400KHZ I2C 28QFN",
                    6,
                    6,
                    3.12,
                    71000001,
                    "2026-11-18",
                ),
                li(
                    so1,
                    5,
                    "ED1139-ND",
                    "3330-0-00-15-00-00-03-0",
                    "CONN PC PIN CIRC 0.030DIA GOLD",
                    100,
                    100,
                    0.1252,
                    71000001,
                    "2026-11-18",
                ),
                li(
                    so1,
                    6,
                    "445-8659-1-ND",
                    "MLZ2012M4R7HT000",
                    "FIXED IND 4.7UH 400MA 340 MOHM",
                    20,
                    20,
                    0.075,
                    71000001,
                    "2026-11-18",
                ),
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    let (sa, sb) = (89000111, 89000112);
    let o2 = Order {
        order_number: 80555123,
        date_entered: Some("2026-10-02T08:40:00".into()),
        currency: Some("EUR".into()),
        entire_order_status: status("ProcessingAwaitingBackorders", "Awaiting backorders"),
        sales_orders: vec![
            SalesOrder {
                sales_order_id: sa,
                order_number: 80555123,
                status: status("Shipped", "Shipped"),
                date_entered: Some("2026-10-02T08:40:00".into()),
                line_items: vec![
                    li(
                        sa,
                        1,
                        "1965-ESP32-S3-WROOM-1-N8R8CT-ND",
                        "ESP32-S3-WROOM-1-N8R8",
                        "RF TXRX MOD BT WIFI PCB TRACE SMD",
                        4,
                        4,
                        4.31,
                        70990001,
                        "2026-10-02",
                    ),
                    li(
                        sa,
                        2,
                        "2073-USB4105-GF-ACT-ND",
                        "USB4105-GF-A",
                        "USB-C (USB TYPE-C) RECEPTACLE CONN",
                        10,
                        10,
                        0.79,
                        70990001,
                        "2026-10-02",
                    ),
                ],
                ..Default::default()
            },
            SalesOrder {
                sales_order_id: sb,
                order_number: 80555123,
                status: status("ProcessingPartialShipment", "Partially shipped"),
                date_entered: Some("2026-10-02T08:40:00".into()),
                line_items: vec![
                    li(
                        sb,
                        1,
                        "497-STM32G031K8T6-ND",
                        "STM32G031K8T6",
                        "IC MCU 32BIT 64KB FLASH 32LQFP",
                        10,
                        5,
                        1.92,
                        70990002,
                        "2026-10-09",
                    ),
                    li(
                        sb,
                        2,
                        "296-TLV75533PDBVRCT-ND",
                        "TLV75533PDBVR",
                        "IC REG LINEAR 3.3V 500MA SOT23-5",
                        25,
                        0,
                        0.21,
                        0,
                        "",
                    ),
                ],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let so3 = 88000777;
    let o3 = Order {
        order_number: 79911000,
        date_entered: Some("2026-08-20T14:02:00".into()),
        currency: Some("EUR".into()),
        entire_order_status: status("Delivered", "Delivered"),
        sales_orders: vec![SalesOrder {
            sales_order_id: so3,
            order_number: 79911000,
            status: status("Delivered", "Delivered"),
            date_entered: Some("2026-08-20T14:02:00".into()),
            line_items: vec![
                li(
                    so3,
                    1,
                    "311-10.0KHRCT-ND",
                    "RC0603FR-0710KL",
                    "RES 10K OHM 1% 1/10W 0603",
                    100,
                    100,
                    0.0123,
                    70800001,
                    "2026-08-20",
                ),
                li(
                    so3,
                    2,
                    "1276-1935-1-ND",
                    "CL10A106KP8NNNC",
                    "CAP CER 10UF 10V X5R 0603",
                    50,
                    50,
                    0.052,
                    70800001,
                    "2026-08-20",
                ),
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    vec![o1, o2, o3]
}

fn manufacturer_for(mpn: &str) -> &'static str {
    match mpn {
        m if m.starts_with("TPS") || m.starts_with("TLV") => "Texas Instruments",
        m if m.starts_with("33") => "Mill-Max Manufacturing Corp.",
        m if m.starts_with("PMEG") => "Nexperia USA Inc.",
        m if m.starts_with("SX") => "Semtech Corporation",
        m if m.starts_with("MLZ") => "TDK Corporation",
        m if m.starts_with("ESP32") => "Espressif Systems",
        m if m.starts_with("USB4") => "GCT",
        m if m.starts_with("STM32") => "STMicroelectronics",
        m if m.starts_with("RC06") => "YAGEO",
        m if m.starts_with("CL10") => "Samsung Electro-Mechanics",
        _ => "Unknown",
    }
}

impl DigiKeyApi for MockDigiKey {
    fn orders(&self, _days: i64, _shared: bool) -> Result<Vec<Order>> {
        std::thread::sleep(std::time::Duration::from_millis(400));
        Ok(demo_orders())
    }

    fn product(&self, dk_pn: &str) -> Result<Option<Product>> {
        std::thread::sleep(std::time::Duration::from_millis(150));
        let item = demo_orders()
            .into_iter()
            .flat_map(|o| o.sales_orders)
            .flat_map(|s| s.line_items)
            .find(|l| l.digi_key_product_number == dk_pn);
        let Some(item) = item else { return Ok(None) };
        let desc = item.description.clone().unwrap_or_default();
        let package = match item.manufacturer_product_number.get(..3).unwrap_or("") {
            "TPS" => Some("SC-70-6"),
            "330" | "333" => Some("Through hole"),
            "PME" => Some("SOD-123W"),
            "SX1" => Some("28-QFN (4x4)"),
            "MLZ" => Some("0805"),
            "ESP" => Some("Module"),
            "USB" => Some("USB-C SMD"),
            "STM" => Some("32-LQFP"),
            "TLV" => Some("SOT-23-5"),
            "RC0" | "CL1" => Some("0603"),
            _ => None,
        }
        .map(|s| s.to_string());
        Ok(Some(Product {
            dk_pn: dk_pn.into(),
            mpn: item.manufacturer_product_number.clone(),
            manufacturer: manufacturer_for(&item.manufacturer_product_number).into(),
            description: desc.clone(),
            detailed_description: format!("{desc} (demo data)"),
            category: "Demo category".into(),
            package: package.clone(),
            photo_url: Some(format!("mock://{dk_pn}")),
            datasheet_url: Some(format!("mock://{dk_pn}.pdf")),
            product_url: None,
            parameters: vec![
                ("Package / Case".into(), package.unwrap_or_default()),
                ("Mounting Type".into(), "Surface Mount".into()),
            ],
        }))
    }

    fn fetch(&self, url: &str) -> Result<(Vec<u8>, String)> {
        // A generated placeholder "photo": a chip-like square, colour derived from the url.
        let h = url
            .bytes()
            .fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
        let (r, g, b) = (
            (h & 0x7f) as u8 + 60,
            ((h >> 8) & 0x7f) as u8 + 60,
            ((h >> 16) & 0x7f) as u8 + 60,
        );
        let img = image::RgbImage::from_fn(160, 160, |x, y| {
            let body = (30..130).contains(&x) && (30..130).contains(&y);
            let pin = ((x % 20) < 8)
                && ((15..30).contains(&y) || (130..145).contains(&y))
                && (35..125).contains(&x);
            if body {
                image::Rgb([r / 3, g / 3, b / 3])
            } else if pin {
                image::Rgb([200, 200, 200])
            } else {
                image::Rgb([r, g, b])
            }
        });
        let mut out = std::io::Cursor::new(vec![]);
        img.write_to(&mut out, image::ImageFormat::Png)?;
        Ok((out.into_inner(), "image/png".into()))
    }
}

// ------------------------------------------------------------------ PartsBox

#[derive(Default)]
struct Db {
    parts: Vec<Part>,
    storages: Vec<Storage>,
    orders: Vec<PbOrder>,
    images: Vec<String>,
    seq: u64,
}

impl Db {
    fn id(&mut self, prefix: &str) -> String {
        self.seq += 1;
        format!("{prefix}{:06}", self.seq)
    }
}

pub struct MockPartsBox {
    db: Mutex<Db>,
    /// simulate a plan without the Orders feature
    no_orders: bool,
}

fn stock(storage: &str, qty: i64, ts: i64) -> StockEntry {
    StockEntry {
        storage_id: Some(storage.into()),
        quantity: qty,
        timestamp: Some(ts),
        ..Default::default()
    }
}

fn part(
    id: &str,
    name: &str,
    desc: &str,
    fp: &str,
    tags: &[&str],
    mfr: &str,
    stock: Vec<StockEntry>,
) -> Part {
    Part {
        id: id.into(),
        name: name.into(),
        kind: Some("local".into()),
        description: Some(desc.into()),
        footprint: Some(fp.into()),
        tags: tags.iter().map(|s| s.to_string()).collect(),
        notes: Some(format!("Manufacturer: {mfr}")),
        stock,
        ..Default::default()
    }
}

impl MockPartsBox {
    pub fn new() -> Self {
        let mut db = Db {
            seq: 100,
            ..Default::default()
        };
        db.storages = [
            "Inbox",
            "Drawer A1",
            "Drawer A2",
            "Reel rack",
            "ESD bag box",
        ]
        .iter()
        .enumerate()
        .map(|(i, n)| Storage {
            id: format!("st{i}"),
            name: n.to_string(),
            ..Default::default()
        })
        .collect();
        let t = 1_750_000_000_000;
        db.parts = vec![
            part(
                "p1",
                "TPS61222DCKR",
                "Boost converter 5V 200mA",
                "SC70-6",
                &["power", "digikey"],
                "Texas Instruments",
                vec![stock("st1", 3, t)],
            ),
            part(
                "p2",
                "RC0603FR-0710KL",
                "RES 10K OHM 1% 0603",
                "0603",
                &["passive", "resistor"],
                "YAGEO",
                vec![stock("st1", 100, t), stock("st1", -12, t + 1)],
            ),
            part(
                "p3",
                "CL10A106KP8NNNC",
                "CAP CER 10UF 10V X5R 0603",
                "0603",
                &["passive", "capacitor"],
                "Samsung Electro-Mechanics",
                vec![stock("st2", 50, t)],
            ),
            part(
                "p4",
                "NE555DR",
                "Timer IC",
                "SOIC-8",
                &["ic"],
                "Texas Instruments",
                vec![stock("st2", 7, t)],
            ),
            part(
                "p5",
                "BSS138",
                "MOSFET N-CH 50V 220mA",
                "SOT-23",
                &["discrete"],
                "onsemi",
                vec![stock("st3", 250, t), stock("st2", 20, t)],
            ),
            part(
                "p6",
                "LM358DR",
                "Dual op-amp",
                "SOIC-8",
                &["ic", "analog"],
                "Texas Instruments",
                vec![],
            ),
            part(
                "p7",
                "GRM188R71H104KA93D",
                "CAP CER 0.1UF 50V X7R 0603",
                "0603",
                &["passive", "capacitor"],
                "Murata",
                vec![stock("st3", 4000, t)],
            ),
        ];
        // An earlier import of the August order, so it shows up as already imported.
        db.orders = vec![PbOrder {
            id: "ord-old".into(),
            vendor_name: Some("DigiKey".into()),
            number: Some("88000777".into()),
            status: Some("received".into()),
            entries: vec![
                OrderEntry {
                    id: "oe1".into(),
                    part_id: Some("p2".into()),
                    vendor_sku: Some("311-10.0KHRCT-ND".into()),
                    vendor_quantity: Some(100),
                    received_quantity: Some(100),
                    ..Default::default()
                },
                OrderEntry {
                    id: "oe2".into(),
                    part_id: Some("p3".into()),
                    vendor_sku: Some("1276-1935-1-ND".into()),
                    vendor_quantity: Some(50),
                    received_quantity: Some(50),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }];
        Self {
            db: Mutex::new(db),
            no_orders: false,
        }
    }

    #[cfg(test)]
    pub fn without_orders() -> Self {
        Self {
            no_orders: true,
            ..Self::new()
        }
    }
}

fn s(v: &Value, k: &str) -> Result<String> {
    v.get(k)
        .and_then(|x| x.as_str())
        .map(|x| x.to_string())
        .ok_or_else(|| anyhow!("missing {k}"))
}

impl PartsBoxTransport for MockPartsBox {
    fn call(&self, op: &str, p: Value) -> Result<Value> {
        std::thread::sleep(std::time::Duration::from_millis(120));
        if self.no_orders && op.starts_with("order/") {
            bail!(
                "PartsBox {op}: HTTP 401 Unauthorized forbidden Feature not present in your plan"
            );
        }
        let mut db = self.db.lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        match op {
            "part/all" => Ok(serde_json::to_value(&db.parts)?),
            "part/get" => {
                let id = s(&p, "part/id")?;
                let part = db
                    .parts
                    .iter()
                    .find(|x| x.id == id)
                    .ok_or_else(|| anyhow!("not-found"))?;
                Ok(serde_json::to_value(part)?)
            }
            "part/create" => {
                let id = db.id("p");
                let mut part: Part = serde_json::from_value(p)?;
                part.id = id.clone();
                part.created = Some(now);
                db.parts.push(part);
                Ok(json!({"part/id": id}))
            }
            "part/update" => {
                let id = s(&p, "part/id")?;
                let part = db
                    .parts
                    .iter_mut()
                    .find(|x| x.id == id)
                    .ok_or_else(|| anyhow!("not-found"))?;
                let mut merged = serde_json::to_value(&part)?;
                for (k, v) in p.as_object().cloned().unwrap_or_default() {
                    merged[k] = v;
                }
                *part = serde_json::from_value(merged)?;
                Ok(Value::Null)
            }
            "custom-field/update" => {
                let id = s(&p, "id")?;
                let part = db
                    .parts
                    .iter_mut()
                    .find(|x| x.id == id)
                    .ok_or_else(|| anyhow!("not-found"))?;
                if !part.custom_fields.is_object() {
                    part.custom_fields = json!({});
                }
                let map = part.custom_fields.as_object_mut().unwrap();
                for f in p["custom-fields"].as_array().cloned().unwrap_or_default() {
                    map.insert(s(&f, "key")?, f["value"].clone());
                }
                Ok(Value::Null)
            }
            "part/delete" => {
                let id = s(&p, "part/id")?;
                db.parts.retain(|x| x.id != id);
                Ok(Value::Null)
            }
            "part/delete-image" => {
                let id = s(&p, "part/id")?;
                let before = db.images.len();
                db.images.retain(|x| *x != id);
                if before == db.images.len() {
                    bail!("not-found")
                } else {
                    Ok(Value::Null)
                }
            }
            "storage/all" => Ok(serde_json::to_value(&db.storages)?),
            "order/all" => Ok(serde_json::to_value(&db.orders)?),
            "order/get" => {
                let id = s(&p, "order/id")?;
                Ok(serde_json::to_value(
                    db.orders
                        .iter()
                        .find(|o| o.id == id)
                        .ok_or_else(|| anyhow!("not-found"))?,
                )?)
            }
            "order/create" => {
                let id = db.id("ord");
                let mut order: PbOrder = serde_json::from_value(p["order"].clone())?;
                order.id = id.clone();
                order.status = Some("open".into());
                for e in p["entries"].as_array().cloned().unwrap_or_default() {
                    let mut entry: OrderEntry = serde_json::from_value(e)?;
                    entry.id = db.id("oe");
                    order.entries.push(entry);
                }
                db.orders.push(order.clone());
                Ok(serde_json::to_value(order)?)
            }
            "order/mark-ordered" | "order/cancel" => {
                let id = s(&p, "order/id")?;
                let o = db
                    .orders
                    .iter_mut()
                    .find(|o| o.id == id)
                    .ok_or_else(|| anyhow!("not-found"))?;
                o.status = Some(
                    if op == "order/cancel" {
                        "open"
                    } else {
                        "ordered"
                    }
                    .into(),
                );
                Ok(Value::Null)
            }
            "order/receive" => {
                let id = s(&p, "order/id")?;
                let storage = s(&p, "storage/id")?;
                let comments = p["stock/comments"].as_str().map(|x| x.to_string());
                let recv = p["order-entries"].as_array().cloned().unwrap_or_default();
                let mut new_stock = vec![];
                {
                    let o = db
                        .orders
                        .iter_mut()
                        .find(|o| o.id == id)
                        .ok_or_else(|| anyhow!("not-found"))?;
                    for r in recv {
                        let eid = s(&r, "order-entry/id")?;
                        let q = r["order-entry/received-quantity"].as_i64().unwrap_or(0);
                        if let Some(e) = o.entries.iter_mut().find(|e| e.id == eid) {
                            e.received_quantity = Some(q);
                            new_stock.push((
                                e.part_id.clone().unwrap_or_default(),
                                eid,
                                q,
                                e.price,
                            ));
                        }
                    }
                    o.status = Some("received".into());
                }
                for (pid, eid, q, price) in new_stock {
                    let sid = db.id("s");
                    if let Some(part) = db.parts.iter_mut().find(|x| x.id == pid) {
                        part.stock.push(StockEntry {
                            id: Some(sid.clone()),
                            storage_id: Some(storage.clone()),
                            quantity: q,
                            price,
                            timestamp: Some(now),
                            comments: comments.clone(),
                            part_id: Some(pid.clone()),
                            order_id: Some(id.clone()),
                            order_entry_id: Some(eid.clone()),
                            ..Default::default()
                        });
                    }
                    if let Some(e) = db
                        .orders
                        .iter_mut()
                        .find(|o| o.id == id)
                        .and_then(|o| o.entries.iter_mut().find(|e| e.id == eid))
                    {
                        e.stock_id = Some(sid);
                    }
                }
                Ok(Value::Null)
            }
            "order/get-entries" => {
                let id = s(&p, "order/id")?;
                let v: Vec<StockEntry> = db
                    .parts
                    .iter()
                    .flat_map(|x| x.stock.iter())
                    .filter(|x| x.order_id.as_deref() == Some(id.as_str()))
                    .cloned()
                    .collect();
                Ok(serde_json::to_value(v)?)
            }
            "order/unreceive" => {
                let sid = s(&p, "stock/id")?;
                let pid = s(&p, "stock/part-id")?;
                let part = db
                    .parts
                    .iter_mut()
                    .find(|x| x.id == pid)
                    .ok_or_else(|| anyhow!("not-found"))?;
                let entry = part
                    .stock
                    .iter()
                    .find(|x| x.id.as_deref() == Some(sid.as_str()))
                    .cloned()
                    .ok_or_else(|| anyhow!("not-found"))?;
                part.stock.retain(|x| x.id.as_deref() != Some(sid.as_str()));
                if let Some(e) = db
                    .orders
                    .iter_mut()
                    .find(|o| Some(o.id.as_str()) == entry.order_id.as_deref())
                    .and_then(|o| {
                        o.entries
                            .iter_mut()
                            .find(|e| Some(e.id.as_str()) == entry.order_entry_id.as_deref())
                    })
                {
                    e.received_quantity = Some(0);
                    e.stock_id = None;
                }
                Ok(Value::Null)
            }
            "stock/add" => {
                let pid = s(&p, "stock/part-id")?;
                let storage = s(&p, "stock/storage-id")?;
                let q = p["stock/quantity"].as_i64().unwrap_or(0);
                let sid = db.id("s");
                let part = db
                    .parts
                    .iter_mut()
                    .find(|x| x.id == pid)
                    .ok_or_else(|| anyhow!("not-found"))?;
                part.stock.push(StockEntry {
                    id: Some(sid.clone()),
                    storage_id: Some(storage),
                    quantity: q,
                    price: p["stock/price"].as_f64(),
                    timestamp: Some(now),
                    comments: p["stock/comments"].as_str().map(|x| x.to_string()),
                    part_id: Some(pid),
                    ..Default::default()
                });
                Ok(json!({"stock/id": sid}))
            }
            "stock/remove" => {
                // simplified: take quantity out of a storage
                let pid = s(&p["stock/source"], "source/part-id")?;
                let sid = s(&p["stock/source"], "source/storage-id")?;
                let q = p["stock/quantity"].as_i64().unwrap_or(0);
                let part = db
                    .parts
                    .iter_mut()
                    .find(|x| x.id == pid)
                    .ok_or_else(|| anyhow!("not-found"))?;
                part.stock.push(StockEntry {
                    storage_id: Some(sid),
                    quantity: -q,
                    timestamp: Some(now),
                    comments: p["stock/comments"].as_str().map(|x| x.to_string()),
                    part_id: Some(pid),
                    ..Default::default()
                });
                Ok(Value::Null)
            }
            "order/delete" => {
                let id = s(&p, "order/id")?;
                db.orders.retain(|o| o.id != id);
                Ok(Value::Null)
            }
            other => bail!("demo PartsBox: operation {other} not implemented"),
        }
    }

    fn set_image(&self, part_id: &str, _bytes: Vec<u8>, _filename: &str, _ct: &str) -> Result<()> {
        std::thread::sleep(std::time::Duration::from_millis(200));
        self.db.lock().unwrap().images.push(part_id.to_string());
        Ok(())
    }
}
