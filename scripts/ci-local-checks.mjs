/**
 * Decisions ci-local makes that must be testable without running the whole
 * gate: how a local release tag, a drifted Gusset sibling and the go.mod
 * toolchain map to PASS, FAIL or NOT RUN. A check that could not run is
 * never reported as a pass.
 */
import { spawnSync } from "node:child_process";
import { accessSync, constants } from "node:fs";
import path from "node:path";

/**
 * The tag step has three honest outcomes. `absent` is NOT RUN: with no local
 * tag there is nothing to compare to HEAD, and the owner creates the tag at
 * the release commit. `off-head` is a FAIL, because the tag would publish the
 * notes of an older commit. `at-head` is a PASS.
 *
 * @param {string} root
 * @param {string} version
 * @returns {{ state: "absent" | "at-head" | "off-head" | "unknown", tag: string, tagSha: string, head: string, reason?: string }}
 */
export function localTagState(root, version) {
  const tag = `v${version}`;
  const git = (...args) => spawnSync("git", ["-C", root, ...args], { encoding: "utf8", shell: false });
  const head = git("rev-parse", "HEAD");
  if (head.status !== 0) {
    return { state: "unknown", tag, tagSha: "", head: "", reason: `git rev-parse HEAD failed in ${root}` };
  }
  const headSha = head.stdout.trim();
  const found = git("rev-parse", "-q", "--verify", `refs/tags/${tag}^{commit}`);
  if (found.status === 1) {
    return { state: "absent", tag, tagSha: "", head: headSha };
  }
  if (found.status !== 0) {
    return { state: "unknown", tag, tagSha: "", head: headSha, reason: `git rev-parse refs/tags/${tag} failed` };
  }
  const tagSha = found.stdout.trim();
  return { state: tagSha === headSha ? "at-head" : "off-head", tag, tagSha, head: headSha };
}

/** @param {{ state: string }} s @returns {"PASS" | "FAIL" | "NOT RUN"} */
export function tagVerdict(s) {
  if (s.state === "at-head") return "PASS";
  if (s.state === "absent") return "NOT RUN";
  return "FAIL";
}

/**
 * The 40-hex ref setup-gusset checks out.
 * @param {string} actionYaml
 * @returns {string | undefined}
 */
export function gussetPin(actionYaml) {
  return /^\s*ref:\s*([0-9a-f]{40})\s*$/m.exec(actionYaml)?.[1];
}

/**
 * The sibling must be the pinned revision: the lock file and the Go module
 * both resolve through it, and CI builds the pin. Anything else, including a
 * pin that cannot be read or a sibling HEAD that cannot be read, fails.
 *
 * @param {string | undefined} pin
 * @param {string} siblingHead
 * @returns {{ ok: boolean, message: string }}
 */
export function gussetPinVerdict(pin, siblingHead) {
  if (!pin) {
    return { ok: false, message: "could not read the Gusset pin from .github/actions/setup-gusset/action.yml" };
  }
  if (!siblingHead) {
    return { ok: false, message: "could not read the Gusset sibling's HEAD (is ../gusset a git checkout?)" };
  }
  if (siblingHead !== pin) {
    return {
      ok: false,
      message:
        `Gusset sibling is at ${siblingHead.slice(0, 12)} but CI pins ${pin.slice(0, 12)}. ` +
        `Check out the pin (git -C ../gusset checkout ${pin.slice(0, 12)}) or move the pin ` +
        "in .github/actions/setup-gusset/action.yml in the same change as the code that needs it.",
    };
  }
  return { ok: true, message: `Gusset sibling is at the pin ${pin.slice(0, 12)}` };
}

/**
 * The toolchain go.mod declares, as a GOTOOLCHAIN value. CI's setup-go
 * installs exactly this, so gofmt must be this version's: gofmt output differs
 * between releases, and a newer local gofmt hid a drift CI then failed on.
 *
 * @param {string} goMod
 * @returns {string | undefined}
 */
