import { describe, expect, test } from "bun:test";
import { caseSchema, parseLines, resultSchema } from "./protocol.ts";
import type { CompatCase, CompatResult } from "./protocol.ts";
import { runCase, toZod } from "./reference.ts";
import { compareResults } from "./check.ts";

const cases = parseLines(await Bun.file(new URL("./cases.ndjson", import.meta.url)).text(), caseSchema);
const findCase = (id: string): CompatCase => {
  const found = cases.find((input) => input.id === id);
  if (!found) throw new Error(`Missing corpus case ${id}`);
  return found;
};
const httpResult = async (id: string) => {
  const result = await runCase(findCase(id));
  if (result.kind !== "http") throw new Error(`Expected HTTP result for ${id}: ${JSON.stringify(result)}`);
  return result;
};
const invokeCheck = async (args: string[]): Promise<{ code: number; stdout: string; stderr: string }> => {
  const process = Bun.spawn([Bun.which("bun") ?? "bun", new URL("./check.ts", import.meta.url).pathname, ...args], {
    stdout: "pipe", stderr: "pipe", timeout: 5_000, env: { ...Bun.env, VERBOSE_ERROR_OUTPUT: "false" },
  });
  const [code, stdout, stderr] = await Promise.all([
    process.exited, new Response(process.stdout).text(), new Response(process.stderr).text(),
  ]);
  return { code, stdout, stderr };
};

describe("compat corpus and reference", () => {
  test("all cases produce validated independent results", async () => {
    expect(cases.length).toBe(94);
    expect(new Set(cases.map((input) => input.id)).size).toBe(cases.length);
    for (const input of cases) {
      const result = resultSchema.parse(await runCase(input));
      expect(result.id).toBe(input.id);
      if (input.kind === "http") expect(result.kind).toBe("http");
    }
  });

  test("adjacent schema tags preserve defaults, catch, stripping and null", () => {
    const schema = toZod({ type: "object", value: {
      page: { type: "default", value: [{ type: "number" }, 5] },
      safe: { type: "catch", value: [{ type: "number" }, 9] },
      nullable: { type: "nullable", value: { type: "string" } },
      optional: { type: "optional", value: { type: "boolean" } },
    } });
    expect(schema.parse({ safe: "wrong", nullable: null, extra: true })).toEqual({ page: 5, safe: 9, nullable: null });
  });

  test("methods retain reference statuses, HEAD bodies, and unsupported errors", async () => {
    for (const [method, status] of [["get", 200], ["post", 201], ["put", 200], ["patch", 200], ["delete", 204], ["options", 200]] as const) {
      expect((await httpResult(`method-${method}`)).responses[0]?.status).toBe(status);
    }
    expect((await httpResult("method-head-fallback")).responses).toEqual([{
      status: 200, headers: [["content-type", "application/json"]], body: { type: "text", value: "" },
    }]);
    expect((await httpResult("method-unsupported-trace")).responses).toEqual([
      { status: 405, headers: [], body: { type: "text", value: "Method Not Allowed: TRACE" } },
      { status: 405, headers: [], body: { type: "text", value: "Method Not Allowed: TRACE" } },
    ]);
  });

  test("prefix form rewriting and auth-before-validation remain observable", async () => {
    expect((await httpResult("prefix-post-form")).responses[0]?.body).toEqual({
      type: "json", value: { params: { itemId: 3 }, body: { amount: 4, active: true }, user: null, prefix: "hu" },
    });
    expect((await httpResult("auth-forbidden")).responses[0]).toEqual({
      status: 403, headers: [["content-type", "application/json"]], body: { type: "json", value: "Forbidden" },
    });
  });

  test("JSON resolves nested futures while formatting observes unresolved values", async () => {
    expect((await httpResult("deferred-nested-json")).responses[0]?.body).toEqual({
      type: "json", value: { title: "title", rows: [{ id: 3 }, [4, 5]], nested: { value: true } },
    });
    expect((await httpResult("deferred-format-unresolved")).responses[0]?.body).toEqual({
      type: "json", value: { pending: ["name", "nested.count"] },
    });
    expect((await httpResult("deferred-format-warning")).warnings.map((warning) => warning.url)).toEqual([
      "http://localhost/items#plain", "http://localhost/items#name",
    ]);
  });

  test("warnings retain exact messages, issues and original invalid output", async () => {
    const result = await httpResult("output-warning");
    const issues = [{ expected: "string", code: "invalid_type", path: ["name"], message: "Invalid input: expected string, received number" }];
    expect(result.warnings).toEqual([{
      message: JSON.stringify(issues, null, 2), issues, data: { name: 4, extra: "retain" }, method: "get", url: "http://localhost/items",
    }]);
    expect(result.responses[0]?.body).toEqual({ type: "json", value: { name: 4, extra: "retain" } });
    expect((await httpResult("output-strip")).responses[0]?.body).toEqual({ type: "json", value: { name: "Ada", nested: { id: 3 } } });
  });

  test("definition failures and URL generation use original exported APIs", async () => {
    expect(await runCase(findCase("definition-duplicate"))).toEqual({
      id: "definition-duplicate", kind: "error", error: { name: "Error", message: "Duplicate route: GET /items" },
    });
    expect(await runCase(findCase("url-canonical-prefix"))).toEqual({ id: "url-canonical-prefix", kind: "url", url: "/items/a" });
    expect(await runCase(findCase("url-query-nested-arrays"))).toEqual({
      id: "url-query-nested-arrays", kind: "url", url: "/items?filters.active=true&filters.min=2&tags.0=a&tags.1=b&rows.0.id=3",
    });
  });

  test("NDJSON boundary rejects unknown fields and invalid recursive schemas", () => {
    expect(() => parseLines('{"id":"bad","kind":"definition","routes":[],"ignored":true}', caseSchema)).toThrow("unrecognized_keys");
    expect(() => parseLines('{"id":"bad","kind":"url","route":{"method":"get","path":"/","params":{"type":"array","value":{"type":"notASchema"}}},"params":{}}', caseSchema)).toThrow("Invalid NDJSON record 1");
    expect(() => parseLines("not JSON", caseSchema)).toThrow();
  });
});

