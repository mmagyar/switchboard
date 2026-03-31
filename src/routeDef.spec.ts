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

describe("query param auto-schema from URL definition", () => {
  test("no query params declared — schema has only path params", () => {
    const schema = urlToZodSchema("/items/:id");
    expect(keys(schema.shape)).toStrictEqual(["id"]);
  });

  test("single optional string query param", () => {
    const schema = urlToZodSchema("/items?page");
    expect(keys(schema.shape)).toContain("page");
    expect(schema.shape.page).toBeInstanceOf(ZodOptional);
    expect(schema.shape.page.def.innerType).toBeInstanceOf(ZodString);
    expect(schema.shape.page.isOptional()).toBe(true);
  });

  test("query param with Id suffix becomes optional number", () => {
    const schema = urlToZodSchema("/items?categoryId");
    expect(schema.shape.categoryId).toBeInstanceOf(ZodOptional);
    expect(schema.shape.categoryId.def.innerType).toBeInstanceOf(ZodNumber);
  });

  test("multiple query params", () => {
    const schema = urlToZodSchema("/items?page&limit");
    expect(keys(schema.shape)).toContain("page");
    expect(keys(schema.shape)).toContain("limit");
    expect(schema.shape.page).toBeInstanceOf(ZodOptional);
    expect(schema.shape.limit).toBeInstanceOf(ZodOptional);
  });

  test("path params and query params combined", () => {
    const schema = urlToZodSchema("/items/:categoryId?page&limit");
    expect(keys(schema.shape)).toContain("categoryId");
    expect(keys(schema.shape)).toContain("page");
    expect(keys(schema.shape)).toContain("limit");
    // path param is a mandatory number (Id suffix)
    expect(schema.shape.categoryId).toBeInstanceOf(ZodNumber);
    expect(schema.shape.categoryId.isOptional()).toBe(false);
    // query params are optional strings
    expect(schema.shape.page).toBeInstanceOf(ZodOptional);
    expect(schema.shape.page.def.innerType).toBeInstanceOf(ZodString);
    expect(schema.shape.limit).toBeInstanceOf(ZodOptional);
  });

  test("def.get with declared query params does not throw", () => {
    expect(() => def.get("/items/:id?page&limit", "admin", z.object({}))).not.toThrow();
  });

  test("def.get with declared query params — handler receives typed query params", () => {
    // This is a compile-time check: the schema inferred from the URL must include 'page'.
    const route = def.get("/items/:id?page", "admin", z.object({}));
    const parsed = route.paramsValidation.safeParse({ id: "1", page: "2" });
    expect(parsed.success).toBe(true);
    // page is optional — omitting it should also be valid
    const parsedWithoutPage = route.paramsValidation.safeParse({ id: "1" });
    expect(parsedWithoutPage.success).toBe(true);
  });

  test("key=:placeholder — key becomes the schema key, placeholder determines the type", () => {
    // ?limit=:limitId → schema key is "limit", type is number (Id suffix on placeholder)
    const schema = urlToZodSchema("/items?limit=:limitId");
    expect(keys(schema.shape)).toContain("limit");
    expect(schema.shape.limit).toBeInstanceOf(ZodOptional);
    expect(schema.shape.limit.def.innerType).toBeInstanceOf(ZodNumber);
  });

  test("key=:placeholder — non-Id placeholder gives string type", () => {
    const schema = urlToZodSchema("/items?page=:page");
    expect(schema.shape.page).toBeInstanceOf(ZodOptional);
    expect(schema.shape.page.def.innerType).toBeInstanceOf(ZodString);
  });

  test("mixed path params and key=:placeholder query params", () => {
    const schema = urlToZodSchema("/items/:categoryId?page=:page&limit=:limitId");
    expect(keys(schema.shape)).toStrictEqual(["categoryId", "page", "limit"]);
    expect(schema.shape.categoryId).toBeInstanceOf(ZodNumber); // mandatory path param
    expect(schema.shape.page).toBeInstanceOf(ZodOptional);
    expect(schema.shape.page.def.innerType).toBeInstanceOf(ZodString);
    expect(schema.shape.limit).toBeInstanceOf(ZodOptional);
    expect(schema.shape.limit.def.innerType).toBeInstanceOf(ZodNumber); // placeholder limitId → number
  });

  test("def.get with key=:placeholder syntax does not throw", () => {
    expect(() => def.get("/items/:id?page=:page&limit=:limitId", "admin", z.object({}))).not.toThrow();
  });
});
