//! Randomised lines, built from the fragments credentials and the code around
//! them are made of, checked against the properties every consumer relies on.
//!
//! The example tests in `src/lib.rs` prove each detector on the lines someone
//! thought to write. These prove the rules that must hold on lines nobody
//! wrote: that the gate and the redactor agree, that redaction leaves nothing
//! a scan would still flag, that it is idempotent, that it keeps every line
//! ending, that no input panics, and that a vendor key the gate reports is not
//! left on display beside the part it did redact.
//!
//! The seed is fixed so a failure reproduces. `DC_REDACT_FUZZ_ITERATIONS`
//! raises the count for a soak run; the default keeps `cargo test` quick.
//! Vendor-shaped values are assembled at run time, so this file is not itself
//! one a push-protection scanner would stop.

use dc_redact::{contains_secret, find_secret, redact_secrets};

/// xorshift64*: deterministic, dependency-free, good enough to pick fragments.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    fn alnum(&mut self, len: usize) -> String {
        const SET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        (0..len).map(|_| *self.pick(SET) as char).collect()
    }
}

/// A vendor-shaped key, prefix joined at run time.
fn vendor_key(rng: &mut Rng) -> String {
    let (head, tail, body) = *rng.pick(&[
        ("AK", "IA", 16),
        ("AS", "IA", 16),
        ("sk", "-", 48),
        ("sk-", "ant-", 40),
        ("sk-", "proj-", 40),
        ("sk_", "live_", 24),
        ("rk_", "live_", 24),
        ("gh", "p_", 36),
        ("gh", "s_", 36),
        ("github", "_pat_", 60),
        ("gl", "pat-", 20),
        ("xo", "xb-", 40),
        ("xa", "pp-", 40),
        ("hf", "_", 34),
        ("np", "m_", 36),
        ("shp", "at_", 32),
        ("AI", "za", 35),
    ]);
    format!("{head}{tail}{}", rng.alnum(body))
}

/// One fragment of a line. Most are the syntax a credential sits in; some are
/// the characters detectors have tripped on before.
fn fragment(rng: &mut Rng, planted: &mut Vec<String>) -> String {
    match rng.below(24) {
        0..=3 => {
            let key = vendor_key(rng);
            planted.push(key.clone());
            key
        }
        4 => rng
            .pick(&[
                "password",
                "api_key",
                "AWS_SECRET_ACCESS_KEY",
                "token",
                "\"secret\"",
                "dbPassword",
                "const ADMIN_PASSWORD: &str",
                "PGPASSWORD",
            ])
            .to_string(),
        5 => rng
            .pick(&[" = ", "=", ": ", ":=", " := ", "\": \"", "=>"])
            .to_string(),
        6 => format!("\"{}\"", rng.alnum(12)),
        7 => format!("'{}9!'", rng.alnum(10)),
        8 => {
            let len = 1 + rng.below(20);
            rng.alnum(len)
        }
        9 => rng
            .pick(&["${", "{{", "}", "}}", "$(", "${API_KEY:?missing}", "<", ">"])
            .to_string(),
        10 => rng
            .pick(&[
                "(", ")", "[", "]", "`", ";", ",", ".", "|", "&&", ":", "/", "+", "=",
            ])
            .to_string(),
        11 => rng.pick(&[" ", "  ", "\t", "\r", "\n", "\r\n"]).to_string(),
        12 => format!("https://user:{}@db.example.com/x", rng.alnum(12)),
        13 => format!("Bearer {}9", rng.alnum(24)),
        14 => "Authorization: Basic dXNlcjpwYXNzd29yZA==".to_string(),
        15 => format!(
            "eyJ{}.eyJ{}.{}",
            rng.alnum(12),
            rng.alnum(14),
            rng.alnum(16)
        ),
        16 => format!("mysql -u root -p{}7", rng.alnum(10)),
        17 => format!("sshpass -p {}3 ssh h", rng.alnum(10)),
        18 => concat!("-----BEGIN RSA ", "PRIVATE KEY-----").to_string(),
        19 => rng
            .pick(&["é", "日本", "\u{2028}", "\u{85}", "🔑", "\u{0}", "ß", "…"])
            .to_string(),
        20 => {
            concat!("https://hooks.slack.com/", "services/T00000000/B00000000/").to_string()
                + &rng.alnum(24)
        }
        21 => "eyJ.".repeat(1 + rng.below(4)),
        22 => format!("SG.{}.{}", rng.alnum(22), rng.alnum(43)),
        _ => rng
            .pick(&["-", "_", ".", "@", "#", "?", "!", "%", "\\", "\""])
            .to_string(),
    }
}

