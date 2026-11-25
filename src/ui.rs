use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, Gauge, Paragraph, Row as TRow, Table, Tabs, Wrap,
};
use ratatui_image::{Resize, StatefulImage};

use crate::app::{App, ERow, Loadable, ORow, Popup, Screen};
use crate::digikey::Product;
use crate::ledger::{Op, TxnStatus};

const SPIN: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

fn date(s: &Option<String>) -> String {
    s.as_deref()
        .map(|d| d.get(..10).unwrap_or(d).to_string())
        .unwrap_or_default()
}

fn money(v: Option<f64>, cur: &str) -> String {
    match v {
        Some(x) => format!("{x:.2} {cur}"),
        None => String::new(),
    }
}

fn tree_prefix(depth: u16, expandable: bool, expanded: bool) -> String {
    let indent = "  ".repeat(depth as usize);
    let arrow = if !expandable {
        "  "
    } else if expanded {
        "▾ "
    } else {
        "▸ "
    };
    format!("{indent}{arrow}")
}

pub fn draw(f: &mut Frame, app: &mut App) {
    app.tick += 1;
    let [top, main, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .areas(f.area());

    // ---- tabs + load status
    let titles = vec![" 1 Orders ", " 2 PartsBox ", " 3 Ledger "];
    let sel = match app.screen {
        Screen::Orders => 0,
        Screen::Explorer => 1,
        Screen::Ledger => 2,
    };
    let [tabs_area, state_area] =
        Layout::horizontal([Constraint::Min(40), Constraint::Length(48)]).areas(top);
    f.render_widget(
        Tabs::new(titles)
            .select(sel)
            .highlight_style(Style::new().fg(Color::Black).bg(ACCENT).bold())
            .divider(" "),
        tabs_area,
    );
    let spin = SPIN[(app.tick / 2) as usize % SPIN.len()];
    let st = |name: &str, l: bool, err: bool| -> Span {
        if l {
            Span::styled(format!("{spin} {name} "), Style::new().fg(Color::Yellow))
        } else if err {
            Span::styled(format!("✗ {name} "), Style::new().fg(Color::Red))
        } else {
            Span::styled(format!("● {name} "), Style::new().fg(Color::Green))
        }
    };
    let mut state = vec![];
    if app.demo {
        state.push(Span::styled(
            " DEMO ",
            Style::new().fg(Color::Black).bg(Color::Magenta),
        ));
        state.push(Span::raw(" "));
    }
    state.push(st(
        "DigiKey",
        matches!(app.orders, Loadable::Loading),
        matches!(app.orders, Loadable::Failed(_)),
    ));
    state.push(st(
        "PartsBox",
        matches!(app.pb_data, Loadable::Loading),
        matches!(app.pb_data, Loadable::Failed(_)),
    ));
    f.render_widget(
        Paragraph::new(Line::from(state)).right_aligned(),
        state_area,
    );

    // ---- main
    let (list_area, detail_area) = if app.detail {
        if main.width >= 110 {
            let [a, b] =
                Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)])
                    .areas(main);
            (a, Some(b))
        } else {
            let [a, b] = Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(main);
            (a, Some(b))
        }
    } else {
        (main, None)
    };
    match app.screen {
        Screen::Orders => draw_orders(f, app, list_area),
        Screen::Explorer => draw_explorer(f, app, list_area),
        Screen::Ledger => draw_ledger(f, app, list_area),
    }
    if let Some(d) = detail_area {
        match app.screen {
            Screen::Orders => draw_item_detail(f, app, d),
            Screen::Explorer => draw_part_detail(f, app, d),
            Screen::Ledger => draw_txn_detail(f, app, d),
        }
    }

    // ---- bottom: progress or hints
    draw_bottom(f, app, bottom);

    // ---- popups
    if app.popup.is_some() {
        draw_popup(f, app);
    }
}

fn block(title: impl Into<Line<'static>>) -> Block<'static> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(title)
}

fn search_title(q: &str, editing: bool) -> Span<'static> {
    if editing {
        Span::styled(format!(" /{q}█ "), Style::new().fg(Color::Yellow).bold())
    } else if !q.is_empty() {
        Span::styled(
            format!(" /{q} (Esc clears) "),
            Style::new().fg(Color::Yellow),
        )
    } else {
        Span::raw("")
    }
}

fn loading_or_error<T>(f: &mut Frame, area: Rect, l: &Loadable<T>, what: &str, tick: u64) -> bool {
    match l {
        Loadable::Loading => {
            let spin = SPIN[(tick / 2) as usize % SPIN.len()];
            f.render_widget(
                Paragraph::new(format!("{spin} Loading {what}…")).block(block(format!(" {what} "))),
                area,
            );
            true
        }
        Loadable::Failed(e) => {
            f.render_widget(
                Paragraph::new(Text::from(vec![
                    Line::from(format!("Could not load {what}:")).red().bold(),
                    Line::raw(""),
                    Line::raw(e.clone()),
                    Line::raw(""),
                    Line::from("Check your config / credentials and press r to retry.").fg(DIM),
                ]))
                .wrap(Wrap { trim: false })
                .block(block(format!(" {what} "))),
                area,
            );
            true
        }
        Loadable::Ready(_) => false,
    }
}

