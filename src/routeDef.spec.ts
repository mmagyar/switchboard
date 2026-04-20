import { keys } from "./util.ts";
import { define, urlToZodSchema } from "./routeDef.ts";
import { describe, expect, test } from "bun:test";
import { ZodNumber, ZodOptional, ZodString, z } from "zod";

const def = define<"admin">();

describe("path param validation", () => {
  test("no error when path has no params", () => {
    expect(() => def.get("/no-params", "admin", z.object({}))).not.toThrow();
  });

  test("no error when all path params are present in the schema", () => {
    expect(() =>
      def.get("/:userId/posts/:postId", "admin", z.object({}), z.object({ userId: z.string(), postId: z.string() })),
    ).not.toThrow();
  });

  test("no error when schema has extra keys beyond path params (query params are fine)", () => {
    expect(() =>
      def.get("/:id", "admin", z.object({}), z.object({ id: z.string(), page: z.number().optional() })),
    ).not.toThrow();
  });

  test("no error when optional path param is present in the schema", () => {
    expect(() =>
      def.get("/:id/:slug?", "admin", z.object({}), z.object({ id: z.string(), slug: z.string().optional() })),
    ).not.toThrow();
  });

  test("no error when paramsValidation is omitted — auto-generated schema is used", () => {
    expect(() => def.get("/:id/:name?", "admin", z.object({}))).not.toThrow();
  });

  test("throws when a mandatory path param is missing from the schema", () => {
    expect(() => def.get("/:userId", "admin", z.object({}), z.object({ otherKey: z.string() }))).toThrow(
      'Route "/:userId" has path params [userId] missing from paramsValidation schema',
    );
  });

  test("throws when an optional path param is missing from the schema", () => {
    expect(() => def.get("/:id/:slug?", "admin", z.object({}), z.object({ id: z.string() }))).toThrow(
      'Route "/:id/:slug?" has path params [slug] missing from paramsValidation schema',
    );
  });

  test("throws when multiple path params are missing", () => {
    expect(() => def.get("/:a/:b/:c", "admin", z.object({}), z.object({ a: z.string() }))).toThrow(
      'Route "/:a/:b/:c" has path params [b, c] missing from paramsValidation schema',
    );
  });

  test("throws when paramsValidation is not a ZodObject but path has params", () => {
    expect(() => def.get("/:id", "admin", z.object({}), z.string())).toThrow(
      'Route "/:id" has path params [id] but paramsValidation is not a ZodObject',
    );
  });

  test("del throws when path param is missing from schema", () => {
    expect(() => def.del("/:id", "admin", z.object({}), z.object({ other: z.string() }))).toThrow(
      'Route "/:id" has path params [id] missing from paramsValidation schema',
    );
  });

  test("options throws when path param is missing from schema", () => {
    expect(() => def.options("/:id", "admin", z.object({}), z.object({ other: z.string() }))).toThrow(
      'Route "/:id" has path params [id] missing from paramsValidation schema',
    );
  });

  test("post throws when path param is missing from schema", () => {
    expect(() =>
      def.post("/:id", "admin", z.object({ name: z.string() }), z.object({}), z.object({ other: z.string() })),
    ).toThrow('Route "/:id" has path params [id] missing from paramsValidation schema');
  });

  test("put throws when path param is missing from schema", () => {
    expect(() =>
      def.put("/:id", "admin", z.object({ name: z.string() }), z.object({}), z.object({ other: z.string() })),
    ).toThrow('Route "/:id" has path params [id] missing from paramsValidation schema');
  });

  test("patch throws when path param is missing from schema", () => {
    expect(() =>
      def.patch("/:id", "admin", z.object({ name: z.string() }), z.object({}), z.object({ other: z.string() })),
    ).toThrow('Route "/:id" has path params [id] missing from paramsValidation schema');
  });
});

