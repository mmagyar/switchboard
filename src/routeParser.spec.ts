import { describe, test, expect } from "bun:test";
import { z } from "zod";
import { parseBooleanFromForm, parseNumberFromForm, parseObjectFromForm } from "./routeParser.ts";
import { merge } from "./util.ts";
describe("can parse strings as numbers when declared in schema", () => {
  test("can parse a number", () => {
    const schema = z.object({
      number: z.number(),
    });
    const input = { number: "123" };
    const result = parseNumberFromForm(schema, input);
    expect(result).toEqual({ number: 123 });
  });
  test("can parse a number with a decimal", () => {
    const schema = z.object({
      number: z.number(),
    });
    const input = { number: "123.45" };
    const result = parseNumberFromForm(schema, input);
    expect(result).toEqual({ number: 123.45 });
  });

  test("can parse number from a schema that is optional", () => {
    const schema = z.object({
      number: z.number().optional(),
    });
    const input = { number: "123" };
    const result = parseNumberFromForm(schema, input);
    expect(result).toEqual({ number: 123 });
  });

  test("can parse numbers from a schema where the whole schema is optional", () => {
    const schema = z
      .object({
        number: z.number(),
      })
      .optional();
    const input = { number: "123" };
    const result = parseNumberFromForm(schema, input);
    expect(result).toEqual({ number: 123 });
  });

  test("can parse in presence of other type arguments", () => {
    const schema = z.object({
      number: z.number(),
      string: z.string(),
    });
    const input = { number: "123", string: "hello" };
    const result = parseNumberFromForm(schema, input);
    expect(result).toEqual({ number: 123 });
  });

  test("arrays of numbers", () => {
    const schema = z.object({
      id: z.number(),
      numbers: z.array(z.number()),
    });
    const input = { id: "1", numbers: ["1", "2", "3"] };
    const result = parseNumberFromForm(schema, input);
    expect(result).toEqual({ id: 1, numbers: [1, 2, 3] });
  });
  test("numbers in nested objects", () => {
    const schema = z.object({
      id: z.number(),
      nested: z.object({
        number: z.number(),
      }),
    });
    const input = { id: "1", nested: { number: "456" } };
    const result = parseNumberFromForm(schema, input);
    expect(result).toEqual({ id: 1, nested: { number: 456 } });
  });
  test("numbers in nested arrays and objects", () => {
    const schema = z.object({
      id: z.number(),
      nested: z.array(
        z.object({
          number: z.number(),
        }),
      ),
    });
    const input = { id: "1", nested: [{ number: "456" }, { number: "789" }] };
    const result = parseNumberFromForm(schema, input);
    expect(result).toEqual({ id: 1, nested: [{ number: 456 }, { number: 789 }] });
  });
  test("numbers in deep nested arrays", () => {
    const schema = z.object({
      id: z.number(),
      nested: z.array(
        z.array(
          z.object({
            number: z.number(),
          }),
        ),
      ),
    });
    const input = { id: "1", nested: [[{ number: "456" }], [{ number: "789" }]] };
    const result = parseNumberFromForm(schema, input);
    expect(result).toEqual({ id: 1, nested: [[{ number: 456 }], [{ number: 789 }]] });
  });

  test("if non of the object values are numbers, returns with undefined", () => {
    const schema = z.object({
      id: z.string(),
    });
    const input = {
      id: "1",
    };
    const result = parseNumberFromForm(schema, input);
    expect(result).toBeUndefined();
  });

  test("non numbers are ignored deep in array of objects, does not return empty object", () => {
    const schema = z.object({
      id: z.number(),
      nested: z.array(
        z.array(
          z.object({
            str: z.string(),
          }),
        ),
      ),
    });
    const input = {
      id: "1",
      nested: [[{ str: "something" }, { str: "abc" }], [{ str: "789" }]],
    };
    const result = parseNumberFromForm(schema, input);
    expect(result).toEqual({
      id: 1,
    });
  });
});
describe("can parse strings as booleans when declared in schema", () => {
  test("can parse a boolean", () => {
    const schema = z.object({
      boolean: z.boolean(),
    });
    const input = { boolean: "true" };
    const result = parseBooleanFromForm(schema, input);
    expect(result).toEqual({ boolean: true });
  });
  test("can parse a boolean form union type", () => {
    const schema = z.object({
      boolean: z.boolean().or(z.number()),
    });
    const input = { boolean: "true" };
    const result = parseBooleanFromForm(schema, input);
    expect(result).toEqual({ boolean: true });
  });

  test("can parse a boolean with a false value", () => {
    const schema = z.object({
      boolean: z.boolean(),
    });
    const input = { boolean: "false" };
    const result = parseBooleanFromForm(schema, input);
    expect(result).toEqual({ boolean: false });
  });
  test("value other than true or false is not parsed", () => {
    const schema = z.object({
      boolean: z.boolean(),
    });
    const input = { boolean: "hello" };
    const result = parseBooleanFromForm(schema, input);
    expect(result).toBeUndefined();
  });
  test("can parse boolean from a schema that is optional", () => {
    const schema = z
      .object({
        boolean: z.boolean().optional(),
      })
      .optional();
    const input = { boolean: "true" };
    const result = parseBooleanFromForm(schema, input);
    expect(result).toEqual({ boolean: true });
  });
  test("can parse booleans in presence of other values in the schema", () => {
    const schema = z.object({
      boolean: z.boolean(),
      string: z.string(),
      number: z.number(),
    });
    const input = { boolean: "true", string: "hello", number: "43" };
    const result = parseBooleanFromForm(schema, input);
    expect(result).toEqual({ boolean: true });
  });

  test("does not mess with arrays that are not boolean", () => {
    const schema = z.object({
      array: z.array(z.number()),
    });
    const input = { array: [1, 2] };
    const result = parseBooleanFromForm(schema, input);
    expect(result).toBeUndefined();
  });

  test("arrays only contain parsed if parsing", () => {
    const schema = z.object({
      array: z.array(z.boolean()),
    });
    const input = { array: ["true", "false"] };
    const result = parseBooleanFromForm(schema, input);
    expect(result).toEqual({ array: [true, false] });
  });
  test("can parse object nested arrays", () => {
    const schema = z.object({
      array: z.array(z.array(z.boolean())),
    });
    const input = { array: [["true", "false"], ["true"]] };
    const result = parseBooleanFromForm(schema, input);
    expect(result).toEqual({ array: [[true, false], [true]] });
  });
  test("can parse object -> array -> object -> array ->array", () => {
    const schema = z.object({
      array: z.array(z.array(z.object({ array: z.array(z.boolean()) }))),
    });
    const input = { array: [[{ array: ["true", "false"] }, { array: ["true"] }]] };
    const result = parseBooleanFromForm(schema, input);
    expect(result).toEqual({ array: [[{ array: [true, false] }, { array: [true] }]] });
  });
});