// ------------------------------------------------------------------ orders

fn draw_orders(f: &mut Frame, app: &mut App, area: Rect) {
    if loading_or_error(f, area, &app.orders, "DigiKey orders", app.tick) {
        return;
    }
    let pb_ready = app.pb().is_some();
    let mut rows = vec![];
    for r in &app.o_rows {
        let prefix = tree_prefix(r.depth, r.expandable, r.expanded);
        let row = match r.kind {
            ORow::Order(oi) => {
                let o = &app.orders()[oi];
                let items: Vec<_> = o
                    .sales_orders
                    .iter()
                    .flat_map(|s| s.line_items.iter())
                    .collect();
                let done = items
                    .iter()
                    .filter(|it| {
                        let s = app.item_status(it);
                        s.target > 0 && s.remaining() == 0
                    })
                    .count();
                let partial = items.iter().any(|it| app.item_status(it).imported > 0);
                let total: f64 = o
                    .sales_orders
                    .iter()
                    .filter_map(|s| s.total_price)
                    .sum::<f64>()
                    + if o.sales_orders.iter().all(|s| s.total_price.is_none()) {
                        items.iter().filter_map(|i| i.total_price).sum::<f64>()
                    } else {
                        0.0
                    };
                let status = o
                    .entire_order_status
                    .as_ref()
                    .and_then(|s| s.short_description.clone().or(s.order_status.clone()))
                    .unwrap_or_default();
                let split = if o.sales_orders.len() > 1 {
                    format!(" · {} sales orders", o.sales_orders.len())
                } else {
                    String::new()
                };
                let imp = imported_badge(done, items.len(), partial);
                TRow::new(vec![
                    Cell::from(Line::from(vec![
                        Span::raw(prefix),
                        Span::styled(date(&o.date_entered), Style::new().fg(ACCENT)),
                        Span::raw("  "),
                        Span::styled(format!("Order {}", o.order_number), Style::new().bold()),
                    ])),
                    Cell::from(format!(
                        "{status}{split} · {}",
                        money(Some(total), o.currency.as_deref().unwrap_or(""))
                    ))
                    .fg(DIM),
                    Cell::from(format!("{} items", items.len())),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(imp),
                ])
            }
            ORow::Sales(oi, si) => {
                let s = &app.orders()[oi].sales_orders[si];
                let done = s
                    .line_items
                    .iter()
                    .filter(|it| {
                        let st = app.item_status(it);
                        st.target > 0 && st.remaining() == 0
                    })
                    .count();
                let partial = s
                    .line_items
                    .iter()
                    .any(|it| app.item_status(it).imported > 0);
                let status = s
                    .status
                    .as_ref()
                    .and_then(|x| x.short_description.clone().or(x.order_status.clone()))
                    .unwrap_or_default();
                TRow::new(vec![
                    Cell::from(Line::from(vec![
                        Span::raw(prefix),
                        Span::styled(
                            format!("Sales order {}", s.sales_order_id),
                            Style::new().fg(Color::Blue).bold(),
                        ),
                    ])),
                    Cell::from(status).fg(DIM),
                    Cell::from(format!("{} items", s.line_items.len())),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(imported_badge(done, s.line_items.len(), partial)),
                ])
            }
            ORow::Item(oi, si, li) => {
                let it = &app.orders()[oi].sales_orders[si].line_items[li];
                let st = app.item_status(it);
                let qty = if it.quantity_shipped == it.quantity_ordered {
                    format!("{}", it.quantity_ordered)
                } else {
                    format!("{}/{} shp", it.quantity_shipped, it.quantity_ordered)
                };
                let qty_style = if it.quantity_shipped < it.quantity_ordered {
                    Style::new().fg(Color::Yellow)
                } else {
                    Style::new()
                };
                let (pb_cell, stock_cell) = if !pb_ready {
                    (Cell::from("?").fg(DIM), Cell::from(""))
                } else if st.part.is_some() {
                    (
                        Cell::from("● in PB").fg(Color::Green),
                        Cell::from(format!("{}", st.on_hand)),
                    )
                } else {
                    (Cell::from("○ new").fg(DIM), Cell::from("–").fg(DIM))
                };
                let imp = if st.target == 0 {
                    Span::styled("not shipped", Style::new().fg(DIM))
                } else if st.remaining() == 0 {
                    Span::styled(format!("✔ {}", st.imported), Style::new().fg(Color::Green))
                } else if st.imported > 0 {
                    Span::styled(
                        format!("◐ {}/{}", st.imported, st.target),
                        Style::new().fg(Color::Yellow),
                    )
                } else {
                    Span::styled("·", Style::new().fg(DIM))
                };
                TRow::new(vec![
                    Cell::from(Line::from(vec![
                        Span::raw(prefix),
                        Span::raw(it.manufacturer_product_number.clone()),
                    ])),
                    Cell::from(it.description.clone().unwrap_or_default()).fg(Color::Gray),
                    Cell::from(qty).style(qty_style),
                    pb_cell,
                    stock_cell,
                    Cell::from(Line::from(imp)),
                ])
            }
        };
        rows.push(row);
    }
    let n_orders = app.orders().len();
    let title = Line::from(vec![
        Span::styled(
            format!(
                " DigiKey orders ({n_orders}, last {} days) ",
                app.cfg.order_history_days
            ),
            Style::new().fg(ACCENT).bold(),
        ),
        search_title(&app.o_search.query, app.o_search.editing),
    ]);
    let table = Table::new(
        rows,
        [
            Constraint::Min(34),
            Constraint::Fill(1),
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(12),
        ],
    )
    .header(
        TRow::new(vec![
            "Order / part",
            "Description",
            "Qty",
            "PartsBox",
            "Stock",
            "Imported",
        ])
        .style(Style::new().fg(DIM).add_modifier(Modifier::UNDERLINED)),
    )
    .block(block(title))
    .row_highlight_style(
        Style::new()
            .bg(Color::Rgb(40, 50, 70))
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▌");
    f.render_stateful_widget(table, area, &mut app.o_state);
}

fn imported_badge(done: usize, total: usize, partial: bool) -> Line<'static> {
    if total > 0 && done == total {
        Line::from(Span::styled(
            format!("✔ {done}/{total}"),
            Style::new().fg(Color::Green),
        ))
    } else if done > 0 || partial {
        Line::from(Span::styled(
            format!("◐ {done}/{total}"),
            Style::new().fg(Color::Yellow),
        ))
    } else {
        Line::from(Span::styled(format!("· 0/{total}"), Style::new().fg(DIM)))
    }
}

