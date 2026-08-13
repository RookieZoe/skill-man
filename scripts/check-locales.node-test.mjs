import assert from "node:assert/strict";
import test from "node:test";

import { en, zhHans, placeholders } from "./lib/locale-contract.mjs";

test("en.json is the complete baseline", () => {
  assert.ok(
    Object.keys(en).length >= 400,
    "the catalog covers the App Copy surface",
  );
  for (const value of Object.values(en)) {
    assert.equal(typeof value, "string");
  }
});

test("zh-Hans.json carries the identical key set", () => {
  assert.deepEqual(Object.keys(zhHans).sort(), Object.keys(en).sort());
});

test("placeholder names match per key", () => {
  for (const key of Object.keys(en)) {
    assert.deepEqual(
      placeholders(zhHans[key]),
      placeholders(en[key]),
      `placeholder mismatch for ${key}`,
    );
  }
});

test("plural entries come in _one/_other pairs", () => {
  for (const key of Object.keys(en)) {
    if (key.endsWith("_one")) {
      assert.ok(
        key.slice(0, -"_one".length) + "_other" in en,
        `${key} needs _other`,
      );
    }
    if (key.endsWith("_other")) {
      assert.ok(
        key.slice(0, -"_other".length) + "_one" in en,
        `${key} needs _one`,
      );
    }
  }
});

test("zh-Hans plural pairs stay consistent with the single Chinese plural category", () => {
  for (const key of Object.keys(en)) {
    if (key.endsWith("_one")) {
      const other = key.slice(0, -"_one".length) + "_other";
      assert.equal(
        placeholders(zhHans[other]).join(),
        placeholders(zhHans[key]).join(),
        `${key} and ${other} must carry identical params in zh-Hans`,
      );
    }
  }
});

test("technical tokens are not smuggled into translated copy", () => {
  // Paths, URLs and code identifiers stay raw in both locales.
  for (const key of Object.keys(en)) {
    // Placeholder examples (path templates) are technical tokens by design.
    if (key.endsWith("_placeholder")) continue;
    for (const value of [en[key], zhHans[key]]) {
      if (typeof value !== "string") continue;
      assert.ok(
        !/https?:\/\/|~\/|\.sqlite3|\.json\b/.test(value),
        `${key} must not embed technical tokens: ${value}`,
      );
    }
  }
});
