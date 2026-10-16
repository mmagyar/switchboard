import { z } from "zod";

export const jsonSchema = z.json();
export type Json = z.infer<typeof jsonSchema>;
export const derivedNameSchema = z.enum(["map", "tuple", "open", "tree", "mutual", "containers"]);
export type DerivedName = z.infer<typeof derivedNameSchema>;
export type SchemaDsl =
  | { type: "unknown" | "string" | "number" | "boolean" | "file" }
  | { type: "literal"; value: string | number | boolean | null }
  | { type: "enum"; value: string[] }
  | { type: "object"; value: Record<string, SchemaDsl> }
  | { type: "array" | "record" | "optional" | "nullable" | "readonly"; value: SchemaDsl }
  | { type: "union" | "tuple"; value: SchemaDsl[] }
  | { type: "default" | "catch"; value: [SchemaDsl, Json] }
  | { type: "derived"; value: DerivedName };

export const schemaDslSchema: z.ZodType<SchemaDsl> = z.lazy(() => z.discriminatedUnion("type", [
  z.strictObject({ type: z.enum(["unknown", "string", "number", "boolean", "file"]) }),
  z.strictObject({ type: z.literal("literal"), value: z.union([z.string(), z.number(), z.boolean(), z.null()]) }),
  z.strictObject({ type: z.literal("enum"), value: z.array(z.string()).min(1) }),
  z.strictObject({ type: z.literal("object"), value: z.record(z.string(), schemaDslSchema) }),
  z.strictObject({ type: z.enum(["array", "record", "optional", "nullable", "readonly"]), value: schemaDslSchema }),
  z.strictObject({ type: z.literal("union"), value: z.array(schemaDslSchema).min(2) }),
  z.strictObject({ type: z.literal("tuple"), value: z.array(schemaDslSchema) }),
  z.strictObject({ type: z.enum(["default", "catch"]), value: z.tuple([schemaDslSchema, jsonSchema]) }),
  z.strictObject({ type: z.literal("derived"), value: derivedNameSchema }),
]));

export const caseSchema = z.discriminatedUnion("op", [
  z.strictObject({ op: z.literal("parse"), schema: schemaDslSchema, input: jsonSchema.optional() }),
  z.strictObject({ op: z.enum(["form", "coerce"]), schema: schemaDslSchema, input: z.record(z.string(), jsonSchema) }),
  z.strictObject({ op: z.literal("def"), schema: schemaDslSchema, path: z.string(), input: jsonSchema }),
  z.strictObject({ op: z.literal("url"), schema: schemaDslSchema, path: z.string(), url: z.url() }),
]);
export type SchemaCase = z.infer<typeof caseSchema>;

const issueSchema = z.object({
  code: z.string(),
  path: z.array(z.union([z.string(), z.number().int().nonnegative()])),
  message: z.string(),
}).catchall(jsonSchema);

export const validationResultSchema = z.union([
  z.strictObject({ success: z.literal(true), present: z.literal(true), data: jsonSchema }),
  z.strictObject({ success: z.literal(true), present: z.literal(false) }),
  z.strictObject({ success: z.literal(false), issues: z.array(issueSchema), message: z.string() }),
  z.strictObject({ success: z.literal(false), message: z.string() }),
]);
export const urlResultSchema = z.union([
  z.strictObject({ success: z.literal(true), data: z.string() }),
  z.strictObject({ success: z.literal(false), message: z.string() }),
]);

export const parseLines = <T>(text: string, schema: z.ZodType<T>): T[] => text.trimEnd().split("\n").map((line, index) => {
  const input: unknown = JSON.parse(line);
  const result = schema.safeParse(input);
  if (!result.success) throw new Error(`Line ${index + 1}: ${result.error.message}`);
  return result.data;
});

export const validateOutput = (text: string, cases: readonly SchemaCase[]): string[] => {
  const lines = text.trimEnd().split("\n");
  if (lines.length !== cases.length) throw new Error(`Expected ${cases.length} results, received ${lines.length}`);
  for (const [index, line] of lines.entries()) {
    const input = cases[index];
    if (!input) throw new Error(`Unexpected result ${index}`);
    const schema = input.op === "coerce" ? jsonSchema : input.op === "def" ? urlResultSchema : validationResultSchema;
    const value: unknown = JSON.parse(line);
    const result = schema.safeParse(value);
    if (!result.success) throw new Error(`Result ${index}: ${result.error.message}`);
  }
  return lines;
};
