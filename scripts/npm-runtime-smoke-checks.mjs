/**
 * Parsing for the runtime smoke, kept apart from the npm and process plumbing
 * so it is testable.
 */

/**
 * This package's tarball entry from `npm pack --dry-run --json`.
 *
 * npm 11 and earlier print an array of tarball objects. npm 12 prints one
 * object keyed by package name (`{ "<name>": { name, files, ... } }`, from
 * logTar in npm's lib/utils/tar.js). Reading the npm 12 object as if it were
 * the tarball leaves `files` undefined, and every packed-file assertion then
 * reports the file missing. Any other shape is refused rather than guessed at.
 *
 * @param {unknown} parsed the parsed JSON npm printed
 * @param {string} name the package name from package.json
 * @returns {{ name: string, files: Array<{ path: string }> }}
 */
export function packedPackage(parsed, name) {
  let entry;
  if (Array.isArray(parsed)) {
    const matches = parsed.filter((item) => item && typeof item === "object" && item.name === name);
    if (matches.length !== 1) {
      throw new Error(
        `npm pack --json listed ${matches.length} entries named ${JSON.stringify(name)} ` +
          `(of ${parsed.length}); expected exactly one`,
      );
    }
    entry = matches[0];
  } else if (parsed && typeof parsed === "object" && Object.hasOwn(parsed, name)) {
    entry = /** @type {Record<string, unknown>} */ (parsed)[name];
  } else {
    const shape =
      parsed && typeof parsed === "object" ? `an object with keys ${JSON.stringify(Object.keys(parsed))}` : typeof parsed;
    throw new Error(
      `npm pack --json printed ${shape}; expected an array of tarballs or an object keyed by ${JSON.stringify(name)}`,
    );
  }
  if (!entry || typeof entry !== "object" || entry.name !== name || !Array.isArray(entry.files)) {
    throw new Error(`npm pack --json entry for ${JSON.stringify(name)} has no name/files list: ${JSON.stringify(entry)}`);
  }
  return entry;
}
