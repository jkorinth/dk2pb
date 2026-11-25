//! dk2pb – a terminal bridge from DigiKey orders to PartsBox.

mod app;
mod config;
mod digikey;
mod import;
mod ledger;
mod mock;
mod partsbox;
mod ui;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui_image::picker::Picker;

use crate::app::App;
use crate::config::{Config, data_dir};
use crate::digikey::{DigiKey, DigiKeyApi};
use crate::ledger::Ledger;
use crate::partsbox::{HttpTransport, PartsBox};

#[derive(Parser)]
#[command(
    version,
    about = "Import DigiKey orders into PartsBox – interactive TUI"
)]
struct Cli {
    /// Try the UI with fake orders and an in-memory PartsBox (no credentials needed).
    #[arg(long)]
    demo: bool,
    /// Config file (default: ~/.config/dk2pb/config.toml)
    #[arg(long)]
    config: Option<PathBuf>,
    /// Write a config template and exit.
    #[arg(long)]
    init: bool,
    /// Ledger file (default: ~/.local/share/dk2pb/ledger.jsonl)
    #[arg(long)]
    ledger: Option<PathBuf>,
    /// Don't try sixel/kitty/iTerm graphics, draw photos with unicode half blocks.
    #[arg(long)]
    halfblocks: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (cfg, cfg_path) = Config::load(cli.config.clone())?;

    if cli.init {
        if cfg_path.exists() {
            println!("{} already exists", cfg_path.display());
        } else {
            Config::write_template(&cfg_path)?;
            println!("Wrote {}", cfg_path.display());
        }
        return Ok(());
    }

    let (dk, pb, ledger_path): (Arc<dyn DigiKeyApi>, PartsBox, PathBuf) = if cli.demo {
        (
            Arc::new(mock::MockDigiKey),
            PartsBox {
                t: Box::new(mock::MockPartsBox::new()),
            },
            cli.ledger
                .clone()
                .unwrap_or_else(|| std::env::temp_dir().join("dk2pb-demo-ledger.jsonl")),
        )
    } else {
        let missing = cfg.missing();
        if !missing.is_empty() {
            eprintln!(
                "Missing settings in {} (or environment):",
                cfg_path.display()
            );
            for m in missing {
                eprintln!("  - {m}");
            }
            eprintln!(
                "\nRun `dk2pb --init` to create a config template, or `dk2pb --demo` to try the UI."
            );
            std::process::exit(2);
        }
        (
            Arc::new(DigiKey::new(cfg.clone())),
            PartsBox {
                t: Box::new(HttpTransport::new(cfg.partsbox_api_key.clone())),
            },
            cli.ledger
                .clone()
                .unwrap_or_else(|| data_dir().join("ledger.jsonl")),
        )
    };
    if cli.demo && cli.ledger.is_none() {
        let _ = std::fs::remove_file(&ledger_path); // fresh demo every time
    }
    let ledger = Arc::new(Mutex::new(Ledger::open(ledger_path)?));

    let mut terminal = ratatui::init();
    let picker = if cli.halfblocks {
        Picker::halfblocks()
    } else {
        Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks())
    };
    let mut app = App::new(cfg, cli.demo, dk, Arc::new(pb), ledger, picker);

    let result = (|| -> Result<()> {
        while !app.quit {
            app.drain();
            terminal.draw(|f| ui::draw(f, &mut app))?;
            if event::poll(Duration::from_millis(80))?
                && let Event::Key(k) = event::read()?
                && k.kind == KeyEventKind::Press
            {
                app.on_key(k);
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests;
