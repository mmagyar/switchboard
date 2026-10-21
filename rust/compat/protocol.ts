import { z } from "zod";

export const jsonSchema = z.json();
export type Json = z.infer<typeof jsonSchema>;

type UnitSchema = { type: "unknown" | "string" | "number" | "boolean" | "file" };
export type SchemaDsl =
  | UnitSchema
  | { type: "literal"; value: Json }
  | { type: "enum"; value: string[] }
  | { type: "object" | "strictObject"; value: Record<string, SchemaDsl> }
  | { type: "array" | "optional" | "nullable" | "readonly"; value: SchemaDsl }
  | { type: "union"; value: SchemaDsl[] }
  | { type: "default" | "catch"; value: [SchemaDsl, Json] };

export const schemaDslSchema: z.ZodType<SchemaDsl> = z.lazy(() =>
  z.discriminatedUnion("type", [
    z.strictObject({ type: z.enum(["unknown", "string", "number", "boolean", "file"]) }),
    z.strictObject({ type: z.literal("literal"), value: jsonSchema }),
    z.strictObject({ type: z.literal("enum"), value: z.array(z.string()).min(1) }),
    z.strictObject({ type: z.enum(["object", "strictObject"]), value: z.record(z.string(), schemaDslSchema) }),
    z.strictObject({ type: z.enum(["array", "optional", "nullable", "readonly"]), value: schemaDslSchema }),
    z.strictObject({ type: z.literal("union"), value: z.array(schemaDslSchema).min(2) }),
    z.strictObject({ type: z.enum(["default", "catch"]), value: z.tuple([schemaDslSchema, jsonSchema]) }),
  ]),
);

export type DeferredDsl =
  | { type: "ready"; value: Json }
  | { type: "future"; value: DeferredDsl }
  | { type: "object"; value: Record<string, DeferredDsl> }
  | { type: "array"; value: DeferredDsl[] };
export const deferredDslSchema: z.ZodType<DeferredDsl> = z.lazy(() =>
  z.discriminatedUnion("type", [
    z.strictObject({ type: z.literal("ready"), value: jsonSchema }),
    z.strictObject({ type: z.literal("future"), value: deferredDslSchema }),
    z.strictObject({ type: z.literal("object"), value: z.record(z.string(), deferredDslSchema) }),
    z.strictObject({ type: z.literal("array"), value: z.array(deferredDslSchema) }),
  ]),
);

const headersSchema = z.record(z.string(), z.string());
const formatSchema = z.discriminatedUnion("kind", [
  z.strictObject({
    kind: z.literal("constant"),
    text: z.string().optional(),
    status: z.number().int().min(200).max(599).optional(),
    headers: headersSchema.optional(),
    redirect: z.literal(true).optional(),
  }),
  z.strictObject({ kind: z.literal("inspectDeferred") }),
]);
export const routeSchema = z.strictObject({
  method: z.enum(["get", "post", "put", "patch", "delete", "head", "options"]),
  path: z.string(),
  permission: z.string().optional(),
  params: schemaDslSchema.optional(),
  body: schemaDslSchema.optional(),
  output: schemaDslSchema.optional(),
  prefixes: z.array(z.string()).optional(),
  handler: z.discriminatedUnion("kind", [
    z.strictObject({ kind: z.literal("echo") }),
    z.strictObject({ kind: z.literal("value"), value: jsonSchema }),
    z.strictObject({ kind: z.literal("deferred"), value: deferredDslSchema }),
    z.strictObject({
      kind: z.literal("throw"),
      error: z.enum(["request", "notFound", "unauthorized", "generic"]),
      status: z.number().int().min(400).max(599).optional(),
      message: z.string().optional(),
    }),
  ]).optional(),
  format: formatSchema.optional(),
});
export type RouteCase = z.infer<typeof routeSchema>;

export const caseSchema = z.discriminatedUnion("kind", [
  z.strictObject({
    id: z.string().min(1),
    kind: z.literal("http"),
    routes: z.array(routeSchema),
    requests: z.array(z.strictObject({
      method: z.string(),
      url: z.url(),
      headers: headersSchema.optional(),
      body: z.string().optional(),
    })).min(1),
    user: jsonSchema.optional(),
    auth: z.enum(["ok", "forbidden", "unauthenticated"]).optional(),
    errorParser: z.strictObject({ status: z.number().int().min(400).max(599), message: z.string() }).optional(),
    htmlErrors: z.boolean().optional(),
    defaultResponse: z.strictObject({ status: z.number().int().min(200).max(599), text: z.string(), headers: headersSchema.optional() }).optional(),
  }),
  z.strictObject({ id: z.string().min(1), kind: z.literal("definition"), routes: z.array(routeSchema) }),
  z.strictObject({ id: z.string().min(1), kind: z.literal("url"), route: routeSchema, params: jsonSchema }),
]);
export type CompatCase = z.infer<typeof caseSchema>;

export const warningSchema = z.strictObject({
  message: z.string(), issues: z.array(jsonSchema), data: jsonSchema, method: z.string(), url: z.string(),
});
export type Warning = z.infer<typeof warningSchema>;
export const resultSchema = z.discriminatedUnion("kind", [
  z.strictObject({
    id: z.string(), kind: z.literal("http"),
    responses: z.array(z.strictObject({
      status: z.number().int(),
      headers: z.array(z.tuple([z.string(), z.string()])),
      body: z.discriminatedUnion("type", [
        z.strictObject({ type: z.literal("json"), value: jsonSchema }),
        z.strictObject({ type: z.literal("text"), value: z.string() }),
      ]),
    })),
    warnings: z.array(warningSchema), errors: z.array(z.string()),
  }),
  z.strictObject({ id: z.string(), kind: z.literal("definition") }),
  z.strictObject({ id: z.string(), kind: z.literal("url"), url: z.string() }),
  z.strictObject({ id: z.string(), kind: z.literal("error"), error: z.strictObject({ name: z.string(), message: z.string() }) }),
]);
export type CompatResult = z.infer<typeof resultSchema>;

export const parseLines = <T>(text: string, schema: z.ZodType<T>): T[] =>
  text.split("\n").filter((line) => line.trim() !== "").map((line, index) => {
    const parsed = schema.safeParse(JSON.parse(line));
    if (!parsed.success) throw new Error(`Invalid NDJSON record ${index + 1}: ${parsed.error.message}`);
    return parsed.data;
  });