fn iterations() -> usize {
    std::env::var("DC_REDACT_FUZZ_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000)
}

/// The line endings of `text`, in order: what redaction must leave untouched.
fn endings(text: &str) -> Vec<&str> {
    text.split_inclusive('\n')
        .map(|seg| {
            if seg.ends_with("\r\n") {
                "\r\n"
            } else if seg.ends_with('\n') {
                "\n"
            } else {
                ""
            }
        })
        .collect()
}

#[test]
fn redaction_properties_hold_on_random_lines() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut failures = Vec::new();
    let mut detected = 0usize;
    let total = iterations();
    for case in 0..total {
        let parts = 1 + rng.below(12);
        let mut planted = Vec::new();
        let mut text = String::new();
        for _ in 0..parts {
            text.push_str(&fragment(&mut rng, &mut planted));
        }
        let found = contains_secret(&text);
        detected += usize::from(found);
        let out = redact_secrets(&text);
        let mut fail = |why: &str| {
            failures.push(format!(
                "case {case}: {why}\n  in:  {text:?}\n  out: {out:?}"
            ))
        };

        // The gate and the redactor are one rule: a text is changed exactly
        // when it is flagged.
        if found != (out != text) {
            fail("contains_secret disagrees with whether redaction changed the text");
        }
        // Redaction leaves nothing a scan would still stop.
        if contains_secret(&out) {
            fail("the redacted text still contains a secret");
        }
        // Redacting twice is redacting once.
        if redact_secrets(&out) != out {
            fail("redaction is not idempotent");
        }
        // Lines are judged and rewritten one at a time; their endings survive.
        if endings(&text) != endings(&out) {
            fail("redaction changed the line structure");
        }
        // A single line: the first-match API agrees with the any-match one.
        if !text.contains('\n') && find_secret(text.trim_end_matches('\r')).is_some() != found {
            fail("find_secret disagrees with contains_secret on one line");
        }
        // A vendor key standing on its own as a token is never left whole on
        // display. Bounded by what a token is: it must not sit inside a word.
        for key in &planted {
            let Some(at) = text.find(key.as_str()) else {
                continue;
            };
            let before = text[..at].chars().next_back();
            let after = text[at + key.len()..].chars().next();
            let isolated = before.is_none_or(|c| " \t\n\r\"'`([{<=:,;|".contains(c))
                && after.is_none_or(|c| " \t\n\r\"'`)]}>,;|".contains(c));
            if isolated && out.contains(key.as_str()) {
                fail(&format!("isolated vendor key left whole: {key}"));
            }
        }
    }
    // Every kind, counted, then the first example of each: a tally that showed
    // only the first few failures hid every class behind the commonest one.
    let mut kinds: std::collections::BTreeMap<String, (usize, &str)> = Default::default();
    for f in &failures {
        let kind = f
            .split_once(": ")
            .map_or("", |(_, r)| r)
            .lines()
            .next()
            .unwrap_or("");
        let kind = kind.split(": ").next().unwrap_or(kind).to_string();
        kinds.entry(kind).or_insert((0, f)).0 += 1;
    }
    assert!(
        failures.is_empty(),
        "{} of {total} random texts broke a redaction property ({detected} were flagged):\n{}",
        failures.len(),
        kinds
            .iter()
            .map(|(k, (n, first))| format!("{n} x {k}\n{first}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
