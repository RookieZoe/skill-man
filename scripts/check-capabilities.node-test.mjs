import assert from "node:assert/strict";
import test from "node:test";

import { containsDirectFileSystemAccess } from "./lib/core-boundaries.mjs";

const forbiddenForms = [
  "std::fs::read(path);",
  "std::os::unix::fs::symlink(target, path);",
  "use std::fs;\nfs::read(path);",
  "use std::fs as filesystem;\nfilesystem::read(path);",
  "use std::{fs, path::Path};\nfs::read(path);",
  "use std::os::unix::fs;\nfs::symlink(target, path);",
  "use std::os::unix::{fs, ffi::OsStrExt};\nfs::symlink(target, path);",
  "use std::{os::unix::fs as unix_fs, path::Path};\nunix_fs::symlink(target, path);",
  "use std as platform;\nplatform::fs::read(path);",
  "use ::std as platform;\nplatform::os::unix::fs::symlink(target, path);",
  "extern crate std as platform;\nplatform::fs::read(path);",
  "use std::os as platform;\nplatform::unix::fs::symlink(target, path);",
  "use std::{os, path::Path};\nos::unix::fs::symlink(target, path);",
];

for (const source of forbiddenForms) {
  test(`rejects direct Core filesystem access: ${source.split("\n")[0]}`, () => {
    assert.equal(containsDirectFileSystemAccess(source), true);
  });
}

test("allows Core code that uses the FileSystem seam", () => {
  assert.equal(
    containsDirectFileSystemAccess(
      "use crate::seams::filesystem::FileSystem;\nfilesystem.tree_hash(path);",
    ),
    false,
  );
});

test("allows non-filesystem `use std::` aliases", () => {
  assert.equal(
    containsDirectFileSystemAccess(
      "use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};",
    ),
    false,
  );
});

test("rejects an aliased filesystem module in a brace list", () => {
  assert.equal(
    containsDirectFileSystemAccess(
      "use std::{\n  os::unix::fs as unix_fs,\n  path::Path,\n};\nunix_fs::symlink(target, path);",
    ),
    true,
  );
});
