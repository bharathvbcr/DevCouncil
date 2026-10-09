//! The per-cycle samples file and the plateau assertion read from it.
//!
//! Every cycle is sampled (cycle, peak or resident RSS, store bytes). A write
//! that fails is a failed soak: the plateau is computed from this file, and a
//! file that silently lost rows would compare whatever survived.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const HEADER: &str = "cycle,rss_bytes,db_bytes";

/// One row of the samples file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub cycle: u64,
    pub rss: u64,
    pub db: u64,
}

/// The samples file. Created with its header; appended to once per cycle.
#[derive(Debug)]
pub struct Samples {
    path: PathBuf,
}

impl Samples {
    /// Create (or truncate) the file and write its header.
    pub fn create(path: &Path) -> io::Result<Self> {
        let mut file = File::create(path)?;
        writeln!(file, "{HEADER}")?;
        file.sync_data()?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, sample: Sample) -> io::Result<()> {
        let mut file = OpenOptions::new().append(true).open(&self.path)?;
        writeln!(file, "{},{},{}", sample.cycle, sample.rss, sample.db)?;
        file.flush()
    }

    /// Every data row, in file order.
    pub fn read(&self) -> io::Result<Vec<Sample>> {
        let text = std::fs::read_to_string(&self.path)?;
        let mut rows = Vec::new();
        for (n, line) in text.lines().enumerate().skip(1) {
            let fields: Vec<&str> = line.split(',').collect();
            let parse = |i: usize| fields.get(i).and_then(|f| f.trim().parse::<u64>().ok());
            match (parse(0), parse(1), parse(2)) {
                (Some(cycle), Some(rss), Some(db)) => rows.push(Sample { cycle, rss, db }),
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "{} line {} is not cycle,rss,db: {line:?}",
                            self.path.display(),
                            n + 1
                        ),
                    ))
                }
            }
        }
        Ok(rows)
    }
}

/// Reads one sampled column from a row.
type Column = fn(&Sample) -> u64;

/// Mean of one column over cycles `from..=to`, truncated toward zero; `None`
/// when no row falls in the range.
fn mean(rows: &[Sample], from: u64, to: u64, column: Column) -> Option<u64> {
    let (sum, n) = rows
        .iter()
        .filter(|r| r.cycle >= from && r.cycle <= to)
        .fold((0u128, 0u128), |(s, n), r| {
            (s + u128::from(column(r)), n + 1)
        });
    (n > 0).then(|| (sum / n) as u64)
}

