use std::sync::{Arc, Mutex};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui_image::picker::Picker;

use crate::app::{App, Popup, Screen};
use crate::config::Config;
use crate::import::{ImportLine, ImportRequest, run_import, run_rollback};
use crate::ledger::{Ledger, TxnStatus};
use crate::mock::{MockDigiKey, MockPartsBox, demo_orders};
use crate::partsbox::PartsBox;

fn tmp_ledger(name: &str) -> Arc<Mutex<Ledger>> {
    let p = std::env::temp_dir().join(format!("dk2pb-test-{name}-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&p);
    Arc::new(Mutex::new(Ledger::open(p).unwrap()))
}

#[test]
fn import_then_rollback_restores_partsbox() {
    let pb = PartsBox {
        t: Box::new(MockPartsBox::new()),
    };
    let dk = MockDigiKey;
    let ledger = tmp_ledger("rt");
    let cfg = Config::default();
    let parts_before = pb.parts().unwrap();
    let orders_before = pb.orders().unwrap().len();
    let index = parts_before
        .iter()
        .map(|p| (p.name.to_lowercase(), p.id.clone()))
        .collect();

    let o = &demo_orders()[0];
    let s = &o.sales_orders[0];
    let lines: Vec<ImportLine> = s
        .line_items
        .iter()
        .map(|it| ImportLine {
            sales_order_id: s.sales_order_id,
            order_number: o.order_number,
            po_number: None,
            order_date: o.date_entered.clone(),
            item: it.clone(),
            quantity: it.quantity_shipped,
        })
        .collect();
    let req = ImportRequest {
        summary: "test".into(),
        storage_id: "st0".into(),
        storage_name: "Inbox".into(),
        lines,
        part_index: index,
    };
    let txn = run_import(req, &cfg, &dk, &pb, &ledger, &|_, _, _| {}).unwrap();

    assert_eq!(txn.status, TxnStatus::Committed);
    assert_eq!(txn.lines.len(), 6);
    let parts = pb.parts().unwrap();
    assert_eq!(
        parts.len(),
        parts_before.len() + 5,
        "TPS61222DCKR existed, 5 new"
    );
    let tps = parts.iter().find(|p| p.name == "TPS61222DCKR").unwrap();
    assert_eq!(tps.on_hand(), 13);
    let sx = parts.iter().find(|p| p.name == "SX1509QBIULTRT").unwrap();
    assert_eq!(sx.on_hand(), 6);
    assert!(
        sx.notes
            .as_ref()
            .unwrap()
            .contains("DigiKey: SX1509QBIULTRTCT-ND")
    );
    // structured fields instead of note lines
    assert_eq!(sx.mpn.as_deref(), Some("SX1509QBIULTRT"));
    assert_eq!(sx.manufacturer.as_deref(), Some("Semtech Corporation"));
    assert!(
        sx.custom_fields["Vendor URL"]
            .as_str()
            .unwrap()
            .contains("SX1509QBIULTRTCT-ND")
    );
    assert_eq!(
        sx.custom_fields["Datasheet"],
        "mock://SX1509QBIULTRTCT-ND.pdf"
    );
    assert!(!sx.notes.as_ref().unwrap().contains("Manufacturer:"));
    assert_eq!(
        txn.ops
            .iter()
            .filter(|o| matches!(o, crate::ledger::Op::SetImage { .. }))
            .count(),
        5
    );
    let pbo = pb.orders().unwrap();
    assert_eq!(pbo.len(), orders_before + 1);
    assert_eq!(pbo.last().unwrap().number.as_deref(), Some("90123456"));

    let rb = run_rollback(&txn.id, &pb, &ledger, &|_, _, _| {}).unwrap();
    assert_eq!(rb.status, TxnStatus::RolledBack, "{:?}", rb.log);
    let after = pb.parts().unwrap();
    assert_eq!(after.len(), parts_before.len());
    assert_eq!(
        after
            .iter()
            .find(|p| p.name == "TPS61222DCKR")
            .unwrap()
            .on_hand(),
        3
    );
    assert_eq!(pb.orders().unwrap().len(), orders_before);

    // ledger survives a reload, last snapshot wins
    let path = ledger.lock().unwrap().path().clone();
    let reloaded = Ledger::open(path).unwrap();
    assert_eq!(reloaded.txns.len(), 1);
    assert_eq!(reloaded.txns[0].status, TxnStatus::RolledBack);
}

#[test]
fn rollback_refuses_to_remove_used_stock() {
    let pb = PartsBox {
        t: Box::new(MockPartsBox::new()),
    };
    let ledger = tmp_ledger("used");
    let o = &demo_orders()[0];
    let s = &o.sales_orders[0];
    let it = s.line_items[3].clone(); // SX1509, 6 pcs, new part
    let req = ImportRequest {
        summary: "one".into(),
        storage_id: "st0".into(),
        storage_name: "Inbox".into(),
        lines: vec![ImportLine {
            sales_order_id: s.sales_order_id,
            order_number: o.order_number,
            po_number: None,
            order_date: None,
            item: it,
            quantity: 6,
        }],
        part_index: Default::default(),
    };
    let txn = run_import(
        req,
        &Config::default(),
        &MockDigiKey,
        &pb,
        &ledger,
        &|_, _, _| {},
    )
    .unwrap();
    // use 4 of the 6 pieces, then try to roll back
    let pid = txn.lines[0].part_id.clone();
    pb.t.call("stock/remove", serde_json::json!({"stock/source": {"source/part-id": pid, "source/storage-id": "st0"}, "stock/quantity": 4})).unwrap();
    let rb = run_rollback(&txn.id, &pb, &ledger, &|_, _, _| {}).unwrap();
    assert_eq!(rb.status, TxnStatus::RollbackFailed);
    assert!(
        rb.log.iter().any(|l| l.contains("stock was used")),
        "{:?}",
        rb.log
    );
    let part = pb.part(&pid).unwrap();
    assert_eq!(part.on_hand(), 2, "remaining stock left alone");
}

#[test]
fn import_without_orders_feature_adds_stock_directly() {
    let pb = PartsBox {
        t: Box::new(MockPartsBox::without_orders()),
    };
    let ledger = tmp_ledger("noorders");
    let o = &demo_orders()[0];
    let s = &o.sales_orders[0];
    let it = s.line_items[3].clone(); // SX1509, new part
    let req = ImportRequest {
        summary: "no orders".into(),
        storage_id: "st0".into(),
        storage_name: "Inbox".into(),
        lines: vec![ImportLine {
            sales_order_id: s.sales_order_id,
            order_number: o.order_number,
            po_number: None,
            order_date: None,
            item: it,
            quantity: 6,
        }],
        part_index: Default::default(),
    };
    let txn = run_import(
        req,
        &Config::default(),
        &MockDigiKey,
        &pb,
        &ledger,
        &|_, _, _| {},
    )
    .unwrap();
    assert_eq!(txn.status, TxnStatus::Committed, "{:?}", txn.log);
    assert!(
        txn.ops
            .iter()
            .any(|o| matches!(o, crate::ledger::Op::AddStock { .. }))
    );
    let pid = txn.lines[0].part_id.clone();
    assert_eq!(pb.part(&pid).unwrap().on_hand(), 6);

    let rb = run_rollback(&txn.id, &pb, &ledger, &|_, _, _| {}).unwrap();
    assert_eq!(rb.status, TxnStatus::RolledBack, "{:?}", rb.log);
    assert!(
        pb.part(&pid).is_err(),
        "part created by the import is gone again"
    );
}

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| crate::ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn wait_loaded(app: &mut App) {
    for _ in 0..100 {
        app.drain();
        if app.pb().is_some() && !app.orders().is_empty() && app.job.is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("did not load");
}

#[test]
fn tui_flow_import_and_rollback() {
    let ledger = tmp_ledger("tui");
    let pb = Arc::new(PartsBox {
        t: Box::new(MockPartsBox::new()),
    });
    let mut app = App::new(
        Config::default(),
        true,
        Arc::new(MockDigiKey),
        pb,
        ledger,
        Picker::halfblocks(),
    );
    wait_loaded(&mut app);
    let s = render(&mut app, 150, 30);
    std::fs::write("/tmp/screen_orders.txt", &s).unwrap();
    assert!(s.contains("Order 81234567"));
    assert!(
        s.contains("Sales order 89000112"),
        "split order shows sales orders"
    );
    assert!(s.contains("● in PB"), "TPS61222DCKR is known");
    assert!(s.contains("✔ 2/2"), "August order already imported");

    // select first order row (index 0) and import it
    app.on_key(key(KeyCode::Char('i')));
    assert!(matches!(app.popup, Some(Popup::Storage { .. })));
    let s = render(&mut app, 150, 30);
    std::fs::write("/tmp/screen_storage.txt", &s).unwrap();
    app.on_key(key(KeyCode::Char('i'))); // filter "i" -> Inbox
    app.on_key(key(KeyCode::Enter));
    assert!(app.job.is_some());
    let s = render(&mut app, 150, 30);
    std::fs::write("/tmp/screen_progress.txt", &s).unwrap();
    wait_loaded(&mut app);
    std::thread::sleep(std::time::Duration::from_millis(600));
    app.drain();
    let s = render(&mut app, 150, 30);
    std::fs::write("/tmp/screen_after_import.txt", &s).unwrap();
    assert!(s.contains("✔ 6/6"), "{s}");

    // details pane with photo on an item
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char(' ')));
    for _ in 0..20 {
        app.drain();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let s = render(&mut app, 160, 40);
    std::fs::write("/tmp/screen_detail.txt", &s).unwrap();
    assert!(s.contains("Deliveries"));

    // explorer, grouped by storage; search
    app.on_key(key(KeyCode::Char(' ')));
    app.on_key(key(KeyCode::Char('2')));
    app.on_key(key(KeyCode::Char('e')));
    let s = render(&mut app, 150, 34);
    std::fs::write("/tmp/screen_explorer.txt", &s).unwrap();
    assert!(s.contains("Inbox"));
    app.on_key(key(KeyCode::Char('/')));
    for c in "sx15".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let s = render(&mut app, 150, 20);
    std::fs::write("/tmp/screen_search.txt", &s).unwrap();
    assert!(s.contains("SX1509QBIULTRT"));
    assert!(!s.contains("NE555DR"));
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Char('g')));
    let s = render(&mut app, 150, 20);
    std::fs::write("/tmp/screen_tags.txt", &s).unwrap();
    assert!(s.contains("grouped by tag"));

    // ledger + rollback
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Char(' ')));
    let s = render(&mut app, 160, 30);
    std::fs::write("/tmp/screen_ledger.txt", &s).unwrap();
    assert!(s.contains("committed"));
    app.on_key(key(KeyCode::Char('u')));
    assert!(matches!(app.popup, Some(Popup::ConfirmRollback { .. })));
    app.on_key(key(KeyCode::Char('y')));
    wait_loaded(&mut app);
    std::thread::sleep(std::time::Duration::from_millis(600));
    app.drain();
    let s = render(&mut app, 160, 30);
    std::fs::write("/tmp/screen_rolled_back.txt", &s).unwrap();
    assert!(s.contains("rolled back"), "{s}");
    app.screen = Screen::Orders;
    let s = render(&mut app, 150, 30);
    assert!(s.contains("· 0/6"), "{s}");
}

#[test]
fn secret_from_file_env_var() {
    let dir = std::env::temp_dir().join(format!("dk2pb-secret-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("pb.key");
    std::fs::write(&f, "from-file\n").unwrap();
    let cfg_path = dir.join("config.toml");
    std::fs::write(&cfg_path, "partsbox_api_key = \"from-config\"\n").unwrap();

    unsafe { std::env::remove_var("PARTSBOX_API_KEY") }; // a key in the real environment would win
    unsafe { std::env::set_var("PARTSBOX_API_KEY_FILE", &f) };
    let (cfg, _) = Config::load(Some(cfg_path.clone())).unwrap();
    assert_eq!(cfg.partsbox_api_key, "from-file");

    unsafe { std::env::set_var("PARTSBOX_API_KEY", "from-env") };
    let (cfg, _) = Config::load(Some(cfg_path)).unwrap();
    assert_eq!(
        cfg.partsbox_api_key, "from-env",
        "direct env var wins over _FILE"
    );

    unsafe {
        std::env::remove_var("PARTSBOX_API_KEY_FILE");
        std::env::remove_var("PARTSBOX_API_KEY");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