export function goModToolchain(goMod) {
  const explicit = /^toolchain\s+(go\S+)\s*$/m.exec(goMod)?.[1];
  if (explicit) return explicit;
  const v = /^go\s+(\d+\.\d+(?:\.\d+)?)\s*$/m.exec(goMod)?.[1];
  return v ? `go${v}` : undefined;
}

const ANSI = /\u001b\[[0-9;]*[A-Za-z]/g;
const NESTED_HEADER = /^ {2}── rust\/verify\.sh ─+$/;
const NESTED_ROW = /^ {4}(PASS|FAIL|SKIP|N\/A) {2,3}\S.*$/;
const QUICK_GREEN = "QUICK GATES GREEN; the full gate suite was NOT RUN";

/**
 * A gate's merged output as lines, colour stripped and CRLF and bare CR taken
 * as line ends, so a gate that ends its lines with \r is read like one that
 * ends them with \n. Every matcher below reads these lines.
 *
 * @param {string} log
 * @returns {string[]}
 */
function logLines(log) {
  return log.replace(ANSI, "").split(/\r\n|\r|\n/);
}

/**
 * The libtest lines that say a test failed: `test <name> ... FAILED` and
 * `test result: FAILED. N passed; M failed`. Cargo prints them for every
 * failing target, so a log that holds one is not a green run, whatever status
 * the process that printed it exited with. A passing test that merely has
 * FAILED in its name ends in `... ok` and does not match.
 *
 * @param {string} log
 * @returns {string[]}
 */
export function libtestFailures(log) {
  return logLines(log).filter((l) => /^test .+ \.\.\. FAILED$/.test(l) || /^test result: FAILED\./.test(l));
}

/**
 * Cargo's own failure lines, which libtest never prints because the target
 * did not run: a test target that did not compile, and the closing count of
 * failed targets. rust/verify.sh does not use nextest (checked by grep), so
 * nextest's `FAIL [` lines are not read.
 *
 * @param {string} log
 * @returns {string[]}
 */
export function cargoFailures(log) {
  return logLines(log).filter(
    (l) => /^error: could not compile /.test(l) || /^error: \d+ targets? failed/.test(l) || /^error: test failed, to rerun/.test(l),
  );
}

/**
 * The step labels a verify.sh declares (`L_NAME="label"`, one per step, in the
 * order it records their rows), or undefined when it declares none. The block's
 * rows must be exactly these: the step list is knowable from the script, and a
 * block with rows missing is not the whole run.
 *
 * @param {string} source
 * @returns {string[] | undefined}
 */
export function expectedRowLabels(source) {
  const labels = [...source.matchAll(/^L_[A-Z0-9_]+="([^"]*)"\s*$/gm)].map((m) => m[1]);
  return labels.length > 0 ? labels : undefined;
}

/**
 * Why a nested gate's exit 0 cannot be believed, or undefined when its own
 * output backs it. An exit status is a claim: on 2026-11-17 rust/verify.sh's
 * summary() died and its EXIT trap turned the crash into 0, so the status said
 * green over a log holding "3/8 tests failed (exit 101)".
 *
 * Nested (--continue --nested) the output must hold exactly one summary block:
 * header, at least one well-formed row, a blank line ending it, no FAIL or SKIP row,
 * and, when `labels` is given, one row per declared step in order. Exactly one
 * rather than "judge each": verify.sh prints its block once, at the end, so a
 * second header means the output is not one run's, and no reading of two blocks
 * is more trustworthy than refusing both. Without those flags the run must end
 * with its QUICK GATES GREEN line. In both, the log must hold no libtest FAILED
 * line and no cargo compile or target failure.
 *
 * @param {string} log the gate's merged stdout and stderr
 * @param {{ nested: boolean, labels?: string[] }} opts
 * @returns {string | undefined}
 */
export function nestedGateProblem(log, { nested, labels }) {
  const lines = logLines(log);
  if (nested) {
    const headers = lines.flatMap((l, n) => (NESTED_HEADER.test(l) ? [n] : []));
    if (headers.length === 0) return "exit 0 but no summary block was printed";
    if (headers.length > 1) return `${headers.length} summary blocks were printed; expected exactly one`;
    const at = headers[0];
    if (lines[at + 1] !== "") return "malformed summary block: no blank line under the header";
    let i = at + 2;
    const rows = [];
    while (i < lines.length && NESTED_ROW.test(lines[i])) {
      if (lines[i].trimStart().startsWith("FAIL")) return `exit 0 but the summary block has a FAIL row: ${lines[i].trim()}`;
      // verify.sh exits 2 whenever any row is SKIP, so an exit 0 beside one is not its status.
      if (lines[i].trimStart().startsWith("SKIP")) return `exit 0 but the summary block has a SKIP row: ${lines[i].trim()}`;
      rows.push(lines[i]);
      i++;
    }
    if (rows.length === 0) return "malformed summary block: no rows";
    if (lines[i] !== "" || i + 1 >= lines.length) return `malformed summary block: ${lines[i] === undefined ? "ends inside the rows" : `unexpected line ${JSON.stringify(lines[i])}`}`;
    if (labels) {
      if (rows.length !== labels.length) return `summary block has ${rows.length} row(s) but the script declares ${labels.length} steps`;
      const wrong = labels.findIndex((label, n) => !rows[n].includes(label));
      if (wrong >= 0) return `summary block row ${wrong + 1} is not ${JSON.stringify(labels[wrong])}`;
    }
  } else if (!lines.includes(QUICK_GREEN)) {
    return `exit 0 but the run never printed "${QUICK_GREEN}"`;
  }
  const failed = [...libtestFailures(log), ...cargoFailures(log)];
  if (failed.length > 0) return `exit 0 but the log holds ${failed.length} libtest/cargo failure line(s): ${failed[0]}`;
  return undefined;
}

/**
 * How a step stops short of a pass. The step prints its own message first;
 * the runner decides what the stop means. `cannotRun` marks a step that never
 * started its work (a missing checkout, a command that could not be spawned):
 * the default mode exits with `code` as it always did, and the new modes
 * report it as SKIP, not FAIL.
 */
export class StepStop extends Error {
  /**
   * @param {number} code
   * @param {string} message
   * @param {{ cannotRun?: boolean, note?: string }} [opts]
   */
  constructor(code, message, opts = {}) {
    super(message);
    this.code = code;
    this.cannotRun = opts.cannotRun ?? false;
    this.note = opts.note ?? `exit ${code}`;
  }
}

const USAGE = "usage: ci-local.mjs [--continue] [--only <step>]";

/**
 * @param {string[]} argv
 * @returns {{ mode: "default" | "continue" | "only", only?: string }}
 */
export function parseArgs(argv) {
  let cont = false;
  /** @type {string | undefined} */
  let only;
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--continue") cont = true;
    else if (a === "--only" || a.startsWith("--only=")) {
      const value = a === "--only" ? argv[++i] : a.slice("--only=".length);
      if (value === undefined || value === "") throw new Error(`--only needs a step name\n${USAGE}`);
      if (only !== undefined) throw new Error(`--only given twice\n${USAGE}`);
      only = value;
    } else throw new Error(`unknown argument ${JSON.stringify(a)}\n${USAGE}`);
  }
  if (only !== undefined) return { mode: "only", only };
  return { mode: cont ? "continue" : "default" };
}