fn kv(k: &str, v: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{k:<14}"), Style::new().fg(DIM)),
        Span::raw(v.into()),
    ])
}

fn photo_and_text(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    title: String,
    pn: Option<String>,
    lines: Vec<Line<'static>>,
) {
    let b = block(Line::from(Span::styled(
        title,
        Style::new().fg(ACCENT).bold(),
    )));
    let inner = b.inner(area);
    f.render_widget(b, area);
    let photo_h = if pn.is_some() {
        (inner.height / 3).clamp(6, 12)
    } else {
        0
    };
    let [photo_area, text_area] =
        Layout::vertical([Constraint::Length(photo_h), Constraint::Min(1)]).areas(inner);
    if let Some(pn) = pn {
        let pic_area = Rect {
            width: (photo_h * 2 + 2).min(photo_area.width),
            ..photo_area
        };
        match app.photos.get_mut(&pn) {
            Some(Loadable::Ready(proto)) => f.render_stateful_widget(
                StatefulImage::default().resize(Resize::Fit(None)),
                pic_area,
                proto,
            ),
            Some(Loadable::Loading) | None => {
                f.render_widget(Paragraph::new("loading photo…").fg(DIM), pic_area)
            }
            Some(Loadable::Failed(e)) => f.render_widget(
                Paragraph::new(format!("(no photo: {e})"))
                    .fg(DIM)
                    .wrap(Wrap { trim: true }),
                pic_area,
            ),
        }
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), text_area);
}

fn product_lines(p: Option<&Loadable<Option<Product>>>, out: &mut Vec<Line<'static>>) {
    match p {
        Some(Loadable::Ready(Some(p))) => {
            if !p.manufacturer.is_empty() {
                out.push(kv("Manufacturer", p.manufacturer.clone()));
            }
            if !p.detailed_description.is_empty() {
                out.push(kv("Detail", p.detailed_description.clone()));
            }
            if !p.category.is_empty() {
                out.push(kv("Category", p.category.clone()));
            }
            if let Some(pk) = &p.package {
                out.push(kv("Package", pk.clone()));
            }
            if let Some(ds) = &p.datasheet_url {
                out.push(kv("Datasheet", ds.clone()));
            }
            if !p.parameters.is_empty() {
                out.push(Line::raw(""));
                out.push(Line::from("Parameters").bold());
                for (k, v) in p.parameters.iter().take(14) {
                    out.push(Line::from(vec![
                        Span::styled(format!("  {k}: "), Style::new().fg(DIM)),
                        Span::raw(v.clone()),
                    ]));
                }
            }
        }
        Some(Loadable::Ready(None)) => {
            out.push(Line::from("DigiKey has no details for this part number").fg(DIM))
        }
        Some(Loadable::Failed(e)) => {
            out.push(Line::from(format!("DigiKey lookup failed: {e}")).red())
        }
        Some(Loadable::Loading) | None => out.push(Line::from("loading DigiKey details…").fg(DIM)),
    }
}

