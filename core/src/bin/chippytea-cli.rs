use chippytea_core::{Engine, model::*, safety, scanner};
use serde_json::json;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(String::as_str).unwrap_or("help");
    let path = args.get(2).map(Path::new);
    let kind = if let Some(index) = args.iter().position(|arg| arg == "--kind") {
        args.get(index + 1)
            .ok_or("Expected home, folder, projects or downloads after --kind")?
            .as_str()
    } else {
        "projects"
    };
    let metadata_coverage = args.iter().any(|arg| arg == "--metadata-coverage");
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(index) = args.iter().position(|s| s == "--cancel-after-ms") {
        let millis = args
            .get(index + 1)
            .ok_or("Missing milliseconds")?
            .parse::<u64>()
            .map_err(|e| e.to_string())?;
        let flag = Arc::clone(&cancel);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(millis));
            flag.store(true, Ordering::Release);
        });
    }
    match command {
        "traverse" => {
            let root = path
                .ok_or("Expected fixture path")?
                .canonicalize()
                .map_err(|e| e.to_string())?;
            let stats = safety::traverse_metadata(&root, &cancel)?;
            println!("{}", serde_json::to_string(&stats).unwrap());
        }
        "scan" => {
            let path = path
                .ok_or("Expected authorized path")?
                .canonicalize()
                .map_err(|e| e.to_string())?;
            let root = safety::authorize(&path, kind)?;
            let mode = if metadata_coverage {
                scanner::ScanMode::MetadataCoverage
            } else {
                scanner::ScanMode::Suggestions
            };
            let stats = scanner::scan_with_checkpoint_mode(
                &root,
                None,
                &[],
                &cancel,
                mode,
                || {},
                |batch| println!("{}", serde_json::to_string(&batch).unwrap()),
            )?;
            println!("{}", serde_json::to_string(&stats).unwrap());
        }
        "index" => {
            let root = path
                .ok_or("Expected fixture path")?
                .canonicalize()
                .map_err(|e| e.to_string())?;
            let directory =
                std::env::temp_dir().join(format!("chippytea-index-benchmark-{}", unique_id()));
            std::fs::create_dir(&directory).map_err(|e| e.to_string())?;
            let engine = Engine::open(&directory.join("index.sqlite"), None)?;
            engine.request(json!({"action":"authorize","path":root,"kind":kind}))?;
            engine.request(json!({"action":"scan","metadata_coverage":metadata_coverage}))?;
            loop {
                if cancel.load(Ordering::Acquire) {
                    engine.cancel_scan();
                }
                let snapshot = engine.snapshot()?;
                println!(
                    "{}",
                    serde_json::to_string(&snapshot).map_err(|e| e.to_string())?
                );
                if !snapshot.scanning {
                    println!(
                        "{}",
                        serde_json::to_string(&snapshot.stats).map_err(|e| e.to_string())?
                    );
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            eprintln!("Disposable index retained at {}", directory.display());
        }
        "--version" | "-V" => println!("chippytea-cli {}", env!("CARGO_PKG_VERSION")),
        _ => {
            println!(
                "Chippytea benchmark CLI\n  chippytea-cli traverse <fixture> [--cancel-after-ms N]\n  chippytea-cli scan <authorized-folder> [--kind projects|home|folder|downloads] [--metadata-coverage] [--cancel-after-ms N]\n  chippytea-cli index <fixture> [--cancel-after-ms N]\nIndex uses a new disposable database. No CLI command deletes files."
            );
        }
    }
    Ok(())
}