/**
 * Labels are unique and every `needs` entry names an earlier step, so a
 * dependency is always decided before the step that needs it.
 *
 * @param {{ label: string, needs: string[] }[]} steps
 */
export function validateSteps(steps) {
  const seen = new Set();
  for (const s of steps) {
    if (seen.has(s.label)) throw new Error(`duplicate step ${JSON.stringify(s.label)}`);
    for (const n of s.needs) {
      if (!seen.has(n)) throw new Error(`step ${JSON.stringify(s.label)} needs ${JSON.stringify(n)}, which is not an earlier step`);
    }
    seen.add(s.label);
  }
}

/**
 * The step and everything it needs, transitively, in step order.
 *
 * @param {{ label: string, needs: string[] }[]} steps
 * @param {string} label
 * @returns {string[]}
 */
export function chainOf(steps, label) {
  const byLabel = new Map(steps.map((s) => [s.label, s]));
  if (!byLabel.has(label)) {
    throw new Error(`no step ${JSON.stringify(label)}; steps are:\n${steps.map((s) => `  ${s.label}`).join("\n")}`);
  }
  const want = new Set();
  const visit = (/** @type {string} */ l) => {
    if (want.has(l)) return;
    want.add(l);
    for (const n of byLabel.get(l)?.needs ?? []) visit(n);
  };
  visit(label);
  return steps.filter((s) => want.has(s.label)).map((s) => s.label);
}

