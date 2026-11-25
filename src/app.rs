use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Matcher, Utf32Str};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

use crate::config::Config;
use crate::digikey::{DigiKeyApi, LineItem, Order, Product, SalesOrder};
use crate::import::{ImportLine, ImportRequest, run_import, run_rollback};
use crate::ledger::{SharedLedger, TxnStatus};
use crate::partsbox::{Part, PartsBox, PbOrder, Storage};

// ------------------------------------------------------------------ messages

// Msg only ever crosses a channel once per event, so the fat Product variant
// isn't worth boxing.
#[allow(clippy::large_enum_variant)]
pub enum Msg {
    DkOrders(Result<Vec<Order>, String>),
    PbData(Result<PbData, String>),
    Product(String, Result<Option<Product>, String>),
    Photo(String, Result<Vec<u8>, String>),
    Progress {
        done: usize,
        total: usize,
        msg: String,
    },
    JobDone(Result<String, String>),
}

#[derive(Default, Clone)]
pub struct PbData {
    pub parts: Vec<Part>,
    pub storages: Vec<Storage>,
    pub orders: Vec<PbOrder>,
}

pub enum Loadable<T> {
    Loading,
    Ready(T),
    Failed(String),
}

// ------------------------------------------------------------------ rows

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Orders,
    Explorer,
    Ledger,
}

#[derive(Clone, Debug)]
pub enum ORow {
    Order(usize),
    Sales(usize, usize),
    Item(usize, usize, usize),
}

#[derive(Clone, Debug)]
pub struct Row<K> {
    pub id: String,
    pub depth: u16,
    pub kind: K,
    pub expandable: bool,
    pub expanded: bool,
}

#[derive(Clone, Debug)]
pub enum ERow {
    Group {
        label: String,
        parts: usize,
        qty: i64,
    },
    Part {
        idx: usize,
        qty: i64,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Storage,
    Tag,
    Footprint,
    Manufacturer,
    Flat,
}

impl Group {
    pub fn next(self) -> Group {
        match self {
            Group::Storage => Group::Tag,
            Group::Tag => Group::Footprint,
            Group::Footprint => Group::Manufacturer,
            Group::Manufacturer => Group::Flat,
            Group::Flat => Group::Storage,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Group::Storage => "storage",
            Group::Tag => "tag",
            Group::Footprint => "footprint",
            Group::Manufacturer => "manufacturer",
            Group::Flat => "flat",
        }
    }
}

pub enum Popup {
    Help,
    Open {
        title: String,
        choices: Vec<(char, String, String)>,
    },
    Storage {
        lines: Vec<ImportLine>,
        summary: String,
        filter: String,
        sel: usize,
    },
    ConfirmRollback {
        txn_id: String,
        summary: String,
        warning: String,
    },
    Message(String),
}

pub struct Job {
    pub title: String,
    pub done: usize,
    pub total: usize,
    pub msg: String,
    pub log: Vec<String>,
}

pub struct Search {
    pub query: String,
    pub editing: bool,
}

/// Import state of a DigiKey line item.
pub struct ItemStatus {
    pub part: Option<usize>,
    pub on_hand: i64,
    pub imported: i64,
    pub target: i64,
}

impl ItemStatus {
    pub fn remaining(&self) -> i64 {
        (self.target - self.imported).max(0)
    }
}

// ------------------------------------------------------------------ app

pub struct App {
    pub cfg: Config,
    pub demo: bool,
    pub dk: Arc<dyn DigiKeyApi>,
    pub pb: Arc<PartsBox>,
    pub ledger: SharedLedger,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,

    pub screen: Screen,
    pub quit: bool,
    pub tick: u64,
    pub status: String,

    pub orders: Loadable<Vec<Order>>,
    pub pb_data: Loadable<PbData>,
    pub products: HashMap<String, Loadable<Option<Product>>>,
    pub photos: HashMap<String, Loadable<StatefulProtocol>>,
    pub picker: Picker,

    // derived from PartsBox data
    pub part_index: HashMap<String, usize>,
    pub pb_imported: HashMap<(i64, String), i64>,
    pub storage_names: HashMap<String, String>,

    pub o_rows: Vec<Row<ORow>>,
    pub o_state: TableState,
    pub o_collapsed: HashSet<String>,
    pub o_search: Search,

    pub group: Group,
    pub e_rows: Vec<Row<ERow>>,
    pub e_state: TableState,
    pub e_expanded: HashSet<String>,
    pub e_search: Search,

    pub l_state: TableState,

    pub detail: bool,
    pub popup: Option<Popup>,
    pub job: Option<Job>,
}

fn fuzzy_rank<T>(query: &str, items: Vec<(T, String)>) -> Vec<(T, u32)> {
    let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);
    let pat = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut buf = vec![];
    let mut out: Vec<(T, u32)> = items
        .into_iter()
        .filter_map(|(t, hay)| {
            pat.score(Utf32Str::new(&hay, &mut buf), &mut matcher)
                .map(|s| (t, s))
        })
        .collect();
    out.sort_by_key(|b| std::cmp::Reverse(b.1));
    out
}

