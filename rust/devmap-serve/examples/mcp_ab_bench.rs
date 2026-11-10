//! A/B of two `devmap` binaries serving the same store over MCP stdio: what an
//! idle reader holds, what its first query costs cold and after idle eviction,
//! and whether warm queries moved.
//!
//! Run (on a COPY of a store, never a live one; release binaries):
//! ```text
//! mcp_ab_bench --store <copy.sqlite> --rounds 23 --discard 2 --seed 1 \
//!     --evict-wait-secs 660 --target <t1> --target <t2> --target <t3> \
//!     --arm base=<devmap-base> --arm branch=<devmap-branch>
//! ```
//! A null pair is the same binary under two arm names (`--arm a=X --arm b=X`).
//!
//! # Method
//!
//! Every sample is one real `devmap mcp --db <store>` process driven over
//! stdio, so the binary under test is exactly what ships.
//!
//! * Phase 1, per round: the arms run in an order that alternates each round
//!   (the seed picks round 0's), so no arm owns a position in the order. Before
//!   each spawn the whole store file is read, so both arms start with the same
//!   OS file cache: "cold" here means a new process with empty SQLite and edge
//!   index caches over a warm file cache. The first call is `devmap_impact` on
//!   the first target (it includes opening the store and building the edge
//!   index), then `status`, the other targets, and cycles of both until
//!   `--queries` calls were made. After one second idle the process's
//!   resident set (`ps`) and physical footprint (`vmmap --summary`) are read.
//!   The process is then left running, idle.
//! * Phase 2: once `--evict-wait-secs` have passed since the last phase-1 call
//!   (longer than the idle TTL plus a sweep), each process, in the same
//!   order, has its memory read again and is sent the first target again,
//!   timed; then it is killed. A binary with no idle eviction is simply idle.
//! * The first `--discard` rounds are run and not reported.
//!
//! Warm latency of a process is the median of its calls after the first four.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

struct Config {
    store: String,
    rounds: usize,
    discard: usize,
    seed: u64,
    evict_wait: Duration,
    queries: usize,
    targets: Vec<String>,
    arms: Vec<(String, String)>,
}

struct Sample {
    name: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    cold_first_ms: f64,
    warm_impact_ms: f64,
    warm_status_ms: f64,
    rss_mib: f64,
    foot_mib: f64,
    last_call: Instant,
}

fn parse(args: &[String]) -> Config {
    let mut config = Config {
        store: String::new(),
        rounds: 23,
        discard: 2,
        seed: 1,
        evict_wait: Duration::from_secs(660),
        queries: 40,
        targets: Vec::new(),
        arms: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let value = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--store" => config.store = value,
            "--rounds" => config.rounds = value.parse().expect("rounds"),
            "--discard" => config.discard = value.parse().expect("discard"),
            "--seed" => config.seed = value.parse().expect("seed"),
            "--evict-wait-secs" => {
                config.evict_wait = Duration::from_secs(value.parse().expect("secs"))
            }
            "--queries" => config.queries = value.parse().expect("queries"),
            "--target" => config.targets.push(value),
            "--arm" => {
                let (name, path) = value.split_once('=').expect("--arm name=path");
                config.arms.push((name.to_string(), path.to_string()));
            }
            other => panic!("unknown argument {other}"),
        }
        i += 2;
    }
    assert!(!config.store.is_empty() && !config.targets.is_empty());
    assert_eq!(config.arms.len(), 2, "exactly two arms");
    assert!(config.rounds > config.discard);
    config
}