/**
 * `MANVI_MAP_BINARY` (the test judges whether it exists) or an executable
 * `devmap` on PATH: what the selector's end-to-end tests drive.
 *
 * @param {NodeJS.ProcessEnv} env
 * @returns {boolean}
 */
export function devmapAvailable(env) {
  if (env.MANVI_MAP_BINARY) return true;
  const names = process.platform === "win32" ? ["devmap.exe", "devmap"] : ["devmap"];
  for (const dir of (env.PATH ?? "").split(path.delimiter)) {
    if (!dir) continue;
    for (const name of names) {
      try {
        accessSync(path.join(dir, name), constants.X_OK);
        return true;
      } catch {
        // not here
      }
    }
  }
  return false;
}

/**
 * @typedef {"PASS" | "FAIL" | "SKIP" | "N/A"} RowStatus
 * @typedef {{ label: string, status: RowStatus, ms: number, note: string }} Row
 * @typedef {{ env?: Record<string, string>, paths?: Record<string, string>, na?: string } | void} StepOutput
 * @typedef {{ label: string, mode: string, env: NodeJS.ProcessEnv, paths: Record<string, Record<string, string>>, header: () => void }} StepContext
 * @typedef {{ label: string, needs: string[], run: (ctx: StepContext) => StepOutput, requires?: (env: NodeJS.ProcessEnv) => string | undefined }} RunnableStep
 */

/**
 * Run the steps.
 *
 * Dependencies are data the runner enforces. A step's `env` and `paths` go
 * only to the steps that name it in `needs`; nothing writes process.env, so a
 * step that uses an undeclared producer's output fails the first time rather
 * than reading leftovers.
 *
 * - default: stop at the first step that stops, returning its code. The
 *   runner prints nothing of its own; each step prints its header where it
 *   always did.
 * - continue: run every step whose `needs` all passed; the rest are SKIP,
 *   naming the prerequisite. `requires` (an external tool) can also SKIP a
 *   step. Prints the summary block.
 * - only: `continue` over the named step and its `needs` chain.
 *
 * @param {RunnableStep[]} steps
 * @param {{ mode: "default" | "continue" | "only", only?: string, gate: string, baseEnv?: NodeJS.ProcessEnv, log?: (s: string) => void, now?: () => number }} opts
 * @returns {{ code: number, rows: Row[] }}
 */