describe("can parse with dot notation", () => {
  test("can parse object with dot notation", () => {
    const input = { "object.inside.str": "trueX", hey: "hello" };
    const result = parseObjectFromForm(input);
    expect(result).toEqual({ hey: "hello", object: { inside: { str: "trueX" } } });
  });

  test("can parse array with dot notation", () => {
    const input = { "array.0": "hello", "array.1": "world" };
    const result = parseObjectFromForm(input);
    expect(result).toEqual({ array: ["hello", "world"] });
  });

  test("can parse nested array with dot notation", () => {
    const input = {
      "array.0.0": "hello",
      "array.0.1": "world",
      "array.1.0": "hi",
      "array.1.1": "there",
    };
    const result = parseObjectFromForm(input);
    expect(result).toEqual({
      array: [
        ["hello", "world"],
        ["hi", "there"],
      ],
    });
  });

  test("can parse object->array->object", () => {
    const input = {
      "object.array.0.str": "hello",
      "object.array.1.str": "world",
    };
    const result = parseObjectFromForm(input);
    expect(result).toEqual({
      object: {
        array: [{ str: "hello" }, { str: "world" }],
      },
    });
  });
  test("can parse object->array->object->array", () => {
    const input = {
      "object.ao.0.ai.0": "hello",
      "object.ao.0.ai.1": "world",
      "object.ao.1.ai.0": "hi",
      "object.ao.1.ai.1": "there",
    };
    const result = parseObjectFromForm(input);
    expect(result).toEqual({
      object: {
        ao: [{ ai: ["hello", "world"] }, { ai: ["hi", "there"] }],
      },
    });
  });

  test("can parse object->array->object->number", () => {
    //TODO make this just about the parseNumberFromForm
    const schema = z.object({
      object: z
        .object({
          array: z.array(
            z
              .object({
                num: z.number(),
              })
              .or(z.object({ anotherNume: z.number() })),
          ),
        })
        .or(z.array(z.number())),
    });
    const input = {
      "object.array.0.num": "1",
      "object.array.1.num": "2",
    };
    const result1 = parseObjectFromForm(input);
    const numbers = parseNumberFromForm(schema, result1);
    if (!numbers || typeof numbers !== "object") throw "IT should have been an object";
    const result = merge(result1, numbers, "nonEmpty");
    expect(result).toEqual({
      object: {
        array: [{ num: 1 }, { num: 2 }],
      },
    });
  });
  test("dis specific case", () => {
    const input = {
      id: "1",
      "arr.0.name": "hello",
      "arr.1.name": "world",
    };
    const result = parseObjectFromForm(input);
    expect(result).toEqual({
      id: "1",
      arr: [{ name: "hello" }, { name: "world" }],
    } as any);
  });

  test("work with union types", () => {
    const input = {
      "filters.0.field": "current_job",
      "filters.0.all_of.0": "METAL work",
    };
    const result = parseObjectFromForm(input);
    expect(result).toEqual({ filters: [{ field: "current_job", all_of: ["METAL work"] }] });
  });
});
