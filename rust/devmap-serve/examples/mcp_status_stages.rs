//! Where does one warm `devmap_status` spend its time, and which stages change
//! with the connection's page-cache size?
//!
//! ```text
//! mcp_status_stages <store.sqlite> <repeats> <cache_kib> [<cache_kib>...]
//! ```
//!
//! One `Store` is opened per cache size, in this process, over the same file.
//! Each stage runs once untimed (warm-up), then `repeats` times per store with
//! the stores interleaved in a rotating order, so no store owns a position.
//! Reported per stage and store: the median in ms. The stages are the calls
//! `status` makes, each timed alone, plus the whole `Store::status`.

use std::path::Path;
use std::time::Instant;

use devmap_store::{ReaderMemory, Store};

type Stage = (&'static str, fn(&Store, &str));

fn stages() -> Vec<Stage> {
    vec![
        ("Store::status (whole)", |s, db| {
            s.status(db).expect("status");
        }),
        ("search_index_damage (fts health)", |s, _| {
            s.search_index_damage().expect("fts");
        }),
        ("latest_file_hashes", |s, _| {
            s.latest_file_hashes().expect("hashes");
        }),
        ("latest_discovery_inventory", |s, _| {
            s.latest_discovery_inventory().expect("inventory");
        }),
        ("latest_generation_payload_is_current", |s, _| {
            s.latest_generation_payload_is_current().expect("payload");
        }),
        ("edge_confidence_mismatches", |s, _| {
            s.edge_confidence_mismatches().expect("mismatches");
        }),
        ("latest_edge_resolution_source", |s, _| {
            s.latest_edge_resolution_source().expect("source");
        }),
        ("latest_resolution_rate", |s, _| {
            s.latest_resolution_rate().expect("rate");
        }),
        ("scan_tree (filesystem walk, no store)", |s, _| {
            let root = s
                .latest_repo_root()
                .expect("root")
                .expect("a repo root is bound");
            devmap_extract::scan_tree(Path::new(&root)).expect("scan");
        }),
    ]
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let db = args[0].clone();
    let repeats: usize = args[1].parse().expect("repeats");
    let sizes: Vec<u32> = args[2..]
        .iter()
        .map(|s| s.parse().expect("cache_kib"))
        .collect();
    let stores: Vec<Store> = sizes
        .iter()
        .map(|kib| {
            Store::open_read_only_with(
                &db,
                ReaderMemory {
                    cache_kib: *kib,
                    ..ReaderMemory::CURRENT
                },
            )
            .expect("open")
        })
        .collect();
    println!("cache sizes (KiB): {sizes:?}; repeats {repeats}");
    for (label, stage) in stages() {
        let mut samples: Vec<Vec<f64>> = vec![Vec::new(); stores.len()];
        for store in &stores {
            stage(store, &db);
        }
        for round in 0..repeats {
            for offset in 0..stores.len() {
                let index = (round + offset) % stores.len();
                let started = Instant::now();
                stage(&stores[index], &db);
                samples[index].push(started.elapsed().as_secs_f64() * 1000.0);
            }
        }
        let medians: Vec<String> = samples
            .iter_mut()
            .zip(&sizes)
            .map(|(values, kib)| format!("{}K {:>8.2}", kib, median(values)))
            .collect();
        println!("{label:<42} {}", medians.join("   "));
    }
}
