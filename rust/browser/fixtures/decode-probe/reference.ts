import { ApiError, createClient, define } from "../../../../src/clientExport.ts";
import { z } from "zod";

const numbers = z.object({ n: z.number(), negative: z.number(), integral: z.number(), nested: z.array(z.number()) });
const observed = numbers.extend({ zero: z.number(), zeros: z.tuple([z.number(), z.object({ zero: z.number() })]), fractional: z.number(), tiny: z.number(), max: z.number() }).passthrough();

export const reference = async (path: string, mode: string): Promise<unknown> => {
  let unauthorized = 0;
  let forbidden = 0;
  const client = createClient(path === "/empty" ? "data:application/json,#" : "", {
    onUnauthorized: () => { unauthorized += 1; },
    onForbidden: () => { forbidden += 1; },
  });
  const def = define<"public">();
  const route = def.get(path, "public", numbers);
  try {
    let value: unknown;
    if (mode === "typed" || mode === "raw-typed") {
      const result = await client(route, {});
      if (!result) throw new Error("Missing numbers");
      value = { n: String(result.n), negative: String(result.negative), integral: String(result.integral), nested: String(result.nested[0]) };
    } else if (mode === "negative-zero") {
      value = Object.is(await client(route, {}, undefined, { validateReturn: false }), -0);
    } else if (mode === "schema") {
      value = await client(def.get(path, "public", z.object({ n: z.number() }).transform((value) => String(value.n))), {});
    } else if (["get-body", "trace", "invalid-header"].includes(mode)) {
      value = await client(def.post(path, "public", z.boolean(), z.unknown()), {}, mode === "invalid-header" ? undefined : false, {
        methodOverride: mode === "trace" ? "TRACE" : "GET",
        authTokenOverride: mode === "invalid-header" ? "bad\r\nvalue" : undefined,
      });
    } else {
      const result = await client(route, {}, undefined, { validateReturn: false });
      const parsed = observed.safeParse(result);
      if (!parsed.success) throw parsed.error;
      const observation = {
        n: String(parsed.data.n), negative: String(parsed.data.negative), integral: String(parsed.data.integral),
        nested: String(parsed.data.nested[0]), negativeZero: Object.is(parsed.data.zero, -0),
        nestedNegativeZero: [Object.is(parsed.data.zeros[0], -0), Object.is(parsed.data.zeros[1].zero, -0)],
        fractional: parsed.data.fractional, tiny: parsed.data.tiny, max: parsed.data.max,
        keys: Object.keys(parsed.data),
      };
      // Unchecked calls retain wire key order before any schema object reconstruction.
      value = typeof result === "object" && result !== null
        ? { ...observation, keys: Object.keys(result) }
        : observation;
    }
    return { outcome: { ok: true, value }, unauthorized, forbidden };
  } catch (error: unknown) {
    if (!(error instanceof ApiError)) throw error;
    return { outcome: { ok: false, error: { status: error.status, kind: error.kind, message: error.message } }, unauthorized, forbidden };
  }
};