fn draw_item_detail(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(row) = app
        .o_state
        .selected()
        .and_then(|i| app.o_rows.get(i))
        .cloned()
    else {
        f.render_widget(block(" Details "), area);
        return;
    };
    let mut l: Vec<Line<'static>> = vec![];
    match row.kind {
        ORow::Order(oi) => {
            let o = app.orders()[oi].clone();
            l.push(kv("Order", o.order_number.to_string()));
            l.push(kv("Date", date(&o.date_entered)));
            if let Some(s) = &o.entire_order_status {
                l.push(kv(
                    "Status",
                    s.long_description
                        .clone()
                        .or(s.short_description.clone())
                        .unwrap_or_default(),
                ));
            }
            if let Some(po) = o.po_number.clone().filter(|p| !p.is_empty()) {
                l.push(kv("PO", po));
            }
            l.push(Line::raw(""));
            l.push(Line::from("Sales orders (each ships and is invoiced separately)").bold());
            for s in &o.sales_orders {
                l.push(Line::from(format!(
                    "  {}  {}  {} items  {}",
                    s.sales_order_id,
                    s.status
                        .as_ref()
                        .and_then(|x| x.short_description.clone())
                        .unwrap_or_default(),
                    s.line_items.len(),
                    s.ship_method.clone().unwrap_or_default()
                )));
            }
            l.push(Line::raw(""));
            l.push(Line::from("Enter: open in browser · i: import the whole order").fg(DIM));
            photo_and_text(f, app, area, format!(" Order {} ", o.order_number), None, l);
        }
        ORow::Sales(oi, si) => {
            let s = app.orders()[oi].sales_orders[si].clone();
            l.push(kv("Sales order", s.sales_order_id.to_string()));
            l.push(kv(
                "Status",
                s.status
                    .as_ref()
                    .and_then(|x| x.long_description.clone().or(x.short_description.clone()))
                    .unwrap_or_default(),
            ));
            l.push(kv("Ship method", s.ship_method.clone().unwrap_or_default()));
            l.push(kv(
                "Total",
                money(s.total_price, s.currency.as_deref().unwrap_or("")),
            ));
            l.push(Line::raw(""));
            l.push(Line::from("i: import this sales order").fg(DIM));
            photo_and_text(
                f,
                app,
                area,
                format!(" Sales order {} ", s.sales_order_id),
                None,
                l,
            );
        }
        ORow::Item(oi, si, li) => {
            let cur = app.orders()[oi].currency.clone().unwrap_or_default();
            let it = app.orders()[oi].sales_orders[si].line_items[li].clone();
            let st = app.item_status(&it);
            l.push(kv("MPN", it.manufacturer_product_number.clone()));
            l.push(kv("DigiKey", it.digi_key_product_number.clone()));
            l.push(kv(
                "Description",
                it.description.clone().unwrap_or_default(),
            ));
            product_lines(app.products.get(&it.digi_key_product_number), &mut l);
            l.push(Line::raw(""));
            l.push(Line::from("This order line").bold());
            l.push(kv(
                "Quantity",
                format!(
                    "{} ordered · {} shipped · {} backorder · {} reserved",
                    it.quantity_ordered,
                    it.quantity_shipped,
                    it.quantity_back_order,
                    it.quantity_reserved
                ),
            ));
            l.push(kv(
                "Price",
                format!(
                    "{} each · {} total",
                    money(it.unit_price, &cur),
                    money(it.total_price, &cur)
                ),
            ));
            if let Some(p) = it.pack_type.clone() {
                l.push(kv("Packaging", p));
            }
            if let Some(c) = it.customer_reference.clone().filter(|c| !c.is_empty()) {
                l.push(kv("Customer ref", c));
            }
            if let Some(c) = it.country_of_origin.clone() {
                l.push(kv("Origin", c));
            }
            if !it.item_shipments.is_empty() {
                l.push(Line::from("Deliveries").bold());
                for s in &it.item_shipments {
                    l.push(Line::from(format!(
                        "  {}  {} pcs  invoice {}  {}",
                        date(&s.shipped_date),
                        s.quantity_shipped,
                        s.invoice_id.map(|x| x.to_string()).unwrap_or("-".into()),
                        s.tracking_number.clone().unwrap_or_default()
                    )));
                }
            }
            for s in &it.schedules {
                l.push(
                    Line::from(format!(
                        "  scheduled {}  {} pcs",
                        date(&s.scheduled_date),
                        s.quantity_scheduled
                    ))
                    .fg(Color::Yellow),
                );
            }
            l.push(Line::raw(""));
            l.push(Line::from("PartsBox").bold());
            match (st.part, app.pb()) {
                (Some(i), Some(d)) => {
                    let p = &d.parts[i];
                    l.push(kv("Part", p.name.clone()));
                    l.push(kv("On hand", st.on_hand.to_string()));
                    for (sid, q) in p.on_hand_by_storage() {
                        l.push(Line::from(format!(
                            "  {:<20} {q}",
                            app.storage_names.get(&sid).cloned().unwrap_or(sid)
                        )));
                    }
                }
                (None, Some(_)) => l.push(
                    Line::from(
                        "Not in PartsBox – import creates a new part with the DigiKey photo",
                    )
                    .fg(DIM),
                ),
                _ => l.push(Line::from("PartsBox not loaded").fg(DIM)),
            }
            l.push(kv(
                "Imported",
                format!(
                    "{} of {} {}",
                    st.imported,
                    st.target,
                    if app.cfg.import_quantity == "ordered" {
                        "ordered"
                    } else {
                        "shipped"
                    }
                ),
            ));
            l.push(Line::raw(""));
            l.push(Line::from("Enter: open · d: DigiKey · p: PartsBox · i: import").fg(DIM));
            let pn = it.digi_key_product_number.clone();
            photo_and_text(
                f,
                app,
                area,
                format!(" {} ", it.manufacturer_product_number),
                Some(pn),
                l,
            );
        }
    }
}