export function runSteps(steps, opts) {
  const { mode, gate, baseEnv = process.env, log = console.log, now = Date.now } = opts;
  validateSteps(steps);
  const selected = mode === "only" ? new Set(chainOf(steps, opts.only ?? "")) : undefined;
  /** @type {Map<string, { env: Record<string, string>, paths: Record<string, string> }>} */
  const outputs = new Map();
  /** @type {Map<string, RowStatus>} */
  const status = new Map();
  /** @type {Row[]} */
  const rows = [];
  const record = (/** @type {Row} */ row) => {
    rows.push(row);
    status.set(row.label, row.status);
  };
  for (const step of steps) {
    if (selected && !selected.has(step.label)) continue;
    let printed = false;
    const header = () => {
      if (printed) return;
      printed = true;
      log(`\n== ${step.label} ==`);
    };
    if (mode !== "default") {
      const missing = step.needs.find((n) => status.get(n) !== "PASS");
      const reason = missing !== undefined ? `needs ${missing}` : step.requires?.(baseEnv);
      if (reason !== undefined) {
        header();
        log(`SKIP: ${step.label} (not run: ${reason})`);
        record({ label: step.label, status: "SKIP", ms: 0, note: `not run: ${reason}` });
        continue;
      }
      header();
    }
    /** @type {NodeJS.ProcessEnv} */
    const env = { ...baseEnv };
    /** @type {Record<string, Record<string, string>>} */
    const paths = {};
    for (const n of step.needs) {
      const out = outputs.get(n);
      if (!out) continue;
      Object.assign(env, out.env);
      paths[n] = { ...out.paths };
    }
    const started = now();
    try {
      const out = step.run({ label: step.label, mode, env, paths, header }) ?? {};
      const ms = now() - started;
      outputs.set(step.label, { env: out.env ?? {}, paths: out.paths ?? {} });
      if (out.na !== undefined) record({ label: step.label, status: "N/A", ms, note: `not applicable: ${out.na}` });
      else record({ label: step.label, status: "PASS", ms, note: "" });
    } catch (e) {
      if (!(e instanceof StepStop)) throw e;
      if (mode === "default") return { code: e.code, rows };
      const ms = now() - started;
      if (e.cannotRun) {
        record({ label: step.label, status: "SKIP", ms: 0, note: `not run: ${e.message}` });
      } else {
        record({ label: step.label, status: "FAIL", ms, note: e.note });
        log(`✗ ${step.label} failed (${e.note}); continuing so the rest still report`);
      }
    }
  }
  if (mode !== "default") log(summarize(gate, rows));
  return { code: mode === "default" ? 0 : exitCode(rows), rows };
}

/**
 * 1 if any step failed; else 2 if any was skipped; else 0. N/A never changes it.
 *
 * @param {{ status: RowStatus }[]} rows
 */
export function exitCode(rows) {
  if (rows.some((r) => r.status === "FAIL")) return 1;
  if (rows.some((r) => r.status === "SKIP")) return 2;
  return 0;
}

/**
 * The summary block, in GitPulse's ci:local `summarize()` shape. A nested
 * runner (rust/verify.sh inside ci:local) prints its rows indented under a
 * sub-heading and no totals line, so only the outermost runner says how the
 * whole run ended. rust/verify.sh prints the same shape from bash.
 *
 * @param {string} gate
 * @param {Row[]} rows
 * @param {{ nested?: boolean }} [opts]
 */
export function summarize(gate, rows, opts = {}) {
  const ind = opts.nested ? "  " : "";
  const lines = ["", `${ind}${`── ${gate} `.padEnd(60, "─")}`, ""];
  for (const r of rows) {
    const seconds = r.status === "PASS" || r.status === "FAIL" ? `${(r.ms / 1000).toFixed(1)}s` : "";
    const note = r.note ? `  (${r.note})` : "";
    lines.push(`${ind}  ${r.status.padEnd(4)}  ${r.label.padEnd(42)} ${seconds.padStart(7)}${note}`);
  }
  if (!opts.nested) {
    const count = (/** @type {RowStatus} */ s) => rows.filter((r) => r.status === s).length;
    lines.push("");
    lines.push(
      `  ${count("PASS")} passed · ${count("FAIL")} failed · ${count("SKIP")} skipped · ` +
        `${count("N/A")} not applicable of ${rows.length} steps`,
    );
  }
  lines.push("");
  return lines.join("\n");
}
