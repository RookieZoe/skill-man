import { expect, test } from "vitest";
import { distributionState } from "./distribution-state";
import { translate } from "../locale/messages";

test("distribution state aggregates records, not runtime health or execution", () => {
  expect(distributionState([])).toBe("not_distributed");
  expect(distributionState([false, false])).toBe("not_distributed");
  expect(distributionState([true])).toBe("distributed");
  expect(distributionState([true, true])).toBe("distributed");
  expect(distributionState([true, false])).toBe("partially_distributed");
});

test("both locales share one distribution vocabulary across status surfaces", () => {
  for (const [locale, distributed, notDistributed, partial] of [
    ["en", "Distributed", "Not distributed", "Partially distributed"],
    ["zh-Hans", "已分发", "未分发", "部分分发"],
  ] as const) {
    expect(translate(locale, "library.distribution.distributed")).toBe(
      distributed,
    );
    expect(translate(locale, "library.distribution.not_distributed")).toBe(
      notDistributed,
    );
    expect(
      translate(locale, "library.distribution.partially_distributed"),
    ).toBe(partial);
    for (const key of [
      "library.filter.enabled",
      "operation.activation.enable_done",
      "enable.global.cellNoOp",
    ] as const) {
      expect(translate(locale, key)).toBe(distributed);
    }
    for (const key of [
      "library.filter.disabled",
      "operation.activation.disable_done",
      "library.activation.state.disabled",
      "evidenceRail.activationNone",
    ] as const) {
      expect(translate(locale, key)).toBe(notDistributed);
    }
    expect(
      translate(locale, "library.activation.state.enabled", { state: "test" }),
    ).toBe(`${distributed} · test`);
  }
});
