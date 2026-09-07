import { expect, test } from "vitest";
import { selectLibrarySkill } from "./library-selection";

const ids = ["a", "b", "c", "d", "e"];
const plain = { shiftKey: false, ctrlKey: false, metaKey: false };
const shift = { ...plain, shiftKey: true };

test("plain click replaces selection and Command/Ctrl toggle by stable ID", () => {
  expect(selectLibrarySkill(["a", "b"], "b", "c", ids, plain)).toEqual({
    ids: ["c"],
    anchor: "c",
  });
  for (const modifier of ["ctrlKey", "metaKey"] as const) {
    const add = selectLibrarySkill(["a"], "a", "c", ids, {
      ...plain,
      [modifier]: true,
    });
    expect(add).toEqual({ ids: ["a", "c"], anchor: "c" });
    expect(
      selectLibrarySkill(add.ids, add.anchor, "c", ids, {
        ...plain,
        [modifier]: true,
      }),
    ).toEqual({ ids: ["a"], anchor: "a" });
    expect(
      selectLibrarySkill(["a"], "a", "a", ids, { ...plain, [modifier]: true }),
    ).toEqual({ ids: [], anchor: null });
  }
});

test("Shift starts at the first visible Skill when selection is empty", () => {
  expect(selectLibrarySkill([], "e", "c", ids, shift)).toEqual({
    ids: ["a", "b", "c"],
    anchor: "a",
  });
});

test("Shift expands and shrinks in both directions from a stable selected anchor", () => {
  const down = selectLibrarySkill(["c"], "c", "e", ids, shift);
  expect(down.ids).toEqual(["c", "d", "e"]);
  const up = selectLibrarySkill(down.ids, down.anchor, "a", ids, shift);
  expect(up).toEqual({ ids: ["a", "b", "c"], anchor: "c" });
  expect(selectLibrarySkill(up.ids, up.anchor, "b", ids, shift).ids).toEqual([
    "b",
    "c",
  ]);
});

test("hidden anchors fall back to the first visible selected row, not catalog order", () => {
  expect(
    selectLibrarySkill(
      ["hidden", "d"],
      "hidden",
      "b",
      ["e", "d", "c", "b"],
      shift,
    ).ids,
  ).toEqual(["d", "c", "b"]);
  expect(selectLibrarySkill(["hidden"], "hidden", "c", ids, shift).ids).toEqual(
    ["a", "b", "c"],
  );
});
