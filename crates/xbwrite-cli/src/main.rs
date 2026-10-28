//! Command-line front end, mainly for scripting and testing.
//!
//!   xbwrite-cli info <images/folders...>
//!   xbwrite-cli convert [-o DIR] [--cso] [--split] [--keep-update] [--no-verify] [--replace] <images/folders...>
//!   xbwrite-cli extract [-o DIR] [--keep-update] [--overwrite] <images...>

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use xbwrite_core::convert::{convert, output_path, ConvertOptions, Format, Progress};
use xbwrite_core::extract::{extract, ExtractOptions};
use xbwrite_core::scan::{scan, Game};
use xbwrite_core::source::collect_inputs;
use xbwrite_core::util::{format_size, percent};

const USAGE: &str = "usage: xbwrite-cli info <images/folders...>
       xbwrite-cli convert [-o DIR] [--cso] [--split] [--keep-update] [--no-verify] [--replace] <images/folders...>
       xbwrite-cli extract [-o DIR] [--keep-update] [--overwrite] <images...>

  -o DIR         write here instead of next to each source
  --cso          write a compressed CSO instead of an XISO
  --split        cut images over 4 GB into .1.iso, .2.iso, ... parts (FAT32)
  --keep-update  keep the $SystemUpdate dashboard update folder
  --no-verify    don't read the new image back to check it
  --replace      delete the source image once the new one is written";

fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

/// Returns `Ok(false)` if some games failed.
fn run() -> Result<bool, Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    let mut out_dir: Option<PathBuf> = None;
    let mut opts = ConvertOptions { strip_system_update: true, verify: true, ..Default::default() };
    let mut overwrite = false;
    let mut inputs = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "-o" => out_dir = Some(args.next().ok_or("-o needs a directory")?.into()),
            "--cso" => opts.format = Format::Cso,
            "--split" => opts.split = true,
            "--keep-update" => opts.strip_system_update = false,
            "--no-verify" => opts.verify = false,
            "--replace" => opts.delete_source = true,
            "--overwrite" => overwrite = true,
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}\n{USAGE}").into()),
            _ => inputs.push(PathBuf::from(a)),
        }
    }
    if !matches!(cmd.as_str(), "info" | "convert" | "extract") || inputs.is_empty() {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }

    let mut games = Vec::new();
    let mut all_ok = true;
    for p in collect_inputs(&inputs) {
        match scan(&p) {
            Ok(g) => games.push(g),
            Err(e) => {
                eprintln!("skipping {}: {e}", p.display());
                all_ok = false;
            }
        }
    }
    for g in &games {
        print_game(g, opts.strip_system_update);
    }

    let cancel = AtomicBool::new(false);
    for g in &games {
        let dir = out_dir.clone().or_else(|| g.source.parent().map(PathBuf::from)).unwrap_or_default();
        let start = Instant::now();
        let result = match cmd.as_str() {
            "convert" => {
                if g.already_trimmed(opts.strip_system_update) && opts.format == Format::Xiso {
                    println!("\n{} is already a trimmed XISO, skipping", g.source.display());
                    continue;
                }
                let path = output_path(g, &dir, opts.format, opts.delete_source);
                println!("\nWriting {}", path.display());
                convert(g, &path, &opts, &cancel, &mut print_progress())
            }
            "extract" => {
                if !g.is_image() {
                    println!("\n{} is already a folder, skipping", g.source.display());
                    continue;
                }
                let path = dir.join(&g.name);
                println!("\nExtracting to {}", path.display());
                let eo = ExtractOptions { strip_system_update: opts.strip_system_update, overwrite };
                extract(g, &path, &eo, &cancel, &mut print_progress()).map(|p| vec![p])
            }
            _ => continue,
        };
        eprintln!();
        match result {
            Ok(written) => {
                println!("  done in {:.1}s", start.elapsed().as_secs_f64());
                for p in written {
                    match p.metadata() {
                        Ok(m) if m.is_file() => println!("  wrote {} ({})", p.display(), format_size(m.len())),
                        _ => println!("  wrote {}", p.display()),
                    }
                }
            }
            Err(e) => {
                eprintln!("  failed: {e}");
                all_ok = false;
            }
        }
    }
    Ok(all_ok)
}

fn print_game(g: &Game, strip: bool) {
    println!("\n{}  [{}]", g.title(), g.kind_label());
    if let Some(x) = &g.xbe {
        println!(
            "  title id {} ({:08X})  region {}  version {}  disc {}",
            x.title_id_label(),
            x.title_id,
            x.region_codes().join("/"),
            x.version,
            x.disc_number + 1
        );
    }
    let out = g.output_size(strip);
    println!(
        "  {} files  ·  source {}  ·  XISO {} (saves {}%)",
        g.file_count,
        format_size(g.source_size),
        format_size(out),
        percent(g.source_size.saturating_sub(out), g.source_size)
    );
    if g.system_update_size > 0 {
        let what = if strip { "left out" } else { "kept" };
        println!("  $SystemUpdate {} ({what})", format_size(g.system_update_size));
    }
    println!("  {}", g.source.display());
}

/// Progress callback: log lines as they come, plus a percentage line on stderr that is
/// redrawn at most twice a second.
fn print_progress() -> impl FnMut(Progress) + Send + Sync {
    let mut last = Instant::now();
    let mut phase = "";
    move |p| match p {
        Progress::Log(m) => println!("  {m}"),
        Progress::Phase(ph) => {
            if !phase.is_empty() {
                eprintln!();
            }
            phase = ph.label();
        }
        Progress::Bytes { done, total } => {
            if last.elapsed().as_millis() > 500 || done == total {
                last = Instant::now();
                eprint!(
                    "\r  {phase:<11} {:5.1}%  {} / {}   ",
                    done as f64 * 100.0 / total.max(1) as f64,
                    format_size(done),
                    format_size(total)
                );
            }
        }
    }
}