/// Assert that RSS and store bytes plateau. Prints one verdict line per
/// sampled column and returns false on a failed comparison — or on a
/// comparison that could not run, which is reported as a failure and never as
/// a plateau.
///
/// Warm-up is reported but not compared, and it is not a fixed fraction of
/// the run. In build mode every cycle is a whole `devmap build`, so the first
/// quarter is allocator settling and the data has been flat from cycle 1 on
/// both corpora. In daemon mode the working set arrives with the first drain:
/// the watcher holds a burst of edits for its 2 s quiet window and at most
/// 10 s, then writes a generation and builds its index — RSS 240 MB -> ~900 MB
/// and the store 247 MB -> 498 MB on the scholarlm corpus. That is wall-clock,
/// while a cycle on a small corpus takes half a second; so the cycle it lands
/// on is read from the samples (the first store-bytes change after the
/// baseline) rather than assumed. A 40-cycle daemon run on this repository's
/// corpus put the first drain at cycle 30 and then compared cycles 11-25
/// against 26-40, calling the process loading its graph a leak.
///
/// After warm-up the rest is split in half and the two *means* are compared: a
/// single sample carries allocator noise a mean does not — over the last
/// hundred cycles of the run that set the tolerance the daemon's per-cycle RSS
/// swung between 755 MB and 953 MB (±12% around its mean) while the mean moved
/// -6.24%. The tolerance is the ceiling on that movement, 10% by default: four
/// times the observed drift, and far below what a leak does — the same run's
/// warm-up moved +210%, which is the shape this looks for.
pub fn assert_plateau(
    rows: &[Sample],
    cycles: u64,
    daemon: bool,
    base_db: u64,
    tolerance_pct: u64,
    out: &mut dyn Write,
) -> io::Result<bool> {
    let warmup = if daemon {
        let Some(first_drain) = rows.iter().find(|r| r.db != base_db).map(|r| r.cycle) else {
            writeln!(
                out,
                "SOAK FAIL: the daemon never wrote a generation in {cycles} cycles, so the plateau check could not run"
            )?;
            return Ok(false);
        };
        // The generations that follow the first, and the index built over
        // them, keep arriving for a while after it: a quarter of what remains
        // is settle.
        let warmup = first_drain + cycles.saturating_sub(first_drain) / 4;
        writeln!(
            out,
            "daemon first drain landed at cycle {first_drain}; warm-up ends at cycle {warmup}"
        )?;
        warmup
    } else {
        cycles / 4
    };
    let remaining = cycles.saturating_sub(warmup);
    if remaining < 30 {
        writeln!(
            out,
            "SOAK FAIL: {remaining} cycles after warm-up (which ends at cycle {warmup} of {cycles}) is fewer than the 30 a two-half comparison needs — run more cycles"
        )?;
        return Ok(false);
    }
    let mid = warmup + remaining / 2;
    let mut ok = true;
    let columns: [(&str, Column); 2] = [("rss", |r| r.rss), ("db", |r| r.db)];
    for (name, column) in columns {
        let first = mean(rows, warmup + 1, mid, column);
        let second = mean(rows, mid + 1, cycles, column);
        let (Some(first), Some(second)) = (first, second) else {
            writeln!(out, "SOAK FAIL: {name} has no samples in one of the halves — the plateau check could not run")?;
            ok = false;
            continue;
        };
        if first == 0 {
            writeln!(out, "SOAK FAIL: {name} has no samples in one of the halves — the plateau check could not run")?;
            ok = false;
            continue;
        }
        let limit = first + first * tolerance_pct / 100;
        let span = format!(
            "mean({}-{mid}) {first} -> mean({}-{cycles}) {second} (limit {limit})",
            warmup + 1,
            mid + 1
        );
        if second > limit {
            writeln!(out, "SOAK FAIL: {name} did not plateau — {span}")?;
            ok = false;
        } else {
            writeln!(out, "plateau ok: {name} {span}")?;
        }
    }
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(cycles: u64, rss: impl Fn(u64) -> u64, db: impl Fn(u64) -> u64) -> Vec<Sample> {
        (1..=cycles)
            .map(|c| Sample {
                cycle: c,
                rss: rss(c),
                db: db(c),
            })
            .collect()
    }

    fn verdict(rows: &[Sample], cycles: u64, daemon: bool, base_db: u64) -> (bool, String) {
        let mut out = Vec::new();
        let ok =
            assert_plateau(rows, cycles, daemon, base_db, 10, &mut out).expect("writes to a Vec");
        (ok, String::from_utf8(out).expect("utf-8"))
    }

    #[test]
    fn every_write_reaches_the_file_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("soak-samples-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("samples.csv");
        let samples = Samples::create(&path).expect("create");
        // The daemon row is resident KiB * 1024; the build row is peak bytes.
        samples
            .append(Sample {
                cycle: 1,
                rss: 123 * 1024,
                db: 456,
            })
            .expect("daemon row");
        samples
            .append(Sample {
                cycle: 2,
                rss: 123,
                db: 456,
            })
            .expect("build row");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "cycle,rss_bytes,db_bytes\n1,125952,456\n2,123,456\n"
        );
        assert_eq!(samples.read().expect("parse").len(), 2);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn a_samples_path_that_cannot_be_written_is_an_error_not_a_silent_gap() {
        let dir = std::env::temp_dir().join(format!("soak-samples-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        // A directory in the file's place: the header write and every append fail.
        assert!(Samples::create(&dir).is_err());
        let samples = Samples { path: dir.clone() };
        assert!(samples
            .append(Sample {
                cycle: 1,
                rss: 1,
                db: 1
            })
            .is_err());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn a_flat_build_run_plateaus() {
        let (ok, out) = verdict(&rows(40, |_| 1000, |_| 500), 40, false, 500);
        assert!(ok, "{out}");
        assert!(
            out.contains("plateau ok: rss mean(11-25) 1000 -> mean(26-40) 1000 (limit 1100)"),
            "{out}"
        );
    }

    #[test]
    fn growth_past_the_tolerance_fails() {
        let (ok, out) = verdict(&rows(40, |c| 1000 + c * 20, |_| 500), 40, false, 500);
        assert!(!ok, "{out}");
        assert!(out.contains("SOAK FAIL: rss did not plateau"), "{out}");
    }

    #[test]
    fn a_run_too_short_to_compare_says_so() {
        // 39 cycles leave exactly 30 after a 9-cycle warm-up, which is enough;
        // 38 leave 29.
        let (ok, _) = verdict(&rows(39, |_| 1000, |_| 500), 39, false, 500);
        assert!(ok);
        let (ok, out) = verdict(&rows(38, |_| 1000, |_| 500), 38, false, 500);
        assert!(!ok);
        assert!(
            out.contains("fewer than the 30 a two-half comparison needs"),
            "{out}"
        );
    }

    #[test]
    fn a_daemon_that_never_drained_cannot_be_compared() {
        let (ok, out) = verdict(&rows(100, |_| 1000, |_| 500), 100, true, 500);
        assert!(!ok);
        assert!(
            out.contains("never wrote a generation in 100 cycles"),
            "{out}"
        );
    }

    #[test]
    fn daemon_warm_up_ends_after_the_first_drain_not_at_a_fixed_quarter() {
        // The drain lands at cycle 30 and doubles both columns; a fixed
        // quarter would compare 26-63 against 64-100 and see growth there.
        let rss = |c: u64| if c < 30 { 1000 } else { 2000 };
        let db = |c: u64| if c < 30 { 500 } else { 900 };
        let (ok, out) = verdict(&rows(100, rss, db), 100, true, 500);
        assert!(ok, "{out}");
        assert!(
            out.contains("daemon first drain landed at cycle 30; warm-up ends at cycle 47"),
            "{out}"
        );
    }

    #[test]
    fn a_zero_first_half_could_not_run() {
        let (ok, out) = verdict(&rows(40, |_| 0, |_| 500), 40, false, 500);
        assert!(!ok);
        assert!(
            out.contains("SOAK FAIL: rss has no samples in one of the halves"),
            "{out}"
        );
    }
}