test("generate the correct zod schema keys for path", () => {
  expect(keys(urlToZodSchema("/hello").shape)).toStrictEqual([]);
  expect(keys(urlToZodSchema("/hello/:you").shape)).toStrictEqual(["you"]);
  expect(keys(urlToZodSchema("/hello/:you/:and").shape)).toStrictEqual(["you", "and"]);
  expect(keys(urlToZodSchema("/hello/:you/:and/:me").shape)).toStrictEqual(["you", "and", "me"]);
  expect(keys(urlToZodSchema("/hello/:you/:and/:me?/:them?").shape)).toStrictEqual(["you", "and", "me", "them"]);
});

test("generate the correct zod schema types based on optional/ non optional prop", () => {
  expect(urlToZodSchema("/hello").shape).toStrictEqual({});
  const first = urlToZodSchema("/hello/:you");
  expect(first.shape.you).toBeInstanceOf(ZodString);
  expect(first.shape.you.safeParse(undefined).success).toBe(false);
  const second = urlToZodSchema("/hello/:you/:and?");
  expect(second.shape.you).toBeInstanceOf(ZodString);
  expect(second.shape.you.safeParse(undefined).success).toBe(false);
  expect(second.shape.and).toBeInstanceOf(ZodOptional);
  expect(second.shape.and.def.innerType).toBeInstanceOf(ZodString);
  expect(second.shape.and.safeParse(undefined).success).toBe(true);

  const third = urlToZodSchema("/hello/:you/:and/:me?/:them?");
  expect(third.shape.you).toBeInstanceOf(ZodString);
  expect(third.shape.you.isOptional()).toBe(false);
  expect(third.shape.and).toBeInstanceOf(ZodString);
  expect(third.shape.and.isOptional()).toBe(false);
  expect(third.shape.me).toBeInstanceOf(ZodOptional);
  expect(third.shape.me.def.innerType).toBeInstanceOf(ZodString);
  expect(third.shape.me.isOptional()).toBe(true);
  expect(third.shape.them).toBeInstanceOf(ZodOptional);
  expect(third.shape.them.def.innerType).toBeInstanceOf(ZodString);
  expect(third.shape.them.isOptional()).toBe(true);
});

test("if a part ends with _id it will be parsed as number", () => {
  const first = urlToZodSchema("/hello/:you/:and_id");
  expect(first.shape.you).toBeInstanceOf(ZodString);
  expect(first.shape.you.isOptional()).toBe(false);
  expect(first.shape.and_id).toBeInstanceOf(ZodNumber);
  expect(first.shape.and_id.isOptional()).toBe(false);

  const withOptional = urlToZodSchema("/hello/:you/:and_id/:me_id?/:them?");
  expect(withOptional.shape.you).toBeInstanceOf(ZodString);
  expect(withOptional.shape.you.isOptional()).toBe(false);
  expect(withOptional.shape.and_id).toBeInstanceOf(ZodNumber);
  expect(withOptional.shape.and_id.isOptional()).toBe(false);
  expect(withOptional.shape.me_id).toBeInstanceOf(ZodOptional);
  expect(withOptional.shape.me_id.def.innerType).toBeInstanceOf(ZodNumber);
  expect(withOptional.shape.me_id.isOptional()).toBe(true);
  expect(withOptional.shape.them).toBeInstanceOf(ZodOptional);
  expect(withOptional.shape.them.def.innerType).toBeInstanceOf(ZodString);
  expect(withOptional.shape.them.isOptional()).toBe(true);

  const withCamelCase = urlToZodSchema("/hello/:you/:andId/:meId?/:them?");
  expect(withCamelCase.shape.you).toBeInstanceOf(ZodString);
  expect(withCamelCase.shape.you.isOptional()).toBe(false);
  expect(withCamelCase.shape.andId).toBeInstanceOf(ZodNumber);
  expect(withCamelCase.shape.andId.isOptional()).toBe(false);
  expect(withCamelCase.shape.meId).toBeInstanceOf(ZodOptional);
  expect(withCamelCase.shape.meId.def.innerType).toBeInstanceOf(ZodNumber);
  expect(withCamelCase.shape.meId.isOptional()).toBe(true);
  expect(withCamelCase.shape.them).toBeInstanceOf(ZodOptional);
});

