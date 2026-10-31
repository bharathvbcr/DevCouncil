import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { packedPackage } from "./npm-runtime-smoke-checks.mjs";

const tarball = (name, paths) => ({ id: `${name}@0.2.5`, name, version: "0.2.5", files: paths.map((p) => ({ path: p })) });

describe("packedPackage", () => {
  it("reads npm 12's object keyed by package name", () => {
    // The shape npm 12.2.1 prints, which the smoke read as the tarball itself.
    const parsed = { devcouncil: tarball("devcouncil", ["LICENSE", "README.md", "bin/devcouncil.js"]) };
    const entry = packedPackage(parsed, "devcouncil");
    assert.deepEqual(
      entry.files.map((f) => f.path),
      ["LICENSE", "README.md", "bin/devcouncil.js"],
    );
  });

  it("reads the array npm 11 and earlier print", () => {
    const entry = packedPackage([tarball("devcouncil", ["bin/devcouncil.js"])], "devcouncil");
    assert.equal(entry.files[0].path, "bin/devcouncil.js");
  });

  it("picks this package out of an array with others in it", () => {
    const entry = packedPackage([tarball("other", ["x.js"]), tarball("devcouncil", ["bin/devcouncil.js"])], "devcouncil");
    assert.equal(entry.name, "devcouncil");
  });

  it("refuses an object keyed by another package instead of reading it", () => {
    assert.throws(() => packedPackage({ other: tarball("other", ["x.js"]) }, "devcouncil"), /keys \["other"\]/);
  });

  it("refuses a bare tarball object, which neither npm shape is", () => {
    assert.throws(() => packedPackage(tarball("devcouncil", ["bin/devcouncil.js"]), "devcouncil"), /expected an array/);
  });

  it("refuses an array with no entry, or two, for this package", () => {
    assert.throws(() => packedPackage([], "devcouncil"), /0 entries/);
    assert.throws(
      () => packedPackage([tarball("devcouncil", []), tarball("devcouncil", [])], "devcouncil"),
      /2 entries/,
    );
  });

  it("refuses an entry without a files list, so a missing list never reads as a missing file", () => {
    assert.throws(() => packedPackage({ devcouncil: { name: "devcouncil" } }, "devcouncil"), /no name\/files list/);
    assert.throws(() => packedPackage({ devcouncil: tarball("other", []) }, "devcouncil"), /no name\/files list/);
  });

  it("refuses non-object output", () => {
    assert.throws(() => packedPackage(null, "devcouncil"), /printed object/);
    assert.throws(() => packedPackage("devcouncil-0.2.5.tgz", "devcouncil"), /printed string/);
  });
});
