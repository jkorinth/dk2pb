# dk2pb — DigiKey → PartsBox terminal bridge

A ratatui TUI that shows your DigiKey orders and your PartsBox database side by side,
imports items or whole orders with product photos, and keeps a ledger so every import
can be rolled back.

```
cargo run --release -- --demo      # try it with fake data, no credentials needed
cargo install --path .             # then: dk2pb
```

## Screens

**1 Orders**: a tree of orders → sales orders → line items.
DigiKey splits one checkout into several *sales orders* (backorders, marketplace items,
scheduled releases), and each line can arrive in several *deliveries* (each has its own
invoice and tracking number). The sales-order level only appears when an order was split.
Columns show whether the part already exists in PartsBox, its current stock, and how much
of *this* line from *this* sales order has been imported (`✔` all, `◐` partly, `·` none).

**2 PartsBox**: explorer, grouped by storage location by default. Press `g` to group by
tag, footprint or manufacturer, or `f` for a flat list.

**3 Ledger**: every import with the changes it made. Press `u` to roll one back.

| Key | Action |
|---|---|
| `↑↓` `j k` `PgUp/PgDn` | move |
| `h l` | fold / unfold (`h` on a child jumps to its parent) |
| `← →` `tab` | previous / next screen |
| `space` | toggle the detail pane (DigiKey details, deliveries, stock per storage, product photo) |
| `enter` | order → opens it on DigiKey · item → choose DigiKey / PartsBox / sales order · explorer part → PartsBox |
| `d` / `p` | open item on DigiKey / in PartsBox |
| `i` | import the selected item, sales order or order (asks for the storage location) |
| `/` | fuzzy search (Enter keeps the filter, Esc clears it) |
| `g` `f` `e` | explorer: cycle grouping, flat list, expand/collapse all |
| `u` | ledger: roll back the selected transaction |
| `r` / `F5` | reload · `?` help · `q` quit |

## What an import does

For each line: it fetches DigiKey product details (cached in `~/.cache/dk2pb`), finds the
PartsBox part by MPN or creates a local part (name = MPN, description, manufacturer, MPN,
footprint from the package, custom fields `Vendor URL` and `Datasheet`, and notes with the
DigiKey PN and category), then uploads the DigiKey photo. Per sales order it creates a PartsBox **order** (vendor DigiKey,
number = sales order id, invoice numbers, unit prices), marks it ordered, and **receives**
it into the chosen storage location. This gives you proper purchase history in PartsBox.

Only quantities that are not imported yet are imported. By default that means quantities
DigiKey reports as *shipped*, so a backordered line can be imported again when the rest
arrives. Set `import_quantity = "ordered"` to change that.

"Already imported" comes from your PartsBox orders (vendor DigiKey, number = sales order
id, entry SKU = DigiKey PN), so orders you entered by hand that way are recognised too.

## Rollback

Each change is written to `~/.local/share/dk2pb/ledger.jsonl` as soon as it happens, so
an import that failed halfway can also be rolled back. Rollback goes newest-first:
unreceive stock → cancel and delete the PartsBox order → remove photos → delete created
parts. Two safety checks apply:

- If some of the received stock has been used since, that stock is left alone.
- A created part that has other stock history is kept.

Anything it skips is reported, and the transaction is marked "undo partial".

## Setup

1. `dk2pb --init` writes `~/.config/dk2pb/config.toml`.
2. PartsBox: Settings → API → create a key → `partsbox_api_key`.
3. DigiKey: on developer.digikey.com create a **production** app and enable
   *Product Information v4* and *Order Status v4*. Put its Client ID and Secret in the
   config. Leave `digikey_account_id` empty unless your login has several DigiKey
   account numbers — then put the one whose orders you want (a number, not an email).
4. Set `digikey_site`, `digikey_currency` and `digikey_web_host` (e.g. `DE`, `EUR`,
   `www.digikey.de`).

Secrets can also come from `PARTSBOX_API_KEY`, `DIGIKEY_CLIENT_ID`,
`DIGIKEY_CLIENT_SECRET` and `DIGIKEY_ACCOUNT_ID`, or from a file by pointing the
`_FILE` variant at it (first line is used, trailing newline is fine):

```sh
PARTSBOX_API_KEY_FILE=/run/secrets/partsbox dk2pb
pass show partsbox > ~/.secrets/pb && PARTSBOX_API_KEY_FILE=~/.secrets/pb dk2pb
```

Precedence is config file < `*_FILE` < environment variable.

Photos are shown using kitty, sixel or iTerm2 graphics when the terminal supports them, and
with unicode half blocks otherwise (`--halfblocks` forces that).

## Things to verify on first real use

These were written against the published API docs and tested only against the built-in
mock, so check them before relying on the tool:

- **Browser URLs**: the DigiKey order and PartsBox part URL patterns are guesses. Adjust
  `url_digikey_order`, `url_partsbox_part` and the other `url_*` templates in the config
  once you see the real URLs in your browser.
- **Currency code** for PartsBox order entries (`partsbox_currency = "eur"`): if PartsBox
  rejects it, try `"EUR"`.
- **PartsBox Orders are a plan feature**: if `order/create` answers `401 forbidden
  Feature not present in your plan`, dk2pb adds the stock directly (`stock/add` into the
  chosen storage, price and DigiKey part number kept in the stock entry) and rolls it back
  with `stock/remove`. No PartsBox order is created, so the "already imported" marks come
  from the local ledger only.
- **`order/delete`**: its parameters aren't in the docs I could read. If rollback reports
  it failed, delete that PartsBox order by hand; everything else still rolls back.
- Start with one item, check it in PartsBox, roll it back from the ledger, and then do
  the real thing.

## Code map

| File | Contents |
|---|---|
| `digikey.rs` | Order Status v4 + Product Information v4 client, OAuth, caching |
| `partsbox.rs` | PartsBox RPC client, models, stock helpers |
| `import.rs` | import and rollback logic |
| `ledger.rs` | append-only JSONL transaction log |
| `app.rs` | state, trees, fuzzy search, keys, background jobs |
| `ui.rs` | rendering |
| `mock.rs` | demo backends (also used by tests) |

No Rust crates for the DigiKey or PartsBox APIs existed, so both clients are written here.
The UI uses `ratatui`, `ratatui-image` for photos, and `nucleo-matcher` (Helix's fuzzy
matcher). `tui-tree-widget` was considered but not used, because the trees need table
columns.

## Development

Every PR is gated on `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`
and `cargo test`, and its title must be a conventional commit.

Versions are never bumped by hand: [release-please](https://github.com/googleapis/release-please)
reads the conventional commits on `main`, keeps a release PR open with the next semver
version and the generated `CHANGELOG.md`, and tags `v<version>` once that PR is merged
(`fix:` → patch, `feat:` → minor, `!` or `BREAKING CHANGE:` → major).

## License

GPL-3.0-or-later, see [LICENSE](LICENSE). All dependencies are GPLv3-compatible
(MIT/Apache-2.0/BSD/ISC/Zlib/MPL-2.0/Unicode-3.0); note that Apache-2.0 crates in the
tree rule out GPLv2-only.