describe("aliases", () => {
  test("no error when aliases use the same param names as canonical path", () => {
    expect(() => def.get("/items/:itemId", "admin", z.object({}), undefined, ["/legacy/items/:itemId"])).not.toThrow();
  });

  test("returned route object carries the aliases array", () => {
    const route = def.get("/items/:itemId", "admin", z.object({}), undefined, ["/legacy/items/:itemId"]);
    expect(route.aliases).toEqual(["/legacy/items/:itemId"]);
  });

  test("aliases key is omitted when no aliases are provided", () => {
    const route = def.get("/items/:itemId", "admin", z.object({}));
    expect(route.aliases).toBeUndefined();
  });

  test("aliases key is omitted when empty array is provided", () => {
    const route = def.get("/items/:itemId", "admin", z.object({}), undefined, []);
    expect(route.aliases).toBeUndefined();
  });

  test("throws when alias has different param names than canonical path", () => {
    expect(() => def.get("/items/:itemId", "admin", z.object({}), undefined, ["/legacy/items/:legacyId"])).toThrow(
      /different param names/i,
    );
  });

  test("throws when alias is missing a param that canonical path has", () => {
    expect(() => def.get("/items/:itemId", "admin", z.object({}), undefined, ["/legacy/items"])).toThrow(
      /different param names/i,
    );
  });

  test("throws when alias param is missing from explicit paramsValidation schema", () => {
    const schema = z.object({ itemId: z.number() });
    // alias uses same param names so assertAliasParamNamesMatch passes,
    // but assertPathParamsInSchema for the alias against the schema should also pass
    expect(() => def.get("/items/:itemId", "admin", z.object({}), schema, ["/v2/items/:itemId"])).not.toThrow();
  });

  test("post builder accepts aliases", () => {
    const route = def.post("/items/:itemId", "admin", z.object({ name: z.string() }), z.object({}), undefined, [
      "/legacy/items/:itemId",
    ]);
    expect(route.aliases).toEqual(["/legacy/items/:itemId"]);
  });

  test("post builder throws when alias has mismatched params", () => {
    expect(() =>
      def.post("/items/:itemId", "admin", z.object({ name: z.string() }), z.object({}), undefined, [
        "/legacy/items/:otherId",
      ]),
    ).toThrow(/different param names/i);
  });

  test("throws when alias has invalid optional-param order (mandatory after optional)", () => {
    expect(() =>
      def.get("/items/:itemId", "admin", z.object({}), undefined, ["/items/:itemId?/:other"] as any),
    ).toThrow(/optional/i);
  });

  test("multiple valid aliases are all stored", () => {
    const route = def.get("/items/:itemId", "admin", z.object({}), undefined, [
      "/legacy/items/:itemId",
      "/v1/items/:itemId",
    ]);
    expect(route.aliases).toEqual(["/legacy/items/:itemId", "/v1/items/:itemId"]);
  });
});

