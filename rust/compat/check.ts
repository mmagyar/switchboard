import { caseSchema, parseLines, resultSchema } from "./protocol.ts";
import type { CompatCase, CompatResult, Json } from "./protocol.ts";

const canonical = (value: Json): string => {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key] ?? null)}`).join(",")}}`;
};

export const compareResults = (expected: readonly CompatResult[], actual: readonly CompatResult[]): string[] => {
  const mismatches: string[] = [];
  for (let index = 0; index < Math.max(expected.length, actual.length); index++) {
    const left = expected[index];
    const right = actual[index];
    if (!left || !right || canonical(left) !== canonical(right)) {
      mismatches.push(JSON.stringify({ index, id: left?.id ?? right?.id, expected: left ?? null, actual: right ?? null }));
    }
  }
  return mismatches;
};

const validateResults = (text: string, cases: readonly CompatCase[]): CompatResult[] => {
  const results = parseLines(text, resultSchema);
  if (results.length !== cases.length) throw new Error(`Expected ${cases.length} results, received ${results.length}`);
  for (const [index, result] of results.entries()) {
    const input = cases[index];
    if (!input || result.id !== input.id) throw new Error(`Result ${index} has unexpected id ${result.id}`);
    if (result.kind !== "error" && result.kind !== input.kind) throw new Error(`Case ${input.id}: unexpected result kind ${result.kind}`);
    if (input.kind === "http" && result.kind === "http" && result.responses.length !== input.requests.length) {
      throw new Error(`Case ${input.id}: expected ${input.requests.length} responses, received ${result.responses.length}`);
    }
  }
  return results;
};

const run = async (command: string[], corpus: string, cases: readonly CompatCase[]): Promise<CompatResult[]> => {
  const child = Bun.spawn(command, {
    stdin: "pipe", stdout: "pipe", stderr: "pipe", timeout: 30_000,
    killSignal: "SIGKILL", env: { ...process.env, VERBOSE_ERROR_OUTPUT: "false" },
  });
  child.stdin.write(corpus);
  child.stdin.end();
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  if (stderr !== "") console.error(`${JSON.stringify(command)} stderr:\n${stderr}`);
  if (exitCode !== 0) throw new Error(`${JSON.stringify(command)} exited ${exitCode}${child.signalCode ? ` (${child.signalCode})` : ""}`);
  return validateResults(stdout, cases);
};

export const check = async (command: string[]): Promise<number> => {
  if (command.length === 0) throw new Error("Usage: bun rust/compat/check.ts <Rust runner executable> [arguments...]");
  const corpus = await Bun.file(new URL("./cases.ndjson", import.meta.url)).text();
  const cases = parseLines(corpus, caseSchema);
  if (cases.length === 0 || new Set(cases.map((input) => input.id)).size !== cases.length) throw new Error("Corpus must contain cases with unique ids");
  try {
    const reference = await run([process.execPath, new URL("./reference.ts", import.meta.url).pathname], corpus, cases);
    const candidate = await run(command, corpus, cases);
    const mismatches = compareResults(reference, candidate);
    for (const mismatch of mismatches) console.error(mismatch);
    console.log(JSON.stringify({ cases: cases.length, mismatches: mismatches.length }));
    return mismatches.length === 0 ? 0 : 1;
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    console.log(JSON.stringify({ cases: cases.length, mismatches: cases.length, runnerFailures: 1 }));
    return 1;
  }
};

if (import.meta.main) {
  try {
    process.exitCode = await check(process.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