fn load_average() -> String {
    let out = Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .expect("sysctl");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Read the whole file so every arm starts over the same OS file cache.
fn warm_file_cache(path: &str) {
    let mut file = std::fs::File::open(path).expect("open store");
    let mut buffer = vec![0u8; 8 << 20];
    while file.read(&mut buffer).expect("read store") > 0 {}
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

impl Sample {
    fn spawn(name: &str, binary: &str, store: &str) -> Self {
        let mut child = Command::new(binary)
            .args(["mcp", "--db", store])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|error| panic!("spawn {binary}: {error}"));
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut sample = Self {
            name: name.to_string(),
            child,
            stdin,
            stdout,
            next_id: 1,
            cold_first_ms: 0.0,
            warm_impact_ms: 0.0,
            warm_status_ms: 0.0,
            rss_mib: 0.0,
            foot_mib: 0.0,
            last_call: Instant::now(),
        };
        let init = json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                       "clientInfo": {"name": "mcp_ab_bench", "version": "0"}}
        });
        sample.send(&init);
        sample.read_response(0);
        sample.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        sample
    }

    fn send(&mut self, frame: &Value) {
        writeln!(self.stdin, "{frame}").expect("write frame");
        self.stdin.flush().expect("flush");
    }

    fn read_response(&mut self, id: u64) -> Value {
        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).expect("read frame");
            assert!(read > 0, "server closed stdout");
            let value: Value = serde_json::from_str(line.trim()).expect("json frame");
            if value.get("id") == Some(&json!(id)) {
                return value;
            }
        }
    }

    /// One tool call, timed from the write to the end of the response line.
    fn call(&mut self, tool: &str, arguments: Value) -> f64 {
        let id = self.next_id;
        self.next_id += 1;
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                           "params": {"name": tool, "arguments": arguments}});
        let started = Instant::now();
        self.send(&frame);
        let response = self.read_response(id);
        let elapsed = started.elapsed();
        self.last_call = Instant::now();
        assert_eq!(
            response["result"]["isError"],
            json!(false),
            "{tool} failed: {response}"
        );
        millis(elapsed)
    }

    fn memory(&mut self) {
        let pid = self.child.id().to_string();
        let ps = Command::new("ps")
            .args(["-o", "rss=", "-p", &pid])
            .output()
            .expect("ps");
        let kib: f64 = String::from_utf8_lossy(&ps.stdout)
            .trim()
            .parse()
            .expect("rss");
        self.rss_mib = kib / 1024.0;
        let vmmap = Command::new("vmmap")
            .args(["--summary", &pid])
            .output()
            .expect("vmmap");
        let text = String::from_utf8_lossy(&vmmap.stdout);
        let line = text
            .lines()
            .find(|line| line.starts_with("Physical footprint:"))
            .unwrap_or_else(|| panic!("no footprint line: {text}"));
        let figure = line.split_whitespace().nth(2).expect("figure");
        let (number, unit) = figure.split_at(figure.len() - 1);
        let number: f64 = number.parse().expect("number");
        self.foot_mib = match unit {
            "K" => number / 1024.0,
            "M" => number,
            "G" => number * 1024.0,
            other => panic!("unit {other} in {line}"),
        };
    }
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

