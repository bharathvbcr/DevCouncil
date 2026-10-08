//! `dc-sparse-encode` — the offline encoder behind `dcgrep index --sparse`.
//!
//! Apple silicon only. The forward is tessl's, the query path in `dcgrep`
//! does not load a model, and this binary is not linked into `dcgrep`.

use std::path::PathBuf;
use std::process::ExitCode;

use dc_sparse_encode::{
    frozen_record_message, self_test, walk, DEFAULT_MAX_FILES, DEFAULT_MODEL, MIN_REPEATS,
};

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(2)
        }
    }
}

struct Args {
    root: PathBuf,
    out: Option<PathBuf>,
    model: String,
    batch_size: usize,
    max_files: usize,
    max_files_set: bool,
    self_test: bool,
    record: bool,
    walk: bool,
    bench: bool,
    repeats: usize,
}

fn run() -> Result<u8, String> {
    let args = parse(std::env::args().skip(1))?;
    if args.record {
        println!("{}", frozen_record_message());
        return Ok(0);
    }
    if args.self_test {
        return match self_test() {
            Ok(()) => Ok(0),
            Err(err) => {
                eprintln!("{err}");
                Ok(1)
            }
        };
    }
    if args.bench {
        return bench(&args);
    }
    let Some(out) = args.out else {
        return Err("--out is required".into());
    };
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        dc_sparse_encode::encode_repository(
            &args.root,
            &out,
            &args.model,
            args.batch_size,
            args.max_files,
            args.max_files_set,
            args.walk,
        )?;
        return Ok(0);
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (out, args);
        Err(
            "dc-sparse-encode runs the model on tessl, which is Apple silicon only. \
             --self-test and --record do not need a model; encoding does."
                .into(),
        )
    }
}

fn bench(args: &Args) -> Result<u8, String> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = args;
        return Err("the benchmark runs the tessl forward, which is Apple silicon only".into());
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        if args.repeats < MIN_REPEATS {
            return Err(format!(
                "--repeats must be at least {MIN_REPEATS}; a shorter run is not a min-of-N comparison"
            ));
        }
        let files = walk(&args.root, args.max_files)?;
        if files.is_empty() {
            return Err(format!("no encodable files under {}", args.root.display()));
        }
        let report = dc_sparse_encode::bench(&args.model, &files, args.repeats)?;
        println!("{}", dc_sparse_encode::format_bench(&args.model, &report));
        Ok(0)
    }
}

fn parse(argv: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args {
        root: PathBuf::from("."),
        out: None,
        model: DEFAULT_MODEL.to_string(),
        batch_size: 8,
        max_files: DEFAULT_MAX_FILES,
        max_files_set: false,
        self_test: false,
        record: false,
        walk: false,
        bench: false,
        repeats: MIN_REPEATS,
    };
    let mut argv = argv.peekable();
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--self-test" => args.self_test = true,
            "--record" => args.record = true,
            "--walk" => args.walk = true,
            "--bench" => args.bench = true,
            "--root" => args.root = PathBuf::from(need(&mut argv, "--root")?),
            "--out" => args.out = Some(PathBuf::from(need(&mut argv, "--out")?)),
            "--model" => args.model = need(&mut argv, "--model")?,
            "--batch-size" => {
                args.batch_size = parse_usize(&need(&mut argv, "--batch-size")?, "--batch-size")?;
            }
            "--max-files" => {
                args.max_files = parse_usize(&need(&mut argv, "--max-files")?, "--max-files")?;
                args.max_files_set = true;
            }
            "--repeats" => {
                args.repeats = parse_usize(&need(&mut argv, "--repeats")?, "--repeats")?;
            }
            "--device" => {
                return Err(
                    "--device is gone. The forward runs on tessl's Metal kernels, on Apple silicon, \
                     and there is no CPU or CUDA path."
                        .into(),
                );
            }
            "--dcgrep" | "--fixtures" => {
                let _ = need(&mut argv, &arg)?;
                return Err(format!(
                    "{arg} is gone. The file list is dcgrep's own walk, in process; \
                     --record prints why the wordpiece fixture is frozen."
                ));
            }
            other if other.starts_with('-') => return Err(format!("unknown argument {other}")),
            other => return Err(format!("unexpected argument {other:?}")),
        }
    }
    Ok(args)
}

fn need(argv: &mut std::iter::Peekable<impl Iterator<Item = String>>, flag: &str) -> Result<String, String> {
    argv.next().ok_or_else(|| format!("{flag} needs a value"))
}

fn parse_usize(text: &str, flag: &str) -> Result<usize, String> {
    text.parse::<usize>()
        .map_err(|_| format!("{flag} must be a positive integer, got {text:?}"))
}

fn print_help() {
    println!(
        "\
Encode a repository for dcgrep's learned sparse ranking.

The forward runs on tessl, on Apple silicon. There is no --device. dcgrep
itself never loads a model: this writes the JSONL, and `dcgrep index`
imports it. Linux and Windows can search that index; they cannot run this
binary's forward.

The default model is {model} (Apache-2.0, 23M). The doc-side family shares
one 30522-token WordPiece vocabulary:

    doc-v2-mini       23M    fastest, smallest; the default
    doc-v2-distill    67M    the usual default elsewhere
    doc-v3-distill   133M    strongest of the Apache-2.0 doc-side family

SPLADE (naver/splade-*) emits the same vocabulary. Its weights are
CC BY-NC-SA 4.0 (non-commercial). This binary prints a note and continues.
A model that is not doc-side would need inference on every query, and
dcgrep has nowhere to put that.

    cargo build --manifest-path rust/dc-sparse-encode/Cargo.toml --release
    ./rust/dc-sparse-encode/target/release/dc-sparse-encode --root . --out /tmp/sparse.jsonl
    dcgrep index <<< '{{\"root\":\".\",\"sparse\":\"/tmp/sparse.jsonl\"}}'

  --root PATH        repository to encode (default .)
  --out PATH         JSONL to write
  --model ID         Hugging Face model id, read from the local cache
  --batch-size N     documents per forward (default 8)
  --max-files N      stop after N listed files; explicit, so a prefix is allowed
  --walk             do not ask dcgrep which files the index would admit
  --self-test        check the JSONL against dcgrep, with no model
  --record           print why the wordpiece fixture is frozen
  --bench            interleaved min-of-N timing against the torch oracle
  --repeats N        bench repeats (default {repeats}, minimum {repeats})
",
        model = DEFAULT_MODEL,
        repeats = MIN_REPEATS,
    );
}