impl App {
    pub fn new(
        cfg: Config,
        demo: bool,
        dk: Arc<dyn DigiKeyApi>,
        pb: Arc<PartsBox>,
        ledger: SharedLedger,
        picker: Picker,
    ) -> App {
        let (tx, rx) = channel();
        let mut app = App {
            cfg,
            demo,
            dk,
            pb,
            ledger,
            tx,
            rx,
            screen: Screen::Orders,
            quit: false,
            tick: 0,
            status: String::new(),
            orders: Loadable::Loading,
            pb_data: Loadable::Loading,
            products: HashMap::new(),
            photos: HashMap::new(),
            picker,
            part_index: HashMap::new(),
            pb_imported: HashMap::new(),
            storage_names: HashMap::new(),
            o_rows: vec![],
            o_state: TableState::default().with_selected(0),
            o_collapsed: HashSet::new(),
            o_search: Search {
                query: String::new(),
                editing: false,
            },
            group: Group::Storage,
            e_rows: vec![],
            e_state: TableState::default().with_selected(0),
            e_expanded: HashSet::new(),
            e_search: Search {
                query: String::new(),
                editing: false,
            },
            l_state: TableState::default().with_selected(0),
            detail: false,
            popup: None,
            job: None,
        };
        app.reload_all();
        app
    }

    // -------------------------------------------------------------- background work

