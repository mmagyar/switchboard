import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { z } from "zod";
import { canonicalJson } from "./check.ts";
import { caseSchema, jsonSchema, validateOutput, validationResultSchema } from "./protocol.ts";
import type { Json, SchemaCase } from "./protocol.ts";
import { runReference } from "./reference.ts";

type Boundary = { readonly name: string; readonly input: SchemaCase; readonly rustLimit?: string };

export const boundaryCases = (): Boundary[] => {
  const emptyObject = { type: "object", value: {} } as const;
  const query = (name: string, key: string, rustLimit?: string): Boundary => ({
    name, input: { op: "url", schema: emptyObject, path: "/", url: `http://example.test/?${key}` }, rustLimit,
  });
  const nodeQuery = (lastIndex: number): string => Array.from({ length: 16 }, (_, index) => `a${index}.${index === 15 ? lastIndex : 4095}=x`).join("&");
  const tree = (depth: number): Json => depth === 0
    ? { name: "leaf", children: [], next: null }
    : { name: "branch", children: [], next: tree(depth - 1) };
  return [
    query("query-index-4095", "a.4095=x"),
    query("query-index-4096", "a.4096=x", "Query array index exceeds 4095"),
    query("query-depth-64", `${Array(64).fill("a").join(".")}=x`),
    query("query-depth-65", `${Array(65).fill("a").join(".")}=x`, "Query key nesting exceeds 64 levels"),
    query("query-nodes-65536", nodeQuery(4063)),
    query("query-nodes-65537", nodeQuery(4064), "Query structure exceeds 65536 nodes"),
    { name: "schema-depth-within-128", input: { op: "parse", schema: { type: "derived", value: "tree" }, input: tree(31) } },
    { name: "schema-depth-over-128", input: { op: "parse", schema: { type: "derived", value: "tree" }, input: tree(32) }, rustLimit: "Schema nesting exceeds 128 levels" },
    { name: "schema-visits-65536", input: { op: "parse", schema: { type: "array", value: { type: "number" } }, input: Array.from({ length: 65_535 }, () => 0) } },
    { name: "schema-visits-65537", input: { op: "parse", schema: { type: "array", value: { type: "number" } }, input: Array.from({ length: 65_536 }, () => 0) }, rustLimit: "Schema validation exceeds 65536 nodes" },
  ];
};

const hasMessage = (value: Json, message: string): boolean => {
  if (Array.isArray(value)) return value.some((item) => hasMessage(item, message));
  if (value !== null && typeof value === "object") return value["message"] === message || Object.values(value).some((item) => hasMessage(item, message));
  return false;
};

const run = async (command: string[], input: string): Promise<{ stdout: string; stderr: string; exitCode: number }> => {
  const child = Bun.spawn(command, { stdin: "pipe", stdout: "pipe", stderr: "pipe", timeout: 5_000, killSignal: "SIGKILL" });
  const output = Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  let inputError: unknown;
  try {
    child.stdin.write(`${input}\n`);
    await child.stdin.end();
  } catch (error) {
    inputError = error;
  }
  const [stdout, stderr, exitCode] = await output;
  if (exitCode === 0 && inputError !== undefined) throw new Error("Runner failed to accept stdin", { cause: inputError });
  return { stdout, stderr, exitCode };
};

export const checkBounds = async (directory: string, command: string[]): Promise<number> => {
  if (command.length === 0) throw new Error("A Rust runner command is required");
  await mkdir(directory, { recursive: true });
  const evidence: string[] = [];
  for (const fixture of boundaryCases()) {
    const input = caseSchema.parse(fixture.input);
    const raw = JSON.stringify(input);
    const reference = JSON.stringify(runReference(input));
    const rust = await run(command, raw);
    if (rust.exitCode !== 0) throw new Error(`${fixture.name}: runner exited ${rust.exitCode}: ${rust.stderr}`);
    const [actual] = validateOutput(rust.stdout, [input]);
    if (actual === undefined) throw new Error(`${fixture.name}: no result`);
    const expectedResult = validationResultSchema.parse(JSON.parse(reference));
    if (!expectedResult.success) throw new Error(`${fixture.name}: original rejected bounded fixture: ${reference}`);
    if (fixture.rustLimit) {
      const result = jsonSchema.parse(JSON.parse(actual));
      if (!hasMessage(result, fixture.rustLimit)) throw new Error(`${fixture.name}: expected exact limit message ${fixture.rustLimit}, received ${actual}`);
    } else if (canonicalJson(reference) !== canonicalJson(actual)) {
      throw new Error(`${fixture.name}: unexpected boundary mismatch: ${reference} != ${actual}`);
    }
    evidence.push(JSON.stringify({ name: fixture.name, input, reference, rust, approvedBoundary: fixture.rustLimit ?? null }));
    console.log(`${fixture.name}: passed`);
  }
  const scalarSchema = z.strictObject({ op: z.literal("parse"), schema: z.strictObject({ type: z.enum(["number", "string"]) }), input: z.unknown() });
  for (const fixture of [
    { name: "finite-json-overflow", raw: '{"op":"parse","schema":{"type":"number"},"input":1e400}', rejection: "number out of range" },
    { name: "finite-json-lone-utf16", raw: '{"op":"parse","schema":{"type":"string"},"input":"\\ud800"}', rejection: "unexpected end of hex escape" },
  ]) {
    const input = scalarSchema.parse(JSON.parse(fixture.raw));
    const referenceInput: SchemaCase = input.schema.type === "number"
      ? { op: "parse", schema: { type: "number" }, input: z.literal(Infinity).parse(input.input) }
      : { op: "parse", schema: { type: "string" }, input: z.string().parse(input.input) };
    const reference = JSON.stringify(runReference(referenceInput));
    const rust = await run(command, fixture.raw);
    if (rust.exitCode === 0 || !rust.stderr.includes(fixture.rejection)) throw new Error(`${fixture.name}: expected finite-JSON rejection ${fixture.rejection}, received ${JSON.stringify(rust)}`);
    evidence.push(JSON.stringify({ name: fixture.name, rawInput: fixture.raw, reference, rust, approvedBoundary: fixture.rejection }));
    console.log(`${fixture.name}: passed`);
  }
  await Bun.write(join(directory, "boundaries.ndjson"), `${evidence.join("\n")}\n`);
  console.log(JSON.stringify({ boundaryCases: evidence.length, failures: 0 }));
  return 0;
};

if (import.meta.main) {
  const args = z.tuple([z.string().min(1), z.string().min(1)]).rest(z.string()).safeParse(process.argv.slice(2));
  if (!args.success) {
    console.error("Usage: bun rust/compat/schema/bounds.ts <evidence-directory> <Rust runner executable> [arguments...]");
    process.exitCode = 1;
  } else {
    const [directory, executable, ...rest] = args.data;
    try { process.exitCode = await checkBounds(directory, [executable, ...rest]); }
    catch (error) { console.error(error instanceof Error ? error.message : String(error)); process.exitCode = 1; }
  }
}
