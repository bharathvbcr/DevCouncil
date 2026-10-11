/**
 * Decisions ci-local makes that must be testable without running the whole
 * gate: how a local release tag, a drifted Gusset sibling and the go.mod
 * toolchain map to PASS, FAIL or NOT RUN. A check that could not run is
 * never reported as a pass.
 */
import { spawnSync } from "node:child_process";

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
