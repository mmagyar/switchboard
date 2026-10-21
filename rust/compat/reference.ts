import { z } from "zod";
import { define, RouteHandlerDefiner, Router, RequestError, NotFoundError, Unauthorized } from "../../src/index.ts";
import { defToUrl } from "../../src/clientExport.ts";
import type { FormatOutput, RouteHandlerOptions } from "../../src/index.ts";
import { caseSchema, jsonSchema, parseLines, resultSchema } from "./protocol.ts";
import type { CompatCase, CompatResult, DeferredDsl, Json, RouteCase, SchemaDsl, Warning } from "./protocol.ts";

export const toZod = (schema: SchemaDsl): z.ZodType => {
  switch (schema.type) {
    case "unknown": return z.unknown();
    case "string": return z.string();
    case "number": return z.number();
    case "boolean": return z.boolean();
    case "file": return z.file();
    case "literal": {
      if (schema.value !== null && typeof schema.value === "object") throw new Error("Protocol literal must be a JSON primitive");
      return z.literal(schema.value);
    }
    case "enum": return z.enum(schema.value);
    case "object": return z.object(Object.fromEntries(Object.entries(schema.value).map(([key, value]) => [key, toZod(value)])));
    case "strictObject": return z.strictObject(Object.fromEntries(Object.entries(schema.value).map(([key, value]) => [key, toZod(value)])));
    case "array": return z.array(toZod(schema.value));
    case "union": return z.union(schema.value.map(toZod));
    case "optional": return toZod(schema.value).optional();
    case "nullable": return toZod(schema.value).nullable();
    case "readonly": return toZod(schema.value).readonly();
    case "default": return toZod(schema.value[0]).default(schema.value[1]);
    case "catch": return toZod(schema.value[0]).catch(schema.value[1]);
  }
};

const definition = (route: RouteCase) => {
  const builder = define<string>();
  const output = route.output ? toZod(route.output) : z.unknown();
  const params = route.params ? toZod(route.params) : undefined;
  const body = route.body ? toZod(route.body) : z.unknown();
  const permission = route.permission ?? "public";
  switch (route.method) {
    case "get": return builder.get(route.path, permission, output, params, route.prefixes);
    case "head": return { ...builder.get(route.path, permission, output, params, route.prefixes), method: "head" as const };
    case "delete": return builder.del(route.path, permission, output, params, route.prefixes);
    case "options": return builder.options(route.path, permission, output, params, route.prefixes);
    case "post": return builder.post(route.path, permission, body, output, params, route.prefixes);
    case "put": return builder.put(route.path, permission, body, output, params, route.prefixes);
    case "patch": return builder.patch(route.path, permission, body, output, params, route.prefixes);
  }
};

const materialize = (node: DeferredDsl, futures: Promise<unknown>[]): unknown => {
  switch (node.type) {
    case "ready": return node.value;
    case "object": return Object.fromEntries(Object.entries(node.value).map(([key, value]) => [key, materialize(value, futures)]));
    case "array": return node.value.map((value) => materialize(value, futures));
    case "future": {
      const promise = Promise.resolve().then(() => materialize(node.value, futures));
      futures.push(promise);
      return promise;
    }
  }
};

const pendingPaths = (value: unknown, path = ""): string[] => {
  if (value instanceof Promise) return [path];
  if (value === null || typeof value !== "object") return [];
  return Object.entries(value).flatMap(([key, child]) => pendingPaths(child, path ? `${path}.${key}` : key));
};

const handlerValue = (route: RouteCase, params: unknown, body: unknown, user: Json, prefix: string | undefined, futures: Promise<unknown>[]): unknown => {
  const handler = route.handler;
  if (!handler || handler.kind === "echo") return { params, body: body ?? null, user, prefix: prefix ?? null };
  switch (handler.kind) {
    case "value": return handler.value;
    case "deferred": return materialize(handler.value, futures);
    case "throw": {
      const message = handler.message ?? "fixture error";
      switch (handler.error) {
        case "request": throw new RequestError(handler.status ?? 422, message);
        case "notFound": throw new NotFoundError();
        case "generic": throw new Error(message);
        case "unauthorized": {
          const status = handler.status ?? 401;
          if (status !== 401 && status !== 403) throw new Error("Protocol unauthorized status must be 401 or 403");
          throw new Unauthorized(status, message);
        }
      }
    }
  }
};