fn run(config: &Config) {
    println!("load before: {}", load_average());
    let mut live: Vec<(usize, usize, Sample)> = Vec::new(); // (round, arm index, sample)
                                                            // Phase 1.
    for round in 0..config.rounds {
        let first = ((round as u64 + config.seed) % 2) as usize;
        for position in 0..2 {
            let arm = (first + position) % 2;
            let (name, binary) = &config.arms[arm];
            warm_file_cache(&config.store);
            let mut sample = Sample::spawn(name, binary, &config.store);
            sample.cold_first_ms =
                sample.call("devmap_impact", json!({"target": config.targets[0]}));
            let mut impacts = Vec::new();
            let mut statuses = Vec::new();
            for index in 1..config.queries {
                let cycle = index % (config.targets.len() + 1);
                let (millis, is_status) = if cycle == 0 {
                    (sample.call("devmap_status", json!({})), true)
                } else {
                    let target = &config.targets[cycle - 1];
                    (
                        sample.call("devmap_impact", json!({"target": target})),
                        false,
                    )
                };
                if index >= 4 {
                    if is_status {
                        statuses.push(millis);
                    } else {
                        impacts.push(millis);
                    }
                }
            }
            sample.warm_impact_ms = median(&impacts);
            sample.warm_status_ms = median(&statuses);
            std::thread::sleep(Duration::from_secs(1));
            sample.memory();
            live.push((round, arm, sample));
        }
    }
    let load_phase1 = load_average();
    println!("load after phase 1: {load_phase1}");

    // Phase 2: wait out the idle TTL measured from the last call of the last process.
    let last_call = live.iter().map(|(_, _, s)| s.last_call).max().unwrap();
    let ready = last_call + config.evict_wait;
    let now = Instant::now();
    if ready > now {
        std::thread::sleep(ready - now);
    }
    struct Row {
        round: usize,
        name: String,
        arm: usize,
        cold_first_ms: f64,
        warm_impact_ms: f64,
        warm_status_ms: f64,
        rss_n: f64,
        foot_n: f64,
        rss_evicted: f64,
        foot_evicted: f64,
        after_evict_ms: f64,
    }
    let mut rows = Vec::new();
    for (round, arm, mut sample) in live {
        let (rss_n, foot_n) = (sample.rss_mib, sample.foot_mib);
        sample.memory();
        let (rss_evicted, foot_evicted) = (sample.rss_mib, sample.foot_mib);
        let after_evict_ms = sample.call("devmap_impact", json!({"target": config.targets[0]}));
        rows.push(Row {
            round,
            name: sample.name.clone(),
            arm,
            cold_first_ms: sample.cold_first_ms,
            warm_impact_ms: sample.warm_impact_ms,
            warm_status_ms: sample.warm_status_ms,
            rss_n,
            foot_n,
            rss_evicted,
            foot_evicted,
            after_evict_ms,
        });
        let _ = sample.child.kill();
        let _ = sample.child.wait();
    }
    println!("load after phase 2: {}", load_average());

    for row in &rows {
        println!(
            "{}",
            json!({"round": row.round, "arm": row.name, "cold_first_ms": row.cold_first_ms,
                   "warm_impact_ms": row.warm_impact_ms, "warm_status_ms": row.warm_status_ms,
                   "rss_after_n_mib": row.rss_n, "footprint_after_n_mib": row.foot_n,
                   "rss_after_idle_mib": row.rss_evicted,
                   "footprint_after_idle_mib": row.foot_evicted,
                   "first_after_idle_ms": row.after_evict_ms})
        );
    }

    type Pick = fn(&Row) -> f64;
    let quantities: [(&str, Pick); 8] = [
        ("rss after N queries (MiB)", |r| r.rss_n),
        ("footprint after N queries (MiB)", |r| r.foot_n),
        ("rss after idle eviction window (MiB)", |r| r.rss_evicted),
        ("footprint after idle eviction window (MiB)", |r| {
            r.foot_evicted
        }),
        ("first query, cold reader (ms)", |r| r.cold_first_ms),
        ("first query after idle eviction window (ms)", |r| {
            r.after_evict_ms
        }),
        ("warm impact, median per process (ms)", |r| r.warm_impact_ms),
        ("warm status, median per process (ms)", |r| r.warm_status_ms),
    ];
    println!(
        "\nrounds reported: {} (of {}, first {} discarded); N = {} calls; arms: {} vs {}; seed {}",
        config.rounds - config.discard,
        config.rounds,
        config.discard,
        config.queries,
        config.arms[0].0,
        config.arms[1].0,
        config.seed
    );
    for (label, pick) in &quantities {
        let mut per_arm: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
        let mut diffs = Vec::new();
        for round in config.discard..config.rounds {
            let value = |arm: usize| {
                rows.iter()
                    .find(|r| r.round == round && r.arm == arm)
                    .map(pick)
                    .expect("row")
            };
            let (a, b) = (value(0), value(1));
            per_arm[0].push(a);
            per_arm[1].push(b);
            diffs.push(b - a);
        }
        let range = |v: &Vec<f64>| {
            (
                v.iter().cloned().fold(f64::INFINITY, f64::min),
                v.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            )
        };
        let (a_min, a_max) = range(&per_arm[0]);
        let (b_min, b_max) = range(&per_arm[1]);
        let better = diffs.iter().filter(|d| **d < 0.0).count();
        println!(
            "{label}\n  {a}: median {am:.2} min {a_min:.2} max {a_max:.2}\n  {b}: median {bm:.2} min {b_min:.2} max {b_max:.2}\n  paired diff ({b} - {a}): median {dm:.2}; {b} lower in {better}/{n} rounds",
            a = config.arms[0].0,
            b = config.arms[1].0,
            am = median(&per_arm[0]),
            bm = median(&per_arm[1]),
            dm = median(&diffs),
            n = diffs.len(),
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(&parse(&args));
}