// ------------------------------------------------------------------ explorer

fn draw_explorer(f: &mut Frame, app: &mut App, area: Rect) {
    if loading_or_error(f, area, &app.pb_data, "PartsBox", app.tick) {
        return;
    }
    let d = app.pb().unwrap();
    let mut rows = vec![];
    for r in &app.e_rows {
        let prefix = tree_prefix(r.depth, r.expandable, r.expanded);
        rows.push(match &r.kind {
            ERow::Group { label, parts, qty } => TRow::new(vec![
                Cell::from(Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(label.clone(), Style::new().fg(Color::Blue).bold()),
                ])),
                Cell::from(if *parts == 1 {
                    "1 part".to_string()
                } else {
                    format!("{parts} parts")
                })
                .fg(DIM),
                Cell::from(""),
                Cell::from(qty.to_string()).fg(DIM),
                Cell::from(""),
            ]),
            ERow::Part { idx, qty } => {
                let p = &d.parts[*idx];
                TRow::new(vec![
                    Cell::from(Line::from(vec![
                        Span::raw(prefix),
                        Span::raw(p.name.clone()),
                    ])),
                    Cell::from(p.description.clone().unwrap_or_default()).fg(Color::Gray),
                    Cell::from(p.footprint.clone().unwrap_or_default()),
                    Cell::from(qty.to_string()).style(if *qty <= 0 {
                        Style::new().fg(Color::Red)
                    } else {
                        Style::new()
                    }),
                    Cell::from(p.tags.join(" ")).fg(Color::Magenta),
                ])
            }
        });
    }
    let title = Line::from(vec![
        Span::styled(
            format!(
                " PartsBox · {} parts · {} storages · grouped by {} ",
                d.parts.len(),
                d.storages.len(),
                app.group.label()
            ),
            Style::new().fg(ACCENT).bold(),
        ),
        search_title(&app.e_search.query, app.e_search.editing),
    ]);
    let table = Table::new(
        rows,
        [
            Constraint::Min(30),
            Constraint::Fill(1),
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Length(18),
        ],
    )
    .header(
        TRow::new(vec!["Part", "Description", "Footprint", "Qty", "Tags"])
            .style(Style::new().fg(DIM).add_modifier(Modifier::UNDERLINED)),
    )
    .block(block(title))
    .row_highlight_style(
        Style::new()
            .bg(Color::Rgb(40, 50, 70))
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▌");
    f.render_stateful_widget(table, area, &mut app.e_state);
}

