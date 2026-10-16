import { z } from "zod";
import { parseUrl, parseNumberFromForm, parseBooleanFromForm } from "../../../src/routeParser.ts";
import { defToUrl } from "../../../src/urlUtils.ts";
import { merge } from "../../../src/util.ts";
import { caseSchema, parseLines } from "./protocol.ts";
import type { DerivedName, SchemaCase, SchemaDsl } from "./protocol.ts";

const derivedSchema = (name: DerivedName): z.ZodType => {
  switch (name) {
    case "map": return z.record(z.string(), z.object({ count: z.number(), pairs: z.array(z.tuple([z.string(), z.boolean()])) }));
    case "tuple": return z.tuple([z.string(), z.boolean(), z.number()]);
    case "open": return z.object({ known: z.string() }).catchall(z.number());
    case "containers": return z.object({ map: z.record(z.string(), z.string()), tuple: z.tuple([z.string(), z.string()]) });
    case "tree": {
      const tree: z.ZodType = z.object({
        name: z.string(),
        get children(): z.ZodType { return z.array(tree); },
        get next(): z.ZodType { return z.union([tree, z.literal(null)]).optional(); },
      });
      return tree;
    }
    case "mutual": {
      const a: z.ZodType = z.object({ get b(): z.ZodType { return z.union([b, z.literal(null)]).optional(); } });
      const b: z.ZodType = z.object({ get a(): z.ZodType { return z.array(a); }, count: z.number() });
      return a;
    }
  }
};

export const referenceSchema = (node: SchemaDsl): z.ZodType => {
  switch (node.type) {
    case "unknown": return z.unknown();
    case "string": return z.string();
    case "number": return z.number();
    case "boolean": return z.boolean();
    case "file": return z.file();
    case "literal": return z.literal(node.value);
    case "enum": return z.enum(node.value);
    case "object": return z.object(Object.fromEntries(Object.entries(node.value).map(([key, value]) => [key, referenceSchema(value)])));
    case "array": return z.array(referenceSchema(node.value));
    case "record": return z.record(z.string(), referenceSchema(node.value));
    case "tuple": {
      const [first, ...rest] = node.value.map(referenceSchema);
      return first ? z.tuple([first, ...rest]) : z.tuple([]);
    }
    case "union": return z.union(node.value.map(referenceSchema));
    case "optional": return referenceSchema(node.value).optional();
    case "nullable": return referenceSchema(node.value).nullable();
    case "readonly": return referenceSchema(node.value).readonly();
    case "default": return referenceSchema(node.value[0]).default(node.value[1]);
    case "catch": return referenceSchema(node.value[0]).catch(node.value[1]);
    case "derived": return derivedSchema(node.value);
  }
};

export const runReference = (input: SchemaCase): unknown => {
  const schema = referenceSchema(input.schema);
  try {
    if (input.op === "def") {
      return { success: true, data: defToUrl({ path: input.path, paramsValidation: schema, method: "get", permissionsNeeded: undefined, outputValidation: z.unknown() }, input.input) };
    }
    if (input.op === "url") return validationOutput(parseUrl(new URL(input.url), input.path, schema));
    if (input.op === "parse") return validationOutput(schema.safeParse(input.input));
    const numbers = parseNumberFromForm(schema, input.input);
    const numbered = merge(input.input, typeof numbers === "object" ? numbers : undefined, "nonEmpty");
    const booleans = parseBooleanFromForm(schema, numbered);
    const coerced = merge(numbered, typeof booleans === "object" ? booleans : undefined, "nonEmpty");
    return input.op === "coerce" ? coerced : validationOutput(schema.safeParse(coerced));
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    return { success: false, message: error.message };
  }
};

const validationOutput = (result: z.ZodSafeParseResult<unknown>): unknown => result.success
  ? { success: true, present: result.data !== undefined, ...(result.data !== undefined ? { data: result.data } : {}) }
  : { success: false, issues: result.error.issues, message: result.error.message };

if (import.meta.main) {
  const cases = parseLines(await Bun.stdin.text(), caseSchema);
  for (const input of cases) console.log(JSON.stringify(runReference(input)));
}
