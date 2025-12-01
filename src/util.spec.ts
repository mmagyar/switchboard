import { merge, map } from "./util.ts";
import { describe, expect, test } from "bun:test";

describe("Util", () => {
  describe("map", () => {
    test("simple", () => {
      const input = { a: 2, b: 4, c: 6 };
      const output = map(input, (x) => x * 2);
      expect(output).toEqual({ a: 4, b: 8, c: 12 });
    });
    test("remove undefined", () => {
      const input = { a: 2, b: 4, c: 6 };
      const output = map(input, (x) => (x === 2 ? undefined : x * 2), true);
      expect(output).not.toHaveProperty("a");
      expect(output).toEqual({ b: 8, c: 12 });

      const output2 = map(input, (x) => (x === 2 ? undefined : x * 2), false);
      expect(output2).toHaveProperty("a");
      expect(output2).toEqual({ a: undefined, b: 8, c: 12 });
    });
  });
  describe("deepMerge", () => {
    test("simple", () => {
      const target = { a: 1, b: 2 };
      const source = { b: 3, c: 4 };
      const output = merge(target, source);
      expect(output).toEqual({ a: 1, b: 3, c: 4 });
    });
    test("nested", () => {
      const target = { a: 1, b: { x: 2, y: 3 } };
      const source = { b: { y: 4, z: 5 }, c: 6 };
      const output = merge(target, source);
      expect(output).toEqual({ a: 1, b: { x: 2, y: 4, z: 5 }, c: 6 });
    });
    test("merging empty objects does not do anything", () => {
      const target = { a: 1, b: { x: 2, y: 3 } };
      expect(merge(target, {})).toEqual(target);

      const targetWithArray = { a: [1, 2], b: { x: [3, 4] } };
      expect(merge(targetWithArray, {})).toEqual(targetWithArray);
    });
    test("merges arrays correctly", () => {
      const target = { a: [1, 2], b: { x: [3, 4] } };
      const source = { b: { x: [5, 6] }, c: [7, 8] };
      const output = merge(target, source, "merge", "concat");
      expect(output).toEqual({ a: [1, 2], b: { x: [3, 4, 5, 6] }, c: [7, 8] });
    });

    test("merges arrays correctly 2 ", () => {
      const target = {
        arr: [1, 2],
        id: 1,
      };
      const source = {
        arr: [],
      };
      const output = merge(target, source);
      expect(output).toEqual({ arr: [1, 2], id: 1 });
    });

    test("merges empty arrays correctly", () => {
      const target = { a: [1, 2], b: { x: [3, 4] }, z: [] };
      const source = { b: { x: [] }, c: [], z: [6, 7] };
      const output = merge(target, source);
      expect(output).toEqual({ a: [1, 2], b: { x: [3, 4] }, c: [], z: [6, 7] });
    });

    test("merges arrays correctly with arrayReplace", () => {
      const target = { a: [1, 2], b: { x: [3, 4] }, z: [], zs: [6, 7] };
      const source = { b: { x: [5, 6] }, c: [7, 8], z: [9, 10], zs: [] };

      expect(merge(target, source, "merge", "override")).toEqual({
        a: [1, 2],
        b: { x: [5, 6] },
        c: [7, 8],
        z: [9, 10],
        zs: [],
      });

      expect(merge(target, source, "nonEmpty")).toEqual({
        a: [1, 2],
        b: { x: [5, 6] },
        c: [7, 8],
        z: [9, 10],
        zs: [6, 7],
      });
    });
    test("merge objects with nonEmpty correctly", () => {
      const first = { id: 2, arr: [{ name: "joe" }, { name: "jane" }] };
      const second = { id: 1, arr: [] };
      const output = merge(first, second);
      expect(output).toEqual({
        id: 1,
        arr: [{ name: "joe" }, { name: "jane" }],
      });
    });

    test("merge value overrides undefined in objects if nonEmpty is used", () => {
      const first = { id: 2, x: { name: "joe" } };
      const second = { id: 1, x: undefined };
      const output = merge(first, second, "nonEmpty");
      expect(output).toEqual({
        id: 1,
        x: { name: "joe" },
      });
    });

    test("object are merged regardless of strategy", () => {
      const base = {
        emptyObj: {},
        nonEmpty: { a: 2 },
        both: { b: 4 },
      };
      const second = {
        emptyObj: { x: 3 },
        nonEmpty: {},
        both: { c: 5 },
      };
      const expected = { emptyObj: { x: 3 }, nonEmpty: { a: 2 }, both: { b: 4, c: 5 } };
      expect(merge(base, second)).toEqual(expected);
      expect(merge(base, second, "nonEmpty")).toEqual(expected);
    });

    test("another merge test, specific for a failure i observed", () => {
      const base = {
        "arr.0.name": "joe",
        "arr.1.name": "jane",
        id: 1,
        arr: [
          {
            name: "joe",
          },
          {
            name: "jane",
          },
        ],
      };
      const override = {
        arr: [{}, {}],
      };
      expect(merge(base, override)).toEqual(base);
    });
  });
});