fn draw_part_detail(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(p) = app.selected_part().cloned() else {
        let hint = match app.e_state.selected().and_then(|i| app.e_rows.get(i)) {
            Some(r) if matches!(r.kind, ERow::Group { .. }) => {
                "Group – Enter/l expands, e expands all"
            }
            _ => "",
        };
        f.render_widget(Paragraph::new(hint).fg(DIM).block(block(" Details ")), area);
        return;
    };
    let mut l = vec![];
    l.push(kv("Name", p.name.clone()));
    l.push(kv("Description", p.description.clone().unwrap_or_default()));
    if let Some(m) = p.manufacturer_guess() {
        l.push(kv("Manufacturer", m));
    }
    if let Some(m) = p.mpn.clone().filter(|m| !m.is_empty()) {
        l.push(kv("MPN", m));
    }
    l.push(kv("Footprint", p.footprint.clone().unwrap_or_default()));
    l.push(kv("Type", p.kind.clone().unwrap_or_default()));
    l.push(kv("Tags", p.tags.join(", ")));
    for (k, v) in p.custom_fields.as_object().into_iter().flatten() {
        l.push(kv(k, v.as_str().unwrap_or_default().to_string()));
    }
    l.push(kv("On hand", p.on_hand().to_string()));
    for (sid, q) in p.on_hand_by_storage() {
        l.push(Line::from(format!(
            "  {:<20} {q}",
            app.storage_names.get(&sid).cloned().unwrap_or(sid)
        )));
    }
    if let Some(n) = p.notes.clone().filter(|n| !n.is_empty()) {
        l.push(Line::raw(""));
        l.push(Line::from("Notes").bold());
        for line in n.lines() {
            l.push(Line::raw(format!("  {line}")));
        }
    }
    let recent: Vec<_> = p.stock.iter().rev().take(8).cloned().collect();
    if !recent.is_empty() {
        l.push(Line::raw(""));
        l.push(Line::from("Stock history (latest)").bold());
        for s in recent {
            let when = s
                .timestamp
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map(|t| t.format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            let place = s
                .storage_id
                .as_ref()
                .and_then(|id| app.storage_names.get(id))
                .cloned()
                .unwrap_or_default();
            l.push(Line::from(format!(
                "  {when}  {:>+6}  {place}  {}{}",
                s.quantity,
                s.status
                    .clone()
                    .map(|x| format!("[{x}] "))
                    .unwrap_or_default(),
                s.comments.clone().unwrap_or_default()
            )));
        }
    }
    let pn = app.part_dk_pn(&p);
    if let Some(pn) = &pn {
        l.push(Line::raw(""));
        l.push(Line::from(format!("DigiKey {pn}")).bold());
        product_lines(app.products.get(pn), &mut l);
    }
    l.push(Line::raw(""));
    l.push(Line::from("Enter: open in PartsBox · d: DigiKey").fg(DIM));
    photo_and_text(f, app, area, format!(" {} ", p.name), pn, l);
}

// ------------------------------------------------------------------ ledger

fn status_span(s: &TxnStatus) -> Span<'static> {
    match s {
        TxnStatus::Running => Span::styled("running", Style::new().fg(Color::Yellow)),
        TxnStatus::Committed => Span::styled("committed", Style::new().fg(Color::Green)),
        TxnStatus::Failed => Span::styled("failed", Style::new().fg(Color::Red)),
        TxnStatus::RolledBack => Span::styled("rolled back", Style::new().fg(DIM)),
        TxnStatus::RollbackFailed => Span::styled("undo partial", Style::new().fg(Color::Red)),
    }
}

