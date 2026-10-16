import { caseSchema, parseLines } from "./protocol.ts";
import type { DerivedName, Json, SchemaCase, SchemaDsl } from "./protocol.ts";

const string: SchemaDsl = { type: "string" };
const number: SchemaDsl = { type: "number" };
const boolean: SchemaDsl = { type: "boolean" };
const object = (value: Record<string, SchemaDsl>): SchemaDsl => ({ type: "object", value });
const derived = (value: DerivedName): SchemaDsl => ({ type: "derived", value });

export const extensionCases = (): SchemaCase[] => {
  const schemas: SchemaDsl[] = [
    { type: "record", value: number },
    { type: "record", value: object({ label: string, enabled: boolean }) },
    { type: "tuple", value: [string, boolean, number] },
    { type: "tuple", value: [] },
    { type: "tuple", value: [{ type: "optional", value: string }] },
    { type: "tuple", value: [{ type: "default", value: [string, "x"] }] },
    { type: "tuple", value: [{ type: "optional", value: string }, { type: "default", value: [number, 2] }] },
    { type: "tuple", value: [string, { type: "optional", value: number }] },
    { type: "tuple", value: [{ type: "array", value: { type: "tuple", value: [boolean, string] } }, { type: "nullable", value: number }] },
    { type: "file" },
    ...(["map", "tuple", "open", "tree", "mutual", "containers"] as const).map(derived),
  ];
  const inputs: Json[] = [
    null, false, 0, "", "2", [], ["a"], ["a", true], ["a", false, 2], ["a", "false", "2"], ["a", true, 2, "extra"],
    [[[true, "a"]], null], [[["true", 2]], "3"], {}, { n: 2, m: 0 }, { n: "2", m: false },
    { a: { label: "A", enabled: true, stripped: 1 }, b: { label: "B", enabled: false } },
    { a: { label: 1, enabled: "true" } },
    { first: { count: 3, pairs: [["a", true]], stripped: 1 }, second: { count: 0, pairs: [] } },
    { first: { count: "3", pairs: [[2, "true"]] } },
    { known: "a", x: 2, y: 0 }, { known: 2, x: "2" }, { x: 2 },
    { name: "root", children: [], next: null, stripped: 1 },
    { name: "root", children: [{ name: "child", children: [], next: null, stripped: 2 }], next: { name: "next", children: [] } },
    { name: "root", children: [{ name: 2, children: "wrong" }], next: false },
    { b: null }, { b: { a: [{ b: null }], count: 2 } }, { b: { a: [{ b: { a: [], count: "2" } }], count: false } },
    { map: { a: "hello world", b: "1" }, tuple: ["yes", "true"] },
    { map: { a: 1 }, tuple: ["yes", false] },
  ];
  const cases: SchemaCase[] = [];
  for (const schema of schemas) {
    cases.push({ op: "parse", schema });
    for (const input of inputs) cases.push({ op: "parse", schema, input });
  }
  const formInputs: Record<string, Json>[] = [
      { record: { n: "2" }, tuple: ["3", "false"] },
      { record: { n: 2 }, tuple: [3, false] },
      { known: "a", x: "2" },
      { map: { a: "hello world", b: "1" }, tuple: ["yes", "true"] },
  ];
  for (const schema of [object({ record: { type: "record", value: number }, tuple: { type: "tuple", value: [number, boolean] } }), derived("open"), derived("containers")]) {
    for (const input of formInputs) {
      cases.push({ op: "form", schema, input }, { op: "coerce", schema, input }, { op: "def", schema, path: "/search", input });
    }
  }
  const recursiveInputs: [DerivedName, Record<string, Json>][] = [
    ["tree", { name: "root", children: [{ name: "child", children: [], next: null }], next: null }],
    ["tree", { name: "root", children: [{ name: 2, children: [] }], next: false }],
    ["mutual", { b: { a: [{ b: { a: [], count: "3" } }], count: "2" } }],
    ["mutual", { b: { a: [{ b: null }], count: "wrong" } }],
  ];
  for (const [name, input] of recursiveInputs) {
    const schema = derived(name);
    cases.push({ op: "form", schema, input }, { op: "coerce", schema, input }, { op: "def", schema, path: "/search", input });
  }
  for (const [schema, query] of [
    [object({ record: { type: "record", value: number }, tuple: { type: "tuple", value: [number, boolean] } }), "record.n=2&tuple.0=3&tuple.1=false"],
    [derived("containers"), "map.a=hello+world&map.b=1&tuple.0=yes&tuple.1=true"],
    [derived("tree"), "name=root&children.0.name=child&children.0.children.0.name=leaf"],
    [derived("open"), "known=a&x=2"],
  ] satisfies [SchemaDsl, string][]) cases.push({ op: "url", schema, path: "/search", url: `http://example.test/search?${query}` });
  return cases.map((input) => caseSchema.parse(input));
};

export const generateCorpus = async (): Promise<string> => {
  const seed = await Bun.file(new URL("./source-cases.ndjson", import.meta.url)).text();
  const source = parseLines(seed, caseSchema);
  if (source.length !== 3103) throw new Error(`Expected 3103 source cases, received ${source.length}`);
  return `${seed.trimEnd()}\n${extensionCases().map((input) => JSON.stringify(input)).join("\n")}\n`;
};

if (import.meta.main) process.stdout.write(await generateCorpus());
