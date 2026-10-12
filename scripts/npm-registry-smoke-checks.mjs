/**
 * Verdicts the registry smoke reports, kept apart from the npm and process
 * plumbing so they are testable. A launcher that cannot reach the host
 * binary has not been exercised: that is NOT RUN, never a pass, and the
 * smoke exits non-zero for it.
 */

/** What bin/devcouncil.js prints when it cannot find the host binary. */
export const MISSING_HOST = /requires the devcouncil binary|devcouncil binary not found/i;

/** Exit codes: 0 PASS, 1 FAIL, 2 NOT RUN (the check could not run). */
export const EXIT = Object.freeze({ PASS: 0, FAIL: 1, "NOT RUN": 2 });

/**
 * One launcher invocation (`devcouncil --help`, `dev version`).
 *
 * @param {string} label
 * @param {{ status: number | null, stdout?: string | null, stderr?: string | null, error?: Error }} result
 * @returns {{ label: string, verdict: "PASS" | "FAIL" | "NOT RUN", detail: string }}
 */
export function classifyLauncher(label, result) {
  const text = `${result.stdout ?? ""}${result.stderr ?? ""}`;
  if (result.error) {
    return { label, verdict: "FAIL", detail: `could not start the launcher: ${result.error.message}` };
  }
  if (result.status === 0) {
    if (MISSING_HOST.test(text)) {
      // A shim that reports the host missing and still exits 0 proved nothing.
      return { label, verdict: "FAIL", detail: `exited 0 but reported the host binary missing\n${text}` };
    }
    if (!text.trim()) {
      return { label, verdict: "FAIL", detail: "exited 0 with empty output" };
    }
    return { label, verdict: "PASS", detail: "" };
  }
  if (result.status !== null && MISSING_HOST.test(text)) {
    return {
      label,
      verdict: "NOT RUN",
      detail: "the launcher found no devcouncil host binary, so the host behind it was not exercised",
    };
  }
  const how = result.status === null ? "was killed by a signal" : `exited ${result.status}`;
  return { label, verdict: "FAIL", detail: `${how}\n${text}` };
}

/**
 * The smoke's overall verdict: any FAIL fails; otherwise any NOT RUN (or no
 * checks at all) is NOT RUN; only all-PASS passes.
 *
 * @param {{ verdict: string }[]} results
 * @returns {"PASS" | "FAIL" | "NOT RUN"}
 */
export function overallVerdict(results) {
  if (results.some((r) => r.verdict === "FAIL")) return "FAIL";
  if (results.length === 0 || results.some((r) => r.verdict !== "PASS")) return "NOT RUN";
  return "PASS";
}

/** @param {string} verdict @returns {number} */
export function exitCodeFor(verdict) {
  return verdict in EXIT ? EXIT[verdict] : EXIT.FAIL;
}