const formatter = (route: RouteCase): FormatOutput<z.ZodType, z.ZodType, Json> | undefined => {
  const format = route.format;
  if (!format) return undefined;
  if (format.kind === "inspectDeferred") return (data) => ({
    data: JSON.stringify({ pending: pendingPaths(data) }), headers: new Headers({ "content-type": "application/json" }),
  });
  return () => ({ data: format.text, status: format.status, redirect: format.redirect, headers: new Headers(format.headers) });
};

const logArgument = (value: unknown): Json => {
  if (value instanceof Error) return { name: value.name, message: value.message };
  return jsonSchema.parse(value);
};

const warningData = (value: unknown): Json => {
  const serialized = JSON.stringify(value);
  if (serialized === undefined) throw new Error("Warning data is absent and cannot be encoded by protocol v1");
  return jsonSchema.parse(JSON.parse(serialized));
};

export const runCase = async (input: CompatCase): Promise<CompatResult> => {
  const warnings: Warning[] = [];
  const errors: string[] = [];
  const futures: Promise<unknown>[] = [];
  try {
    const options: RouteHandlerOptions<Json> = {
      outputErrorWarning: (error, data, method, url) => warnings.push({
        message: error.message, issues: z.array(jsonSchema).parse(error.issues), data: warningData(data), method, url,
      }),
      errorLogger: (...args) => errors.push(JSON.stringify(args.map(logArgument))),
      ...(input.kind === "http" && input.errorParser ? { errorParser: async () => input.errorParser } : {}),
      ...(input.kind === "http" && input.htmlErrors ? {
        errorHtmlFormatter: async (status: number, message: string) => `<error status="${status}">${message}</error>`,
      } : {}),
    };
    const bind = RouteHandlerDefiner<Json, string>(
      async () => input.kind === "http" ? input.auth ?? "ok" : "ok",
      async () => input.kind === "http" ? input.user ?? null : null,
      options,
    );
    if (input.kind === "url") {
      const def = definition(input.route);
      const url = defToUrl({
        method: def.method, path: def.path, permissionsNeeded: def.permissionsNeeded,
        paramsValidation: def.paramsValidation, outputValidation: def.outputValidation,
      }, input.params);
      return { id: input.id, kind: "url", url };
    }
    const fallback = input.kind === "http" ? input.defaultResponse : undefined;
    const router = fallback ? new Router(() => new Response(fallback.text, { status: fallback.status, headers: fallback.headers })) : new Router();
    for (const route of input.routes) {
      const def = definition(route);
      const format = formatter(route);
      // Body and non-body overloads retain the original library's argument positions.
      if (def.method === "post" || def.method === "put" || def.method === "patch") {
        router.addRoute(bind(def, (params, body, user, prefix) => handlerValue(route, params, body, user, prefix, futures), format));
      } else {
        router.addRoute(bind(def, (params, user, prefix) => handlerValue(route, params, undefined, user, prefix, futures), format));
      }
    }
    if (input.kind === "definition") return { id: input.id, kind: "definition" };
    const responses = [];
    for (const request of input.requests) {
      const response = await router.handleRequest(new Request(request.url, { method: request.method, headers: request.headers, body: request.body }));
      const text = await response.text();
      const headers = [...response.headers.entries()]
        .filter(([key]) => !["date", "connection", "keep-alive"].includes(key))
        .sort(([leftKey, leftValue], [rightKey, rightValue]) => leftKey < rightKey ? -1 : leftKey > rightKey ? 1 : leftValue < rightValue ? -1 : leftValue > rightValue ? 1 : 0);
      const body = response.headers.get("content-type")?.includes("json") && text !== ""
        ? { type: "json" as const, value: jsonSchema.parse(JSON.parse(text)) }
        : { type: "text" as const, value: text };
      responses.push({ status: response.status, headers, body });
      // Nested fixture futures may add further futures while their parent resolves.
      for (let index = 0; index < futures.length; index++) await futures[index];
      await Promise.resolve();
      await Promise.resolve();
    }
    return resultSchema.parse({ id: input.id, kind: "http", responses, warnings, errors });
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    return { id: input.id, kind: "error", error: { name: error.name, message: error.message } };
  }
};

if (import.meta.main) {
  if (process.env["VERBOSE_ERROR_OUTPUT"] === "true") throw new Error("Run the reference with VERBOSE_ERROR_OUTPUT=false");
  const cases = parseLines(await Bun.stdin.text(), caseSchema);
  for (const input of cases) console.log(JSON.stringify(resultSchema.parse(await runCase(input))));
}