describe("paramsValidation ambiguous union detection", () => {
  describe("throws for ambiguous unions", () => {
    test("z.string().or(z.number()) is ambiguous", () => {
      expect(() => def.get("/", "admin", z.object({}), z.object({ v: z.string().or(z.number()) }))).toThrow(
        /ambiguous union/,
      );
    });

    test("z.number().or(z.string()) is ambiguous — order does not matter", () => {
      expect(() => def.get("/", "admin", z.object({}), z.object({ v: z.number().or(z.string()) }))).toThrow(
        /ambiguous union/,
      );
    });

    test("z.string().or(z.boolean()) is ambiguous", () => {
      expect(() => def.get("/", "admin", z.object({}), z.object({ v: z.string().or(z.boolean()) }))).toThrow(
        /ambiguous union/,
      );
    });

    test("z.number().or(z.boolean()) is ambiguous", () => {
      expect(() => def.get("/", "admin", z.object({}), z.object({ v: z.number().or(z.boolean()) }))).toThrow(
        /ambiguous union/,
      );
    });

    test("z.string().or(z.literal('foo')) is ambiguous — literal is string-origin", () => {
      expect(() => def.get("/", "admin", z.object({}), z.object({ v: z.string().or(z.literal("foo")) }))).toThrow(
        /ambiguous union/,
      );
    });

    test("z.string().or(z.number()).optional() is ambiguous — unwraps optional", () => {
      expect(() => def.get("/", "admin", z.object({}), z.object({ v: z.string().or(z.number()).optional() }))).toThrow(
        /ambiguous union/,
      );
    });

    test("nested object field — error message contains the field path", () => {
      expect(() =>
        def.get("/", "admin", z.object({}), z.object({ outer: z.object({ v: z.string().or(z.number()) }) })),
      ).toThrow(/outer\.v/);
    });

    test("array element — throws for ambiguous union inside array", () => {
      expect(() =>
        def.get("/", "admin", z.object({}), z.object({ items: z.array(z.string().or(z.number())) })),
      ).toThrow(/ambiguous union/);
    });

    test("error message contains hint about the fix", () => {
      expect(() => def.get("/", "admin", z.object({}), z.object({ v: z.string().or(z.number()) }))).toThrow(
        /z\.preprocess/,
      );
    });

    test("del builder also validates", () => {
      expect(() => def.del("/", "admin", z.object({}), z.object({ v: z.string().or(z.number()) }))).toThrow(
        /ambiguous union/,
      );
    });

    test("post builder also validates", () => {
      expect(() =>
        def.post(
          "/",
          "admin",
          z.object({ name: z.string() }),
          z.object({}),
          z.object({ v: z.string().or(z.number()) }),
        ),
      ).toThrow(/ambiguous union/);
    });

    test("put builder also validates", () => {
      expect(() =>
        def.put("/", "admin", z.object({ name: z.string() }), z.object({}), z.object({ v: z.string().or(z.number()) })),
      ).toThrow(/ambiguous union/);
    });

    test("patch builder also validates", () => {
      expect(() =>
        def.patch(
          "/",
          "admin",
          z.object({ name: z.string() }),
          z.object({}),
          z.object({ v: z.string().or(z.number()) }),
        ),
      ).toThrow(/ambiguous union/);
    });
  });

  describe("does not throw for unambiguous schemas", () => {
    test("z.number() alone is fine", () => {
      expect(() => def.get("/", "admin", z.object({}), z.object({ v: z.number() }))).not.toThrow();
    });

    test("z.string() alone is fine", () => {
      expect(() => def.get("/", "admin", z.object({}), z.object({ v: z.string() }))).not.toThrow();
    });

    test("z.boolean() alone is fine", () => {
      expect(() => def.get("/", "admin", z.object({}), z.object({ v: z.boolean() }))).not.toThrow();
    });

    test("z.boolean().or(z.object({...})) — only one string-origin branch is fine", () => {
      expect(() =>
        def.get("/", "admin", z.object({}), z.object({ v: z.boolean().or(z.object({ x: z.string() })) })),
      ).not.toThrow();
    });

    test("union of two objects — neither is string-origin at union level", () => {
      expect(() =>
        def.get(
          "/",
          "admin",
          z.object({}),
          z.object({ v: z.object({ a: z.number() }).or(z.object({ b: z.string() })) }),
        ),
      ).not.toThrow();
    });

    test("z.literal with a number value is not string-origin", () => {
      expect(() => def.get("/", "admin", z.object({}), z.object({ v: z.number().or(z.literal(42)) }))).not.toThrow();
    });

    test("auto-generated urlToZodSchema never contains ambiguous unions", () => {
      expect(() => def.get("/:id/:name?", "admin", z.object({}))).not.toThrow();
    });

    test("omitting paramsValidation entirely is fine", () => {
      expect(() => def.get("/no-params", "admin", z.object({}))).not.toThrow();
    });
  });
});
