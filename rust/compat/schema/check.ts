import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { z } from "zod";
import { generateCorpus } from "./generate.ts";
import { caseSchema, jsonSchema, parseLines, validateOutput } from "./protocol.ts";
import type { SchemaCase } from "./protocol.ts";

const decimal = (token: string): string => {
  const [mantissa = "", exponent = "0"] = token.toLowerCase().split("e");
  const [whole = "", fraction = ""] = mantissa.replace(/^-/, "").split(".");
  const digits = `${whole}${fraction}`.replace(/^0+/, "");
  if (digits === "") return "0";
  const coefficient = digits.replace(/0+$/, "");
  const power = BigInt(exponent) - BigInt(fraction.length) + BigInt(digits.length - coefficient.length);
  return `${token.startsWith("-") ? "-" : ""}${coefficient}e${power}`;
};

// Preserve mathematical JSON numbers beyond JS precision while ignoring object key order.
export const canonicalJson = (text: string): string => {
  jsonSchema.parse(JSON.parse(text));
  const tokens = [...text.matchAll(/"(?:[^"\\]|\\.)*"|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|true|false|null|[{}[\],:]/g)].map((match) => match[0]);
  let index = 0;
  const take = (): string => {
    const token = tokens[index++];
    if (token === undefined) throw new Error("Unexpected end of JSON");
    return token;
  };
  const expect = (token: string): void => { if (take() !== token) throw new Error(`Expected ${token}`); };
  const value = (): string => {
    const token = take();
    if (token === "{") {
      const fields = new Map<string, string>();
      if (tokens[index] !== "}") {
        do {
          const key = z.string().parse(JSON.parse(take()));
          if (fields.has(key)) throw new Error(`Duplicate JSON key ${key}`);
          expect(":");
          fields.set(key, value());
          if (tokens[index] !== ",") break;
          take();
        } while (true);
      }
      expect("}");
      return `{${[...fields].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0).map(([key, item]) => `${JSON.stringify(key)}:${item}`).join(",")}}`;
    }
    if (token === "[") {
      const items: string[] = [];
      if (tokens[index] !== "]") {
        do {
          items.push(value());
          if (tokens[index] !== ",") break;
          take();
        } while (true);
      }
      expect("]");
      return `[${items.join(",")}]`;
    }
    if (token.startsWith('"')) return JSON.stringify(z.string().parse(JSON.parse(token)));
    if (token === "true" || token === "false" || token === "null") return token;
    return `number:${decimal(token)}`;
  };
  const result = value();
  if (index !== tokens.length) throw new Error("Unexpected trailing JSON tokens");
  return result;
};

export const compareResults = (cases: readonly SchemaCase[], expected: readonly string[], actual: readonly string[]): string[] => {
  if (expected.length !== cases.length || actual.length !== cases.length) throw new Error("Result count differs from corpus");
  return cases.flatMap((input, index) => {
    const left = expected[index];
    const right = actual[index];
    if (left === undefined || right === undefined) throw new Error(`Missing result ${index}`);
    return canonicalJson(left) === canonicalJson(right) ? [] : [JSON.stringify({ index, case: input, expected: left, actual: right })];
  });
};

const run = async (command: string[], corpus: string, cases: readonly SchemaCase[], outputPath: string): Promise<string> => {
  const child = Bun.spawn(command, { stdin: "pipe", stdout: "pipe", stderr: "pipe", timeout: 30_000, killSignal: "SIGKILL" });
  const output = Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  let inputError: unknown;
  try {
    child.stdin.write(corpus);
    await child.stdin.end();
  } catch (error) {
    inputError = error;
  }
  const [stdout, stderr, exitCode] = await output;
  await Bun.write(outputPath, stdout);
  await Bun.write(`${outputPath}.stderr.log`, stderr);
  if (stderr !== "") console.error(`${JSON.stringify(command)} stderr:\n${stderr}`);
  if (exitCode !== 0) throw new Error(`${JSON.stringify(command)} exited ${exitCode}${child.signalCode ? ` (${child.signalCode})` : ""}`);
  if (inputError !== undefined) throw new Error(`${JSON.stringify(command)} failed to accept stdin`, { cause: inputError });
  validateOutput(stdout, cases);
  return stdout;
};

export const check = async (directory: string, command: string[]): Promise<number> => {
  if (command.length === 0) throw new Error("A Rust runner command is required");
  const corpus = await generateCorpus();
  const cases = parseLines(corpus, caseSchema);
  await mkdir(directory, { recursive: true });
  await Bun.write(join(directory, "cases.ndjson"), corpus);
  await Promise.all(["reference.ndjson", "rust.ndjson", "reference.ndjson.stderr.log", "rust.ndjson.stderr.log", "mismatches.ndjson"].map((name) => Bun.write(join(directory, name), "")));
  await Bun.write(join(directory, "summary.json"), `${JSON.stringify({ cases: cases.length, status: "running" })}\n`);
  try {
    const reference = await run([process.execPath, new URL("./reference.ts", import.meta.url).pathname], corpus, cases, join(directory, "reference.ndjson"));
    const candidate = await run(command, corpus, cases, join(directory, "rust.ndjson"));
    const mismatches = compareResults(cases, validateOutput(reference, cases), validateOutput(candidate, cases));
    await Bun.write(join(directory, "mismatches.ndjson"), mismatches.join("\n"));
    const summary = JSON.stringify({ cases: cases.length, sourceCases: 3103, extensionCases: cases.length - 3103, mismatches: mismatches.length });
    await Bun.write(join(directory, "summary.json"), `${summary}\n`);
    for (const mismatch of mismatches) console.error(mismatch);
    console.log(summary);
    return mismatches.length === 0 ? 0 : 1;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    await Bun.write(join(directory, "summary.json"), `${JSON.stringify({ cases: cases.length, runnerFailures: 1, error: message })}\n`);
    console.error(message);
    return 1;
  }
};

if (import.meta.main) {
  const args = z.tuple([z.string().min(1), z.string().min(1)]).rest(z.string()).safeParse(process.argv.slice(2));
  if (!args.success) {
    console.error("Usage: bun rust/compat/schema/check.ts <evidence-directory> <Rust runner executable> [arguments...]");
    process.exitCode = 1;
  } else {
    const [directory, executable, ...argsRest] = args.data;
    process.exitCode = await check(directory, [executable, ...argsRest]);
  }
}