describe("compat differential checker", () => {
  test("object key order is immaterial; validation strings, headers, warnings and types are exact", async () => {
    const original = await httpResult("output-warning");
    expect(compareResults([original], [{ ...original, responses: [...original.responses] }])).toEqual([]);
    expect(compareResults([original], [{ ...original, warnings: original.warnings.map((warning) => ({ ...warning, message: warning.message.replace("received number", "received boolean") })) }])).toHaveLength(1);
    expect(compareResults([original], [{ ...original, warnings: [] }])).toHaveLength(1);
    expect(compareResults([original], [{ ...original, responses: original.responses.map((response) => ({ ...response, headers: [["x-extra", "new"]] })) }])).toHaveLength(1);
    const ordered: CompatResult = { id: "order", kind: "http", responses: [{ status: 200, headers: [], body: { type: "json", value: { a: 1, b: 2 } } }], warnings: [], errors: [] };
    expect(compareResults([ordered], [{ ...ordered, responses: [{ status: 200, headers: [], body: { type: "json", value: { b: 2, a: 1 } } }] }])).toEqual([]);
    expect(compareResults([ordered], [{ ...ordered, responses: [{ status: 200, headers: [], body: { type: "json", value: { a: "1", b: 2 } } }] }])).toHaveLength(1);
    expect(compareResults([original], [])).toHaveLength(1);
  });

  test("CLI executes both processes and reports all shared cases", async () => {
    const result = await invokeCheck([Bun.which("bun") ?? "bun", new URL("./reference.ts", import.meta.url).pathname]);
    expect(result).toEqual({ code: 0, stdout: '{"cases":94,"mismatches":0}\n', stderr: "" });
  });

  test("CLI reports a semantic mismatch with complete validation text", async () => {
    const script = `
      import {runCase} from ${JSON.stringify(new URL("./reference.ts", import.meta.url).href)};
      import {caseSchema,parseLines} from ${JSON.stringify(new URL("./protocol.ts", import.meta.url).href)};
      for (const input of parseLines(await Bun.stdin.text(),caseSchema)) {
        const result = await runCase(input);
        const changed = result.id === "output-warning" && result.kind === "http"
          ? {...result,warnings:result.warnings.map(warning=>({...warning,message:warning.message.replace("received number","received boolean")}))}
          : result;
        console.log(JSON.stringify(changed));
      }
    `;
    const result = await invokeCheck([Bun.which("bun") ?? "bun", "-e", script]);
    expect(result.code).toBe(1);
    expect(result.stdout).toBe('{"cases":94,"mismatches":1}\n');
    expect(result.stderr).toContain("received number");
    expect(result.stderr).toContain("received boolean");
    expect(result.stderr).toContain("Invalid input: expected string");
    expect(result.stderr).toContain('"expected":{');
    expect(result.stderr).toContain('"actual":{');
  });

  test("CLI fails on invalid output, nonzero exit and result-count mismatch", async () => {
    const invalid = await invokeCheck([Bun.which("bun") ?? "bun", "-e", 'await Bun.stdin.text(); console.log("{}");']);
    expect(invalid.code).toBe(1);
    expect(invalid.stderr).toContain("Invalid NDJSON record 1");
    expect(invalid.stdout).toBe('{"cases":94,"mismatches":94,"runnerFailures":1}\n');
    const crash = await invokeCheck([Bun.which("bun") ?? "bun", "-e", "await Bun.stdin.text(); process.exit(7);"]);
    expect(crash.code).toBe(1);
    expect(crash.stderr).toContain("exited 7");
    const missing = await invokeCheck([Bun.which("bun") ?? "bun", "-e", "await Bun.stdin.text();"]);
    expect(missing.code).toBe(1);
    expect(missing.stderr).toContain("Expected 94 results, received 0");
  });
});