fn draw_ledger(f: &mut Frame, app: &mut App, area: Rect) {
    let l = app.ledger.lock().unwrap();
    let rows: Vec<TRow> = l
        .txns
        .iter()
        .map(|t| {
            let pcs: i64 = t.lines.iter().map(|x| x.quantity).sum();
            TRow::new(vec![
                Cell::from(t.created.get(..19).unwrap_or(&t.created).replace('T', " ")),
                Cell::from(Line::from(status_span(&t.status))),
                Cell::from(t.summary.clone()),
                Cell::from(t.storage.clone()),
                Cell::from(format!("{} lines / {pcs} pcs", t.lines.len())),
                Cell::from(format!("{} ops", t.ops.len())).fg(DIM),
            ])
        })
        .collect();
    let title = format!(" Transaction ledger · {} ", l.path().display());
    let empty = rows.is_empty();
    drop(l);
    if empty {
        f.render_widget(
            Paragraph::new("No imports yet. Import something from the Orders screen (i).")
                .fg(DIM)
                .block(block(title)),
            area,
        );
        return;
    }
    let table = Table::new(
        rows,
        [
            Constraint::Length(19),
            Constraint::Length(12),
            Constraint::Min(20),
            Constraint::Length(14),
            Constraint::Length(18),
            Constraint::Length(8),
        ],
    )
    .header(
        TRow::new(vec![
            "When", "Status", "Summary", "Storage", "Imported", "Changes",
        ])
        .style(Style::new().fg(DIM).add_modifier(Modifier::UNDERLINED)),
    )
    .block(block(Line::from(Span::styled(
        title,
        Style::new().fg(ACCENT).bold(),
    ))))
    .row_highlight_style(
        Style::new()
            .bg(Color::Rgb(40, 50, 70))
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▌");
    f.render_stateful_widget(table, area, &mut app.l_state);
}

fn draw_txn_detail(f: &mut Frame, app: &mut App, area: Rect) {
    let l = app.ledger.lock().unwrap();
    let Some(t) = app.l_state.selected().and_then(|i| l.txns.get(i)).cloned() else {
        drop(l);
        f.render_widget(block(" Details "), area);
        return;
    };
    drop(l);
    let mut lines = vec![
        kv("Transaction", t.id.clone()),
        Line::from(vec![
            Span::styled(format!("{:<14}", "Status"), Style::new().fg(DIM)),
            status_span(&t.status),
        ]),
        kv("Storage", t.storage.clone()),
    ];
    if let Some(r) = &t.rolled_back {
        lines.push(kv(
            "Rolled back",
            r.get(..19).unwrap_or(r).replace('T', " "),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from("Imported lines").bold());
    for x in &t.lines {
        lines.push(Line::raw(format!(
            "  {:>5} × {:<24} SO {}",
            x.quantity, x.mpn, x.sales_order_id
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from("Changes made to PartsBox (undone bottom-up)").bold());
    let name_of = |id: &str| -> String {
        t.lines
            .iter()
            .find(|x| x.part_id == id)
            .map(|x| x.mpn.clone())
            .unwrap_or_else(|| id.to_string())
    };
    for op in &t.ops {
        lines.push(Line::raw(match op {
            Op::CreatePart { name, .. } => format!("  + new part {name}"),
            Op::SetImage { part_id } => format!("  + photo on {}", name_of(part_id)),
            Op::CreateOrder { number, .. } => {
                format!("  + PartsBox order for sales order {number}")
            }
            Op::ReceiveStock {
                quantity, part_id, ..
            } => format!("  + received {quantity} × {}", name_of(part_id)),
            Op::AddStock {
                quantity, part_id, ..
            } => format!("  + added {quantity} × {}", name_of(part_id)),
        }));
    }
    if !t.log.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from("Log").bold());
        for m in t.log.iter().rev().take(30) {
            let style = if m.contains("FAILED") || m.contains("ERROR") {
                Style::new().fg(Color::Red)
            } else {
                Style::new().fg(DIM)
            };
            lines.push(Line::styled(format!("  {m}"), style));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from("u: roll back this transaction").fg(DIM));
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block(" Transaction ")),
        area,
    );
}

// ------------------------------------------------------------------ bottom + popups

fn draw_bottom(f: &mut Frame, app: &App, area: Rect) {
    if let Some(j) = &app.job {
        let ratio = if j.total == 0 {
            0.0
        } else {
            (j.done as f64 / j.total as f64).clamp(0.0, 1.0)
        };
        let [a, b] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
        f.render_widget(
            Gauge::default()
                .gauge_style(Style::new().fg(ACCENT).bg(Color::Rgb(30, 30, 40)))
                .ratio(ratio)
                .label(format!("{} – {}/{}", j.title, j.done, j.total)),
            a,
        );
        let spin = SPIN[(app.tick / 2) as usize % SPIN.len()];
        f.render_widget(
            Paragraph::new(format!("{spin} {}", j.msg)).fg(Color::Yellow),
            b,
        );
        return;
    }
    let keys = match app.screen {
        Screen::Orders => {
            "↑↓ move  h l fold  ←→ screens  space details  enter open  i import  / search  r reload  ? help  q quit"
        }
        Screen::Explorer => {
            "↑↓ move  h l fold  ←→ screens  space details  enter open  g group  f flat  e expand all  / search  ? help"
        }
        Screen::Ledger => "↑↓ move  ←→ screens  space details  u roll back  ? help  q quit",
    };
    let [a, b] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    f.render_widget(Paragraph::new(app.status.clone()).fg(Color::Gray), a);
    f.render_widget(Paragraph::new(keys).fg(DIM), b);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let [v] = Layout::vertical([Constraint::Length(h.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    let [r] = Layout::horizontal([Constraint::Length(w.min(area.width))])
        .flex(Flex::Center)
        .areas(v);
    r
}

fn draw_popup(f: &mut Frame, app: &App) {
    let area = f.area();
    match app.popup.as_ref().unwrap() {
        Popup::Help => {
            let text = vec![
                Line::from("Screens").bold(),
                Line::raw("  1 / Tab / ← →  DigiKey orders → sales orders → items"),
                Line::raw("  2              PartsBox explorer"),
                Line::raw("  3              Transaction ledger"),
                Line::raw(""),
                Line::from("Everywhere").bold(),
                Line::raw("  ↑↓ j k PgUp/Dn  move          h l       fold / unfold, go to parent"),
                Line::raw("  space           toggle detail pane (with product photo)"),
                Line::raw("  /               fuzzy search, Enter keeps filter, Esc clears"),
                Line::raw("  r / F5          reload DigiKey and PartsBox"),
                Line::raw(""),
                Line::from("Orders").bold(),
                Line::raw(
                    "  enter           order: open in browser · item: choose DigiKey / PartsBox",
                ),
                Line::raw("  d / p           open item on DigiKey / in PartsBox"),
                Line::raw("  i               import selected item, sales order or whole order"),
                Line::raw("                  (only quantities not imported yet)"),
                Line::raw(""),
                Line::from("PartsBox explorer").bold(),
                Line::raw("  g  cycle grouping: storage → tag → footprint → manufacturer → flat"),
                Line::raw("  f  flat list on/off     e  expand/collapse all     enter  open part"),
                Line::raw(""),
                Line::from("Ledger").bold(),
                Line::raw("  u  roll back the selected import (unreceive stock, delete the"),
                Line::raw("     PartsBox order, remove photos and parts it created)"),
                Line::raw(""),
                Line::from("Columns: ● part exists in PartsBox · ✔ imported · ◐ partly imported")
                    .fg(DIM),
            ];
            let r = centered(area, 80, text.len() as u16 + 2);
            f.render_widget(Clear, r);
            f.render_widget(
                Paragraph::new(text).block(block(" Help · any key closes ")),
                r,
            );
        }
        Popup::Message(m) => {
            let r = centered(area, 80, 14);
            f.render_widget(Clear, r);
            f.render_widget(
                Paragraph::new(m.clone()).wrap(Wrap { trim: false }).block(
                    block(" Notice · any key closes ").border_style(Style::new().fg(Color::Red)),
                ),
                r,
            );
        }
        Popup::Open { title, choices } => {
            let mut text: Vec<Line> = choices
                .iter()
                .map(|(k, label, _)| {
                    Line::from(vec![
                        Span::styled(format!(" {k} "), Style::new().fg(Color::Black).bg(ACCENT)),
                        Span::raw(format!("  {label}")),
                    ])
                })
                .collect();
            text.push(Line::raw(""));
            text.push(Line::from("Enter = first · Esc = cancel").fg(DIM));
            let r = centered(area, 64, text.len() as u16 + 2);
            f.render_widget(Clear, r);
            f.render_widget(Paragraph::new(text).block(block(format!(" {title} "))), r);
        }
        Popup::ConfirmRollback {
            txn_id,
            summary,
            warning,
        } => {
            let mut text = vec![
                Line::from(format!("Roll back transaction {txn_id}?")).bold(),
                Line::raw(summary.clone()),
                Line::raw(""),
                Line::raw("This unreceives the stock, deletes the PartsBox order(s) it created,"),
                Line::raw("and deletes parts it created if they have no other stock."),
                Line::raw("Stock that was used or moved since then is left alone and reported."),
            ];
            if !warning.is_empty() {
                text.push(Line::raw(""));
                text.push(Line::from(warning.clone()).fg(Color::Yellow));
            }
            text.push(Line::raw(""));
            text.push(Line::from("y = roll back · n / Esc = cancel").bold());
            let r = centered(area, 76, text.len() as u16 + 4);
            f.render_widget(Clear, r);
            f.render_widget(
                Paragraph::new(text).wrap(Wrap { trim: false }).block(
                    block(" Confirm rollback ").border_style(Style::new().fg(Color::Yellow)),
                ),
                r,
            );
        }
        Popup::Storage {
            lines,
            summary,
            filter,
            sel,
        } => {
            let pcs: i64 = lines.iter().map(|l| l.quantity).sum();
            let list = app.storage_list(filter);
            let h = (list.len() as u16 + 9)
                .min(area.height.saturating_sub(4))
                .max(10);
            let r = centered(area, 70, h);
            f.render_widget(Clear, r);
            let b = block(" Import into which storage? ").border_style(Style::new().fg(ACCENT));
            let inner = b.inner(r);
            f.render_widget(b, r);
            let [head, lst, foot] = Layout::vertical([
                Constraint::Length(4),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .areas(inner);
            let new_parts = lines
                .iter()
                .filter(|l| app.item_status(&l.item).part.is_none())
                .count();
            f.render_widget(
                Paragraph::new(vec![
                    Line::from(summary.clone()).bold(),
                    Line::raw(format!(
                        "{} lines · {pcs} pcs · {new_parts} new parts (with photos)",
                        lines.len()
                    )),
                    Line::from(vec![
                        Span::styled("filter: ", Style::new().fg(DIM)),
                        Span::styled(format!("{filter}█"), Style::new().fg(Color::Yellow)),
                    ]),
                ]),
                head,
            );
            let visible = lst.height as usize;
            let start = sel.saturating_sub(visible.saturating_sub(1));
            let items: Vec<Line> = list
                .iter()
                .enumerate()
                .skip(start)
                .take(visible)
                .map(|(i, s)| {
                    if i == *sel {
                        Line::styled(
                            format!("▌ {}", s.name),
                            Style::new().bg(Color::Rgb(40, 50, 70)).bold(),
                        )
                    } else {
                        Line::raw(format!("  {}", s.name))
                    }
                })
                .collect();
            f.render_widget(Paragraph::new(items), lst);
            f.render_widget(
                Paragraph::new("type to filter · ↑↓ choose · Enter import · Esc cancel").fg(DIM),
                foot,
            );
        }
    }
}