    fn spawn<F: FnOnce() -> Msg + Send + 'static>(&self, f: F) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
    }

    pub fn reload_all(&mut self) {
        self.orders = Loadable::Loading;
        let dk = self.dk.clone();
        let (days, shared) = (self.cfg.order_history_days, self.cfg.shared_orders);
        self.spawn(move || Msg::DkOrders(dk.orders(days, shared).map_err(|e| format!("{e:#}"))));
        self.reload_pb();
    }

    pub fn reload_pb(&mut self) {
        if !matches!(self.pb_data, Loadable::Ready(_)) {
            self.pb_data = Loadable::Loading;
        }
        let pb = self.pb.clone();
        self.spawn(move || {
            Msg::PbData(
                (|| -> anyhow::Result<PbData> {
                    // orders are a plan feature; without them we only lose the "already
                    // imported" marks taken from PartsBox (the ledger still provides them)
                    Ok(PbData {
                        parts: pb.parts()?,
                        storages: pb.storages()?,
                        orders: pb.orders().unwrap_or_default(),
                    })
                })()
                .map_err(|e| format!("{e:#}")),
            )
        });
    }

    fn request_product(&mut self, dk_pn: &str) {
        if self.products.contains_key(dk_pn) {
            return;
        }
        self.products.insert(dk_pn.to_string(), Loadable::Loading);
        let dk = self.dk.clone();
        let pn = dk_pn.to_string();
        self.spawn(move || {
            let r = dk.product(&pn).map_err(|e| format!("{e:#}"));
            Msg::Product(pn, r)
        });
    }

    fn request_photo(&mut self, dk_pn: &str) {
        if self.photos.contains_key(dk_pn) {
            return;
        }
        let url = match self.products.get(dk_pn) {
            Some(Loadable::Ready(Some(p))) => p.photo_url.clone(),
            _ => return,
        };
        let Some(url) = url else {
            self.photos
                .insert(dk_pn.to_string(), Loadable::Failed("no photo".into()));
            return;
        };
        self.photos.insert(dk_pn.to_string(), Loadable::Loading);
        let dk = self.dk.clone();
        let pn = dk_pn.to_string();
        let cache = crate::config::cache_dir().join("photos");
        self.spawn(move || {
            let safe: String = pn
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let path = cache.join(safe);
            if let Ok(b) = std::fs::read(&path) {
                return Msg::Photo(pn, Ok(b));
            }
            let r = dk.fetch(&url).map(|(b, _)| {
                let _ = std::fs::create_dir_all(&cache);
                let _ = std::fs::write(&path, &b);
                b
            });
            Msg::Photo(pn, r.map_err(|e| format!("{e:#}")))
        });
    }

    pub fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::DkOrders(r) => {
                    self.orders = match r {
                        Ok(o) => Loadable::Ready(o),
                        Err(e) => Loadable::Failed(e),
                    };
                    self.rebuild_orders();
                }
                Msg::PbData(r) => {
                    match r {
                        Ok(d) => {
                            self.index_pb(&d);
                            self.pb_data = Loadable::Ready(d);
                        }
                        Err(e) => self.pb_data = Loadable::Failed(e),
                    }
                    self.rebuild_orders();
                    self.rebuild_explorer();
                }
                Msg::Product(pn, r) => {
                    self.products.insert(
                        pn,
                        match r {
                            Ok(p) => Loadable::Ready(p),
                            Err(e) => Loadable::Failed(e),
                        },
                    );
                }
                Msg::Photo(pn, r) => {
                    let v = match r
                        .and_then(|b| image::load_from_memory(&b).map_err(|e| e.to_string()))
                    {
                        Ok(img) => Loadable::Ready(self.picker.new_resize_protocol(img)),
                        Err(e) => Loadable::Failed(e),
                    };
                    self.photos.insert(pn, v);
                }
                Msg::Progress { done, total, msg } => {
                    if let Some(j) = &mut self.job {
                        j.done = done;
                        j.total = total;
                        j.log.push(msg.clone());
                        j.msg = msg;
                    }
                }
                Msg::JobDone(r) => {
                    let title = self
                        .job
                        .as_ref()
                        .map(|j| j.title.clone())
                        .unwrap_or_default();
                    self.job = None;
                    match r {
                        Ok(m) => self.status = m,
                        Err(e) => {
                            self.popup = Some(Popup::Message(format!("{title} failed:\n\n{e}")))
                        }
                    }
                    self.reload_pb();
                }
            }
        }
        self.ensure_detail();
    }

    fn index_pb(&mut self, d: &PbData) {
        self.part_index.clear();
        for (i, p) in d.parts.iter().enumerate() {
            if let Some(m) = p.mpn.as_ref().filter(|m| !m.is_empty()) {
                self.part_index.insert(m.to_lowercase(), i);
            }
            self.part_index.entry(p.name.to_lowercase()).or_insert(i);
        }
        self.pb_imported.clear();
        for o in &d.orders {
            let vendor = o.vendor_name.as_deref().unwrap_or("").to_lowercase();
            if !vendor.contains("digi") {
                continue;
            }
            let Some(so) = o
                .number
                .as_deref()
                .and_then(|n| n.trim().parse::<i64>().ok())
            else {
                continue;
            };
            for e in &o.entries {
                if let Some(sku) = &e.vendor_sku {
                    *self.pb_imported.entry((so, sku.clone())).or_default() +=
                        e.received_quantity.unwrap_or(0);
                }
            }
        }
        self.storage_names = d
            .storages
            .iter()
            .map(|s| (s.id.clone(), s.name.clone()))
            .collect();
    }

    // -------------------------------------------------------------- derived info

    pub fn pb(&self) -> Option<&PbData> {
        match &self.pb_data {
            Loadable::Ready(d) => Some(d),
            _ => None,
        }
    }

    pub fn orders(&self) -> &[Order] {
        match &self.orders {
            Loadable::Ready(o) => o,
            _ => &[],
        }
    }

    pub fn ledger_imported(&self, so: i64, detail: i64) -> i64 {
        let l = self.ledger.lock().unwrap();
        l.txns
            .iter()
            .filter(|t| t.active())
            .flat_map(|t| t.lines.iter())
            .filter(|x| x.sales_order_id == so && x.detail_id == detail)
            .map(|x| x.quantity)
            .sum()
    }

    pub fn item_status(&self, it: &LineItem) -> ItemStatus {
        let key = if it.manufacturer_product_number.is_empty() {
            it.digi_key_product_number.to_lowercase()
        } else {
            it.manufacturer_product_number.to_lowercase()
        };
        let part = self.part_index.get(&key).copied();
        let on_hand = part
            .and_then(|i| self.pb().map(|d| d.parts[i].on_hand()))
            .unwrap_or(0);
        let from_pb = self
            .pb_imported
            .get(&(it.sales_order_id, it.digi_key_product_number.clone()))
            .copied()
            .unwrap_or(0);
        // PartsBox orders are the source of truth (also catches imports done by hand);
        // the ledger covers the time before PartsBox data is (re)loaded.
        let imported = from_pb.max(self.ledger_imported(it.sales_order_id, it.detail_id));
        let target = if self.cfg.import_quantity == "ordered" {
            it.quantity_ordered
        } else {
            it.quantity_shipped
        };
        ItemStatus {
            part,
            on_hand,
            imported,
            target,
        }
    }

    // -------------------------------------------------------------- orders tree

    pub fn rebuild_orders(&mut self) {
        let sel_id = self
            .o_state
            .selected()
            .and_then(|i| self.o_rows.get(i))
            .map(|r| r.id.clone());
        let mut rows = vec![];
        let query = self.o_search.query.trim().to_string();
        let filter: Option<HashSet<String>> = if query.is_empty() {
            None
        } else {
            let mut hay = vec![];
            for o in self.orders() {
                for s in &o.sales_orders {
                    for it in &s.line_items {
                        hay.push((
                            format!("i{}-{}", s.sales_order_id, it.detail_id),
                            format!(
                                "{} {} {} {} {}",
                                it.manufacturer_product_number,
                                it.digi_key_product_number,
                                it.description.clone().unwrap_or_default(),
                                o.order_number,
                                s.sales_order_id
                            ),
                        ));
                    }
                }
            }
            Some(
                fuzzy_rank(&query, hay)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect(),
            )
        };
        for (oi, o) in self.orders().iter().enumerate() {
            let oid = format!("o{}", o.order_number);
            let matches = |s: &SalesOrder, it: &LineItem| {
                filter
                    .as_ref()
                    .is_none_or(|f| f.contains(&format!("i{}-{}", s.sales_order_id, it.detail_id)))
            };
            if filter.is_some()
                && !o
                    .sales_orders
                    .iter()
                    .any(|s| s.line_items.iter().any(|it| matches(s, it)))
            {
                continue;
            }
            let open = filter.is_some() || !self.o_collapsed.contains(&oid);
            rows.push(Row {
                id: oid,
                depth: 0,
                kind: ORow::Order(oi),
                expandable: true,
                expanded: open,
            });
            if !open {
                continue;
            }
            let multi = o.sales_orders.len() > 1;
            for (si, s) in o.sales_orders.iter().enumerate() {
                let mut depth = 1;
                if multi {
                    let sid = format!("s{}", s.sales_order_id);
                    let sopen = filter.is_some() || !self.o_collapsed.contains(&sid);
                    rows.push(Row {
                        id: sid,
                        depth: 1,
                        kind: ORow::Sales(oi, si),
                        expandable: true,
                        expanded: sopen,
                    });
                    if !sopen {
                        continue;
                    }
                    depth = 2;
                }
                for (li, it) in s.line_items.iter().enumerate() {
                    if !matches(s, it) {
                        continue;
                    }
                    rows.push(Row {
                        id: format!("i{}-{}", s.sales_order_id, it.detail_id),
                        depth,
                        kind: ORow::Item(oi, si, li),
                        expandable: false,
                        expanded: false,
                    });
                }
            }
        }
        self.o_rows = rows;
        restore_selection(&mut self.o_state, &self.o_rows, sel_id);
    }

    // -------------------------------------------------------------- explorer tree

    pub fn rebuild_explorer(&mut self) {
        let sel_id = self
            .e_state
            .selected()
            .and_then(|i| self.e_rows.get(i))
            .map(|r| r.id.clone());
        let Some(d) = self.pb() else {
            self.e_rows.clear();
            return;
        };
        let query = self.e_search.query.trim().to_string();
        let ranked: Vec<(usize, u32)> = if query.is_empty() {
            let mut v: Vec<usize> = (0..d.parts.len()).collect();
            v.sort_by_key(|&i| d.parts[i].name.to_lowercase());
            v.into_iter().map(|i| (i, 0)).collect()
        } else {
            let hay = d
                .parts
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    (
                        i,
                        format!(
                            "{} {} {} {} {} {}",
                            p.name,
                            p.description.clone().unwrap_or_default(),
                            p.footprint.clone().unwrap_or_default(),
                            p.mpn.clone().unwrap_or_default(),
                            p.manufacturer_guess().unwrap_or_default(),
                            p.tags.join(" ")
                        ),
                    )
                })
                .collect();
            fuzzy_rank(&query, hay)
        };

        // group key(s) and quantity for each part
        let mut groups: std::collections::BTreeMap<String, Vec<(usize, i64)>> = Default::default();
        let mut flat = vec![];
        for (i, _) in &ranked {
            let p = &d.parts[*i];
            match self.group {
                Group::Flat => flat.push((*i, p.on_hand())),
                Group::Storage => {
                    let by = p.on_hand_by_storage();
                    if by.is_empty() {
                        groups
                            .entry("~ (no stock)".into())
                            .or_default()
                            .push((*i, 0));
                    }
                    for (sid, q) in by {
                        let name = self.storage_names.get(&sid).cloned().unwrap_or(sid);
                        groups.entry(name).or_default().push((*i, q));
                    }
                }
                Group::Tag => {
                    if p.tags.is_empty() {
                        groups
                            .entry("~ (untagged)".into())
                            .or_default()
                            .push((*i, p.on_hand()));
                    }
                    for t in &p.tags {
                        groups.entry(t.clone()).or_default().push((*i, p.on_hand()));
                    }
                }
                Group::Footprint => {
                    let k = p
                        .footprint
                        .clone()
                        .filter(|f| !f.is_empty())
                        .unwrap_or("~ (none)".into());
                    groups.entry(k).or_default().push((*i, p.on_hand()));
                }
                Group::Manufacturer => {
                    let k = p.manufacturer_guess().unwrap_or("~ (unknown)".into());
                    groups.entry(k).or_default().push((*i, p.on_hand()));
                }
            }
        }
        let mut rows = vec![];
        if self.group == Group::Flat {
            for (i, q) in flat {
                rows.push(Row {
                    id: format!("p{}", d.parts[i].id),
                    depth: 0,
                    kind: ERow::Part { idx: i, qty: q },
                    expandable: false,
                    expanded: false,
                });
            }
        } else {
            for (label, parts) in groups {
                let gid = format!("g{}:{}", self.group.label(), label);
                let open = !query.is_empty() || self.e_expanded.contains(&gid);
                let display = label.trim_start_matches("~ ").to_string();
                rows.push(Row {
                    id: gid.clone(),
                    depth: 0,
                    kind: ERow::Group {
                        label: display,
                        parts: parts.len(),
                        qty: parts.iter().map(|x| x.1).sum(),
                    },
                    expandable: true,
                    expanded: open,
                });
                if open {
                    for (i, q) in parts {
                        rows.push(Row {
                            id: format!("{gid}/p{}", d.parts[i].id),
                            depth: 1,
                            kind: ERow::Part { idx: i, qty: q },
                            expandable: false,
                            expanded: false,
                        });
                    }
                }
            }
        }
        self.e_rows = rows;
        restore_selection(&mut self.e_state, &self.e_rows, sel_id);
    }

    // -------------------------------------------------------------- selection helpers

    pub fn selected_item(&self) -> Option<&LineItem> {
        let r = self.o_rows.get(self.o_state.selected()?)?;
        match r.kind {
            ORow::Item(oi, si, li) => self
                .orders()
                .get(oi)?
                .sales_orders
                .get(si)?
                .line_items
                .get(li),
            _ => None,
        }
    }

    pub fn selected_part(&self) -> Option<&Part> {
        let r = self.e_rows.get(self.e_state.selected()?)?;
        match r.kind {
            ERow::Part { idx, .. } => self.pb()?.parts.get(idx),
            _ => None,
        }
    }

    /// DigiKey part number for a PartsBox part (from the notes we write, or an import).
    pub fn part_dk_pn(&self, p: &Part) -> Option<String> {
        if let Some(n) = &p.notes
            && let Some(pn) = n.lines().find_map(|l| l.strip_prefix("DigiKey: "))
        {
            return Some(pn.trim().to_string());
        }
        let l = self.ledger.lock().unwrap();
        l.txns
            .iter()
            .flat_map(|t| t.lines.iter())
            .find(|x| x.part_id == p.id)
            .map(|x| x.dk_pn.clone())
    }

    fn ensure_detail(&mut self) {
        if !self.detail {
            return;
        }
        let pn = match self.screen {
            Screen::Orders => self
                .selected_item()
                .map(|i| i.digi_key_product_number.clone()),
            Screen::Explorer => self.selected_part().and_then(|p| self.part_dk_pn(p)),
            Screen::Ledger => None,
        };
        if let Some(pn) = pn {
            self.request_product(&pn);
            self.request_photo(&pn);
        }
    }

    // -------------------------------------------------------------- import

    fn lines_for_row(&self, row: &ORow) -> (Vec<ImportLine>, String) {
        let orders = self.orders();
        let mk = |o: &Order, s: &SalesOrder, it: &LineItem| -> Option<ImportLine> {
            let st = self.item_status(it);
            (st.remaining() > 0).then(|| ImportLine {
                sales_order_id: s.sales_order_id,
                order_number: o.order_number,
                po_number: o.po_number.clone().or(s.purchase_order.clone()),
                order_date: s.date_entered.clone().or(o.date_entered.clone()),
                item: it.clone(),
                quantity: st.remaining(),
            })
        };
        match *row {
            ORow::Order(oi) => {
                let o = &orders[oi];
                let v = o
                    .sales_orders
                    .iter()
                    .flat_map(|s| s.line_items.iter().filter_map(move |it| mk(o, s, it)))
                    .collect();
                (v, format!("DigiKey order {}", o.order_number))
            }
            ORow::Sales(oi, si) => {
                let (o, s) = (&orders[oi], &orders[oi].sales_orders[si]);
                (
                    s.line_items.iter().filter_map(|it| mk(o, s, it)).collect(),
                    format!("DigiKey sales order {}", s.sales_order_id),
                )
            }
            ORow::Item(oi, si, li) => {
                let (o, s) = (&orders[oi], &orders[oi].sales_orders[si]);
                let it = &s.line_items[li];
                (
                    mk(o, s, it).into_iter().collect(),
                    format!(
                        "{} from sales order {}",
                        it.manufacturer_product_number, s.sales_order_id
                    ),
                )
            }
        }
    }

    fn begin_import(&mut self) {
        if self.job.is_some() {
            self.status = "A job is already running".into();
            return;
        }
        let Some(d) = self.pb() else {
            self.status = "PartsBox data not loaded yet".into();
            return;
        };
        if d.storages.is_empty() {
            self.popup = Some(Popup::Message(
                "No storage locations in PartsBox. Create one first.".into(),
            ));
            return;
        }
        let Some(row) = self
            .o_state
            .selected()
            .and_then(|i| self.o_rows.get(i))
            .map(|r| r.kind.clone())
        else {
            return;
        };
        let (lines, summary) = self.lines_for_row(&row);
        if lines.is_empty() {
            self.status = format!(
                "Nothing to import: everything {} is already imported",
                if self.cfg.import_quantity == "ordered" {
                    "ordered"
                } else {
                    "shipped"
                }
            );
            return;
        }
        let sel = self
            .storage_list("")
            .iter()
            .position(|s| s.name == self.cfg.default_storage)
            .unwrap_or(0);
        self.popup = Some(Popup::Storage {
            lines,
            summary,
            filter: String::new(),
            sel,
        });
    }

    pub fn storage_list(&self, filter: &str) -> Vec<Storage> {
        let Some(d) = self.pb() else { return vec![] };
        let mut v = d.storages.clone();
        v.sort_by_key(|s| s.name.to_lowercase());
        if filter.is_empty() {
            return v;
        }
        fuzzy_rank(
            filter,
            v.into_iter()
                .map(|s| {
                    let n = s.name.clone();
                    (s, n)
                })
                .collect(),
        )
        .into_iter()
        .map(|(s, _)| s)
        .collect()
    }

    fn start_import(&mut self, lines: Vec<ImportLine>, summary: String, storage: Storage) {
        let part_index: HashMap<String, String> = match self.pb() {
            Some(d) => self
                .part_index
                .iter()
                .map(|(k, &i)| (k.clone(), d.parts[i].id.clone()))
                .collect(),
            None => HashMap::new(),
        };
        let n = lines.len();
        let req = ImportRequest {
            summary: summary.clone(),
            storage_id: storage.id.clone(),
            storage_name: storage.name.clone(),
            lines,
            part_index,
        };
        self.job = Some(Job {
            title: format!("Importing {summary}"),
            done: 0,
            total: n,
            msg: "starting…".into(),
            log: vec![],
        });
        let (cfg, dk, pb, ledger, tx) = (
            self.cfg.clone(),
            self.dk.clone(),
            self.pb.clone(),
            self.ledger.clone(),
            self.tx.clone(),
        );
        std::thread::spawn(move || {
            let ptx = tx.clone();
            let progress = move |done, total, msg| {
                let _ = ptx.send(Msg::Progress { done, total, msg });
            };
            let r = run_import(req, &cfg, dk.as_ref(), &pb, &ledger, &progress);
            let _ = tx.send(Msg::JobDone(
                r.map(|t| format!("Imported {} lines into {} (transaction {})", t.lines.len(), storage.name, t.id))
                    .map_err(|e| format!("{e:#}\n\nEverything done so far is recorded in the ledger and can be rolled back (screen 3).")),
            ));
        });
    }

    fn start_rollback(&mut self, txn_id: String) {
        self.job = Some(Job {
            title: format!("Rolling back {txn_id}"),
            done: 0,
            total: 1,
            msg: "starting…".into(),
            log: vec![],
        });
        let (pb, ledger, tx) = (self.pb.clone(), self.ledger.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let ptx = tx.clone();
            let progress = move |done, total, msg| {
                let _ = ptx.send(Msg::Progress { done, total, msg });
            };
            let r = run_rollback(&txn_id, &pb, &ledger, &progress);
            let _ = tx.send(Msg::JobDone(match r {
                Ok(t) if t.status == TxnStatus::RolledBack => Ok(format!("Rolled back {txn_id}")),
                Ok(t) => Err(format!(
                    "Some steps could not be undone:\n\n{}",
                    t.log
                        .iter()
                        .filter(|l| l.contains("FAILED"))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n")
                )),
                Err(e) => Err(format!("{e:#}")),
            }));
        });
    }

    // -------------------------------------------------------------- opening things

    fn open_url(&mut self, url: &str) {
        match open::that_detached(url) {
            Ok(()) => self.status = format!("Opened {url}"),
            Err(e) => self.status = format!("Could not open browser: {e} – {url}"),
        }
    }

    fn so_url(&self, so: i64) -> String {
        self.cfg.url(
            &self.cfg.url_digikey_order,
            &[("sales_order", &so.to_string())],
        )
    }

    fn product_url(&self, dk_pn: &str) -> String {
        match self.products.get(dk_pn) {
            Some(Loadable::Ready(Some(p))) if p.product_url.is_some() => {
                p.product_url.clone().unwrap()
            }
            _ => self
                .cfg
                .url(&self.cfg.url_digikey_product, &[("dk_pn", dk_pn)]),
        }
    }

    fn part_url(&self, id: &str) -> String {
        self.cfg
            .url(&self.cfg.url_partsbox_part, &[("part_id", id)])
    }

    fn enter_orders(&mut self) {
        let Some(row) = self
            .o_state
            .selected()
            .and_then(|i| self.o_rows.get(i))
            .cloned()
        else {
            return;
        };
        match row.kind {
            ORow::Order(oi) => {
                let o = &self.orders()[oi];
                if o.sales_orders.len() == 1 {
                    let url = self.so_url(o.sales_orders[0].sales_order_id);
                    self.open_url(&url);
                } else {
                    let choices = o
                        .sales_orders
                        .iter()
                        .enumerate()
                        .map(|(i, s)| {
                            (
                                char::from_digit((i + 1) as u32 % 10, 10).unwrap_or('0'),
                                format!("Sales order {}", s.sales_order_id),
                                self.so_url(s.sales_order_id),
                            )
                        })
                        .collect();
                    self.popup = Some(Popup::Open {
                        title: format!("Order {} has several sales orders", o.order_number),
                        choices,
                    });
                }
            }
            ORow::Sales(oi, si) => {
                let url = self.so_url(self.orders()[oi].sales_orders[si].sales_order_id);
                self.open_url(&url);
            }
            ORow::Item(oi, si, li) => {
                let it = self.orders()[oi].sales_orders[si].line_items[li].clone();
                let mut choices = vec![(
                    'd',
                    format!("{} on DigiKey", it.digi_key_product_number),
                    self.product_url(&it.digi_key_product_number),
                )];
                if let Some(i) = self.item_status(&it).part {
                    let id = self.pb().unwrap().parts[i].id.clone();
                    choices.push((
                        'p',
                        format!("{} in PartsBox", it.manufacturer_product_number),
                        self.part_url(&id),
                    ));
                }
                choices.push((
                    'o',
                    format!("Sales order {}", it.sales_order_id),
                    self.so_url(it.sales_order_id),
                ));
                self.popup = Some(Popup::Open {
                    title: "Open in browser".into(),
                    choices,
                });
            }
        }
    }

    // -------------------------------------------------------------- keys

    pub fn on_key(&mut self, k: KeyEvent) {
        if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if self.popup.is_some() {
            self.on_popup_key(k);
            self.ensure_detail();
            return;
        }
        // search input
        let search_editing = match self.screen {
            Screen::Orders => self.o_search.editing,
            Screen::Explorer => self.e_search.editing,
            Screen::Ledger => false,
        };
        if search_editing {
            self.on_search_key(k);
            self.ensure_detail();
            return;
        }
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.popup = Some(Popup::Help),
            KeyCode::Tab => self.screen = next_screen(self.screen, 1),
            KeyCode::BackTab => self.screen = next_screen(self.screen, 2),
            KeyCode::Char('1') => self.screen = Screen::Orders,
            KeyCode::Char('2') => self.screen = Screen::Explorer,
            KeyCode::Char('3') => self.screen = Screen::Ledger,
            KeyCode::Char('r') | KeyCode::F(5) => {
                self.products
                    .retain(|_, v| !matches!(v, Loadable::Failed(_)));
                self.reload_all();
                self.status = "Reloading…".into();
            }
            KeyCode::Char(' ') => self.detail = !self.detail,
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::PageDown => self.move_sel(15),
            KeyCode::PageUp => self.move_sel(-15),
            KeyCode::Home => self.move_sel(-100_000),
            KeyCode::End => self.move_sel(100_000),
            KeyCode::Right => self.screen = next_screen(self.screen, 1),
            KeyCode::Left => self.screen = next_screen(self.screen, 2),
            KeyCode::Char('l') => self.expand(true),
            KeyCode::Char('h') => self.expand(false),
            KeyCode::Char('/') => match self.screen {
                Screen::Orders => self.o_search.editing = true,
                Screen::Explorer => self.e_search.editing = true,
                Screen::Ledger => {}
            },
            KeyCode::Esc => self.clear_search(),
            _ => match self.screen {
                Screen::Orders => self.on_orders_key(k),
                Screen::Explorer => self.on_explorer_key(k),
                Screen::Ledger => self.on_ledger_key(k),
            },
        }
        self.ensure_detail();
    }

    fn clear_search(&mut self) {
        match self.screen {
            Screen::Orders => {
                self.o_search.query.clear();
                self.rebuild_orders();
            }
            Screen::Explorer => {
                self.e_search.query.clear();
                self.rebuild_explorer();
            }
            Screen::Ledger => {}
        }
    }

    fn on_search_key(&mut self, k: KeyEvent) {
        let s = if self.screen == Screen::Orders {
            &mut self.o_search
        } else {
            &mut self.e_search
        };
        match k.code {
            KeyCode::Esc => {
                s.editing = false;
                s.query.clear();
            }
            KeyCode::Enter => s.editing = false,
            KeyCode::Down | KeyCode::Up => {
                s.editing = false;
                let d = if k.code == KeyCode::Down { 1 } else { -1 };
                self.move_sel(d);
                return;
            }
            KeyCode::Backspace => {
                s.query.pop();
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => s.query.clear(),
            KeyCode::Char(c) => s.query.push(c),
            _ => {}
        }
        if self.screen == Screen::Orders {
            self.rebuild_orders();
            // jump to first item row
            if let Some(i) = self
                .o_rows
                .iter()
                .position(|r| matches!(r.kind, ORow::Item(..)))
            {
                self.o_state.select(Some(i));
            }
        } else {
            self.rebuild_explorer();
            if let Some(i) = self
                .e_rows
                .iter()
                .position(|r| matches!(r.kind, ERow::Part { .. }))
            {
                self.e_state.select(Some(i));
            }
        }
    }

    fn on_orders_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Enter => self.enter_orders(),
            KeyCode::Char('i') => self.begin_import(),
            KeyCode::Char('d') => {
                if let Some(it) = self.selected_item().cloned() {
                    let u = self.product_url(&it.digi_key_product_number);
                    self.open_url(&u);
                }
            }
            KeyCode::Char('p') => {
                if let Some(it) = self.selected_item().cloned() {
                    match self.item_status(&it).part {
                        Some(i) => {
                            let u = self.part_url(&self.pb().unwrap().parts[i].id.clone());
                            self.open_url(&u);
                        }
                        None => self.status = "Not in PartsBox yet".into(),
                    }
                }
            }
            _ => {}
        }
    }

    fn on_explorer_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char('g') => {
                self.group = self.group.next();
                self.e_state.select(Some(0));
                self.rebuild_explorer();
                self.status = format!("Grouped by {}", self.group.label());
            }
            KeyCode::Char('f') => {
                self.group = if self.group == Group::Flat {
                    Group::Storage
                } else {
                    Group::Flat
                };
                self.rebuild_explorer();
            }
            KeyCode::Char('e') => {
                // expand / collapse all groups
                let all: Vec<String> = self
                    .e_rows
                    .iter()
                    .filter(|r| r.expandable)
                    .map(|r| r.id.clone())
                    .collect();
                if all.iter().all(|g| self.e_expanded.contains(g)) {
                    self.e_expanded.clear();
                } else {
                    self.e_expanded.extend(all);
                }
                self.rebuild_explorer();
            }
            KeyCode::Enter => {
                let row = self
                    .e_state
                    .selected()
                    .and_then(|i| self.e_rows.get(i))
                    .cloned();
                match row.map(|r| r.kind) {
                    Some(ERow::Group { .. }) => self.toggle_group(),
                    Some(ERow::Part { idx, .. }) => {
                        let id = self.pb().unwrap().parts[idx].id.clone();
                        let u = self.part_url(&id);
                        self.open_url(&u);
                    }
                    None => {}
                }
            }
            KeyCode::Char('d') => {
                if let Some(pn) = self.selected_part().and_then(|p| self.part_dk_pn(p)) {
                    let u = self.product_url(&pn);
                    self.open_url(&u);
                }
            }
            _ => {}
        }
    }

    fn on_ledger_key(&mut self, k: KeyEvent) {
        if let KeyCode::Char('u') | KeyCode::Delete = k.code {
            if self.job.is_some() {
                self.status = "A job is running".into();
                return;
            }
            let l = self.ledger.lock().unwrap();
            let Some(t) = self.l_state.selected().and_then(|i| l.txns.get(i)).cloned() else {
                return;
            };
            drop(l);
            if t.status == TxnStatus::RolledBack {
                self.status = "Already rolled back".into();
                return;
            }
            let newer = self
                .ledger
                .lock()
                .unwrap()
                .txns
                .iter()
                .take_while(|x| x.id != t.id)
                .filter(|x| x.active())
                .count();
            let warning = if newer > 0 {
                format!(
                    "{newer} newer transaction(s) are still active. Rolling back out of order is usually fine, but check parts shared between them."
                )
            } else {
                String::new()
            };
            self.popup = Some(Popup::ConfirmRollback {
                txn_id: t.id.clone(),
                summary: t.summary.clone(),
                warning,
            });
        }
    }

    fn on_popup_key(&mut self, k: KeyEvent) {
        let Some(popup) = self.popup.take() else {
            return;
        };
        match popup {
            Popup::Help | Popup::Message(_) => {} // any key closes
            Popup::Open { title, choices } => {
                let pick = match k.code {
                    KeyCode::Char(c) => choices.iter().find(|x| x.0 == c).map(|x| x.2.clone()),
                    KeyCode::Enter => choices.first().map(|x| x.2.clone()),
                    _ => None,
                };
                match (pick, k.code) {
                    (Some(url), _) => self.open_url(&url),
                    (None, KeyCode::Esc) | (None, KeyCode::Char('q')) => {}
                    _ => self.popup = Some(Popup::Open { title, choices }),
                }
            }
            Popup::ConfirmRollback {
                txn_id,
                summary,
                warning,
            } => match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.start_rollback(txn_id),
                KeyCode::Char('n') | KeyCode::Esc => {}
                _ => {
                    self.popup = Some(Popup::ConfirmRollback {
                        txn_id,
                        summary,
                        warning,
                    })
                }
            },
            Popup::Storage {
                lines,
                summary,
                mut filter,
                mut sel,
            } => {
                let list = self.storage_list(&filter);
                match k.code {
                    KeyCode::Esc => return,
                    KeyCode::Enter => {
                        if let Some(s) = list.get(sel).cloned() {
                            self.start_import(lines, summary, s);
                        }
                        return;
                    }
                    KeyCode::Down => sel = (sel + 1).min(list.len().saturating_sub(1)),
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Backspace => {
                        filter.pop();
                        sel = 0;
                    }
                    KeyCode::Char(c) => {
                        filter.push(c);
                        sel = 0;
                    }
                    _ => {}
                }
                self.popup = Some(Popup::Storage {
                    lines,
                    summary,
                    filter,
                    sel,
                });
            }
        }
    }

    fn move_sel(&mut self, delta: i64) {
        let (state, len) = match self.screen {
            Screen::Orders => (&mut self.o_state, self.o_rows.len()),
            Screen::Explorer => (&mut self.e_state, self.e_rows.len()),
            Screen::Ledger => {
                let n = self.ledger.lock().unwrap().txns.len();
                (&mut self.l_state, n)
            }
        };
        if len == 0 {
            return;
        }
        let cur = state.selected().unwrap_or(0) as i64;
        state.select(Some((cur + delta).clamp(0, len as i64 - 1) as usize));
    }

    fn toggle_group(&mut self) {
        if let Some(r) = self
            .e_state
            .selected()
            .and_then(|i| self.e_rows.get(i))
            .cloned()
        {
            if r.expandable && !self.e_expanded.remove(&r.id) {
                self.e_expanded.insert(r.id);
            }
            self.rebuild_explorer();
        }
    }

    fn expand(&mut self, open: bool) {
        match self.screen {
            Screen::Orders => {
                let Some(i) = self.o_state.selected() else {
                    return;
                };
                let Some(r) = self.o_rows.get(i).cloned() else {
                    return;
                };
                if r.expandable && r.expanded != open {
                    if open {
                        self.o_collapsed.remove(&r.id);
                    } else {
                        self.o_collapsed.insert(r.id);
                    }
                    self.rebuild_orders();
                } else if !open && r.depth > 0 {
                    // jump to parent
                    if let Some(p) = self.o_rows[..i].iter().rposition(|x| x.depth < r.depth) {
                        self.o_state.select(Some(p));
                    }
                } else if open && r.expandable {
                    self.move_sel(1);
                }
            }
            Screen::Explorer => {
                let Some(i) = self.e_state.selected() else {
                    return;
                };
                let Some(r) = self.e_rows.get(i).cloned() else {
                    return;
                };
                if r.expandable && r.expanded != open {
                    self.toggle_group();
                } else if !open
                    && r.depth > 0
                    && let Some(p) = self.e_rows[..i].iter().rposition(|x| x.depth < r.depth)
                {
                    self.e_state.select(Some(p));
                }
            }
            Screen::Ledger => {}
        }
    }
}

fn next_screen(s: Screen, n: usize) -> Screen {
    let all = [Screen::Orders, Screen::Explorer, Screen::Ledger];
    let i = all.iter().position(|x| *x == s).unwrap();
    all[(i + n) % 3]
}

fn restore_selection<K>(state: &mut TableState, rows: &[Row<K>], id: Option<String>) {
    if rows.is_empty() {
        state.select(None);
        return;
    }
    if let Some(id) = id
        && let Some(i) = rows.iter().position(|r| r.id == id)
    {
        state.select(Some(i));
        return;
    }
    let cur = state.selected().unwrap_or(0).min(rows.len() - 1);
    state.select(Some(cur));
}
