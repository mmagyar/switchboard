import { keys } from "./util.ts";
import { urlToZodSchema } from "./routeDef.ts";
import { expect, test } from "bun:test";
import { ZodNumber, ZodOptional, ZodString } from "zod";

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
  expect(third.shape.me._def.innerType).toBeInstanceOf(ZodString);
  expect(third.shape.me.isOptional()).toBe(true);
  expect(third.shape.them).toBeInstanceOf(ZodOptional);
  expect(third.shape.them._def.innerType).toBeInstanceOf(ZodString);
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
  expect(withOptional.shape.me_id._def.innerType).toBeInstanceOf(ZodNumber);
  expect(withOptional.shape.me_id.isOptional()).toBe(true);
  expect(withOptional.shape.them).toBeInstanceOf(ZodOptional);
  expect(withOptional.shape.them._def.innerType).toBeInstanceOf(ZodString);
  expect(withOptional.shape.them.isOptional()).toBe(true);

  const withCamelCase = urlToZodSchema("/hello/:you/:andId/:meId?/:them?");
  expect(withCamelCase.shape.you).toBeInstanceOf(ZodString);
  expect(withCamelCase.shape.you.isOptional()).toBe(false);
  expect(withCamelCase.shape.andId).toBeInstanceOf(ZodNumber);
  expect(withCamelCase.shape.andId.isOptional()).toBe(false);
  expect(withCamelCase.shape.meId).toBeInstanceOf(ZodOptional);
  expect(withCamelCase.shape.meId._def.innerType).toBeInstanceOf(ZodNumber);
  expect(withCamelCase.shape.meId.isOptional()).toBe(true);
  expect(withCamelCase.shape.them).toBeInstanceOf(ZodOptional);
});
