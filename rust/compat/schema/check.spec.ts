import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { join } from "node:path";
import { z } from "zod";
import { canonicalJson, compareResults, check } from "./check.ts";
import { extensionCases, generateCorpus } from "./generate.ts";
import { caseSchema, parseLines, validateOutput } from "./protocol.ts";
import { runReference } from "./reference.ts";

const input = caseSchema.parse({ op: "parse", schema: { type: "string" }, input: null });

describe("schema strict differential gate", () => {
  test("preserves all 3103 scratch cases and generates validated portable extensions deterministically", async () => {
    const corpus = await generateCorpus();
    const cases = parseLines(corpus, caseSchema);
    expect(cases).toHaveLength(3741);
    expect(extensionCases()).toHaveLength(638);
    expect(await generateCorpus()).toBe(corpus);
    expect(cases[0]).toEqual(input);
    expect(cases[3102]).toEqual({ op: "form", schema: { type: "object", value: { n: { type: "number" } } }, input: { n: "1000000000000000128" } });
  });

  test("compares full nested issues, exact messages, URLs, missing values and array order", () => {
    const expected = JSON.stringify(runReference(input));
    expect(compareResults([input], [expected], [expected])).toEqual([]);
    for (const mutated of [
      expected.replace("received null", "received number"),
      expected.replace('"path":[]', '"path":[0]'),
      expected.replace('"issues":[', '"issues":[{"code":"custom","path":[],"message":"extra"},'),
      '{"success":true,"present":false}',
    ]) expect(compareResults([input], [expected], [mutated])).toHaveLength(1);
    expect(canonicalJson('{"url":"/x?q=a+b"}')).not.toBe(canonicalJson('{"url":"/x?q=a%20b"}'));
    expect(canonicalJson('{"a":1}')).not.toBe(canonicalJson('{"a":1,"b":null}'));
    expect(canonicalJson('[1,2]')).not.toBe(canonicalJson('[2,1]'));
  });

  test("retains lossless decimal differences without confusing equivalent JSON numeric notation", () => {
    expect(canonicalJson('{"n":1000000000000000128}')).not.toBe(canonicalJson('{"n":1000000000000000100}'));
    expect(canonicalJson('{"n":1e-300}')).not.toBe(canonicalJson('{"n":1.0000000000000001e-300}'));
    expect(canonicalJson('{"b":-0.0,"a":12300e-2}')).toBe(canonicalJson('{"a":123,"b":0}'));
    expect(canonicalJson('{"n":1.50e+2}')).toBe(canonicalJson('{"n":150}'));
    expect(canonicalJson('"number:1e0"')).not.toBe(canonicalJson('1'));
    expect(() => canonicalJson('{"n":1,"n":2}')).toThrow("Duplicate JSON key n");
  });

  test("rejects unvalidated input, invalid output envelopes and missing results", () => {
    expect(() => parseLines('{"op":"parse","schema":{"type":"mystery"}}', caseSchema)).toThrow("Line 1");
    expect(() => parseLines('{"op":"parse","schema":{"type":"string"},"unknown":1}', caseSchema)).toThrow("Line 1");
    expect(() => validateOutput('{"success":true,"present":true}', [input])).toThrow("Result 0");
    expect(() => validateOutput('{"success":false,"issues":[{}],"message":"x"}', [input])).toThrow("Result 0");
    expect(() => validateOutput('{}\n{}', [input])).toThrow("Expected 1 results, received 2");
    expect(() => compareResults([input], [], [])).toThrow("Result count differs from corpus");
  });

  test("integration: CLI pipeline persists complete validated artifacts and executes both processes", async () => {
    await mkdir(".tmp", { recursive: true });
    const directory = await mkdtemp(join(".tmp", "schema-check-test-"));
    try {
      expect(await check(directory, [process.execPath, new URL("./reference.ts", import.meta.url).pathname])).toBe(0);
      const raw: unknown = JSON.parse(await Bun.file(join(directory, "summary.json")).text());
      const summary = z.strictObject({ cases: z.literal(3741), sourceCases: z.literal(3103), extensionCases: z.literal(638), mismatches: z.literal(0) }).parse(raw);
      expect(summary.mismatches).toBe(0);
      expect(await Bun.file(join(directory, "mismatches.ndjson")).text()).toBe("");
      const cases = parseLines(await Bun.file(join(directory, "cases.ndjson")).text(), caseSchema);
      expect(validateOutput(await Bun.file(join(directory, "reference.ndjson")).text(), cases)).toHaveLength(3741);
      expect(validateOutput(await Bun.file(join(directory, "rust.ndjson")).text(), cases)).toHaveLength(3741);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("integration: process failure, invalid JSON and wrong result counts cannot pass", async () => {
    await mkdir(".tmp", { recursive: true });
    const directory = await mkdtemp(join(".tmp", "schema-check-failure-"));
    try {
      const commands = [
        { script: "process.exit(7)", error: "exited 7" },
        { script: "await Bun.stdin.text(); console.log('{}')", error: "Expected 3741 results, received 1" },
        { script: "await Bun.stdin.text(); console.log(Array(3741).fill('not-json').join('\\n'))", error: "JSON Parse error" },
        { script: `await Bun.stdin.text(); console.log(Array(3741).fill('{"success":true,"present":true}').join('\\n'))`, error: "Result 0" },
      ];
      for (const { script, error } of commands) {
        expect(await check(directory, [process.execPath, "-e", script])).toBe(1);
        const raw: unknown = JSON.parse(await Bun.file(join(directory, "summary.json")).text());
        const summary = z.strictObject({ cases: z.literal(3741), runnerFailures: z.literal(1), error: z.string().min(1) }).parse(raw);
        expect(summary.runnerFailures).toBe(1);
        expect(summary.error).toContain(error);
      }
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });
});
