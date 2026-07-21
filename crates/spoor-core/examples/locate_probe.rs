//! Interactive probe for `locate_quote` — explore its capability boundary.
//!
//! Two modes:
//!
//! One-shot (doc file + quote on the command line):
//!     cargo run --example locate_probe -- doc.md "营收增长 12%"
//!
//! Batch (doc file, quotes from stdin, one per line — blank lines and lines
//! starting with `#` are ignored so you can annotate your quote list):
//!     cargo run --example locate_probe -- doc.md < quotes.txt
//!     cargo run --example locate_probe -- doc.md      # then type quotes
//!
//! Output per quote:
//!     <METHOD> corrob=<bool> score=<f|-> occ=<n> page=<n|-> | <hit>
//!   or `NONE` when nothing matched.

use spoor_core::locate_quote;
use std::io::{BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(doc_path) = args.first() else {
        eprintln!("usage: locate_probe <doc.md> [quote]");
        std::process::exit(2);
    };
    let md = std::fs::read_to_string(doc_path).unwrap_or_else(|e| {
        eprintln!("cannot read {doc_path}: {e}");
        std::process::exit(2);
    });

    if let Some(quote) = args.get(1) {
        report(&md, quote);
        return;
    }

    let stdin = std::io::stdin();
    let is_tty = args.len() == 1; // hint only
    if is_tty {
        eprintln!("# reading quotes from stdin (one per line, Ctrl-D to end)");
    }
    for line in stdin.lock().lines() {
        let line = line.unwrap_or_default();
        let quote = line.trim();
        if quote.is_empty() || quote.starts_with('#') {
            continue;
        }
        print!("{quote}\n  → ");
        report(&md, quote);
        let _ = std::io::stdout().flush();
    }
}

fn report(md: &str, quote: &str) {
    match locate_quote(md, quote) {
        None => println!("NONE"),
        Some(f) => {
            let score = f.score.map_or("-".to_string(), |s| format!("{s:.3}"));
            let page = f.page.map_or("-".to_string(), |p| p.to_string());
            let hit: String = f.hit.chars().take(50).collect();
            println!(
                "{:<22} corrob={:<5} score={:<5} occ={:<3} page={:<3} | {hit}",
                f.method.as_str(),
                f.corroborated,
                score,
                f.occurrences,
                page,
            );
        }
    }
}
