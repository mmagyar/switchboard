import { chromium } from "playwright-core";
import { strict as assert } from "node:assert";
import { resolve } from "node:path";
import { mkdir } from "node:fs/promises";
import { z } from "zod";

declare global {
  interface Window {
    runDecodeProbe: (path: string, mode: string) => Promise<unknown>;
    runRustDecodeProbe: (path: string, mode: string) => Promise<unknown>;
    runNetworkEmptyProbe: () => Promise<unknown>;
  }
}

const root = resolve(import.meta.dir, "../..");
const artifacts = resolve(root, ".tmp/browser-decode");
await mkdir(artifacts, { recursive: true });
const bundle = await Bun.build({
  entrypoints: [resolve(import.meta.dir, "fixtures/decode-probe/reference.ts")],
  target: "browser",
});
if (!bundle.success || !bundle.outputs[0]) throw new Error(`Reference build failed: ${bundle.logs}`);
const reference = await bundle.outputs[0].text();
const numbers = '{"10":true,"2":false,"n":9007199254740993,"negative":-9007199254740993,"integral":1.0,"nested":[9007199254740995],"zero":-0,"zeros":[-0,{"zero":-0.0}],"fractional":1.25,"tiny":5e-324,"max":1.7976931348623157e308}';
const server = Bun.serve({
  hostname: "127.0.0.1", port: 0,
  fetch(request) {
    const path = new URL(request.url).pathname;
    if (path === "/") return new Response(`<!doctype html><script type="module">
      import init, { probe } from "/generated/probe.js";
      import { reference } from "/reference.js";
      await init();
      const nativeFetch = window.fetch.bind(window);
      let nativeDiagnostic;
      window.fetch = async (...args) => {
        const path = new URL(args[0], location.href).pathname;
        const response = await nativeFetch(...args);
        if (path === "/network-empty") {
          const nativeJson = response.json.bind(response);
          response.json = () => nativeJson().catch(error => {
            nativeDiagnostic = error.message;
            throw error;
          });
        }
        return response;
      };
      window.runRustDecodeProbe = async (path, mode) => JSON.parse(await probe(path, mode));
      window.runNetworkEmptyProbe = async () => {
        nativeDiagnostic = undefined;
        const rust = JSON.parse(await probe("/network-empty", "unchecked"));
        const rustDiagnostic = nativeDiagnostic;
        nativeDiagnostic = undefined;
        const ref = await reference("/network-empty", "unchecked");
        return { rust, reference: ref, rustDiagnostic, referenceDiagnostic: nativeDiagnostic };
      };
      window.runDecodeProbe = async (path, mode) => ({
        rust: JSON.parse(await probe(path, mode)), reference: await reference(path, mode)
      });
      document.title = "Decode probe ready";
    </script>`, { headers: { "Content-Type": "text/html" } });
    if (path === "/reference.js") return new Response(reference, { headers: { "Content-Type": "text/javascript" } });
    if (path === "/generated/probe.js") return new Response(Bun.file(resolve(artifacts, "generated/probe.js")));
    if (path === "/generated/probe_bg.wasm") return new Response(Bun.file(resolve(artifacts, "generated/probe_bg.wasm")), { headers: { "Content-Type": "application/wasm" } });
    const body = path === "/malformed" ? "{" : (path === "/empty" || path === "/network-empty") ? "" : path === "/trailing" ? "[1,]"
      : path === "/wrong-type" ? "false" : path === "/zero" ? "-0" : path === "/nonfinite" ? '{"nested":[1e400]}'
      : path === "/surrogate" ? '"\\ud800"' : path === "/deep" ? `${"[".repeat(140)}0${"]".repeat(140)}` : numbers;
    const status = path === "/unauthorized" ? 401 : path === "/forbidden" ? 403 : 200;
    return new Response(body, { status, headers: { "Content-Type": "application/json" } });
  },
});
const resultSchema = z.object({ rust: z.unknown(), reference: z.unknown() });
const failureSchema = z.object({ outcome: z.object({ ok: z.literal(false), error: z.object({
  kind: z.string(), status: z.number(), message: z.string(),
}) }), unauthorized: z.number(), forbidden: z.number() });
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage();
  page.setDefaultTimeout(5_000);
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  await page.goto(`http://127.0.0.1:${server.port}/`);
  await page.waitForFunction(() => document.title === "Decode probe ready");
  const evidence: unknown[] = [];
  const failures: string[] = [];
  for (const [path, mode] of [
    ["/numbers", "unchecked"], ["/numbers", "typed"], ["/numbers", "raw-typed"], ["/numbers", "schema"],
    ["/zero", "negative-zero"], ["/malformed", "unchecked"], ["/empty", "unchecked"], ["/trailing", "unchecked"],
    ["/unauthorized", "unchecked"], ["/forbidden", "unchecked"],
    ["/numbers", "get-body"], ["/numbers", "trace"], ["/numbers", "invalid-header"],
  ]) {
    if (!path || !mode) throw new Error("Missing probe case");
    const parsed = resultSchema.safeParse(await page.evaluate(async ({ path, mode }) => window.runDecodeProbe(path, mode), { path, mode }));
    if (!parsed.success) throw parsed.error;
    const result = parsed.data;
    evidence.push({ path, mode, ...result });
    try {
      assert.deepEqual(result.rust, result.reference);
      if (["/malformed", "/empty", "/trailing"].includes(path)) {
        const failure = failureSchema.parse(result.rust);
        assert.equal(failure.outcome.error.status, 502);
        assert.equal(failure.outcome.error.kind, "invalid-response");
        assert.match(failure.outcome.error.message, /^Response body is not valid JSON: /);
      }
      if (mode === "typed" || mode === "raw-typed") assert.deepEqual(result.rust, { outcome: { ok: true, value: {
        n: "9007199254740992", negative: "-9007199254740992", integral: "1", nested: "9007199254740996",
      } }, unauthorized: 0, forbidden: 0 });
      console.log(`PASS ${path} ${mode}`);
    } catch (error: unknown) {
      failures.push(`${path} ${mode}: ${error instanceof Error ? error.message : String(error)}`);
      console.log(`FAIL ${path} ${mode}`);
    }
  }
  // Network empty-body diagnostics depend on stream timing. Compare each rejection to its own native message.
  const networkEmpty = resultSchema.extend({ rustDiagnostic: z.string(), referenceDiagnostic: z.string() })
    .parse(await page.evaluate(() => window.runNetworkEmptyProbe()));
  for (const [result, diagnostic] of [[networkEmpty.rust, networkEmpty.rustDiagnostic], [networkEmpty.reference, networkEmpty.referenceDiagnostic]]) {
    const failure = failureSchema.parse(result);
    assert.equal(failure.outcome.error.kind, "invalid-response");
    assert.equal(failure.outcome.error.status, 502);
    assert.equal(failure.outcome.error.message, `Response body is not valid JSON: ${diagnostic}`);
  }
  evidence.push({ path: "/network-empty", ...networkEmpty });
  console.log("PASS /network-empty own native diagnostics");
  for (const mode of ["rust-type", "route-rust-type"]) {
    // Rust-only typed deserialization has no JavaScript output-type counterpart.
    const result: unknown = await page.evaluate(async ({ mode }) => window.runRustDecodeProbe("/wrong-type", mode), { mode });
    evidence.push({ path: "/wrong-type", mode, rust: result });
    if (mode === "rust-type") {
      assert.match(failureSchema.parse(result).outcome.error.message, /^Response body is not valid JSON for the requested Rust type: /);
    } else {
      const parsed = z.object({ outcome: z.object({ ok: z.literal(false), deserialization: z.string() }) }).parse(result);
      assert.match(parsed.outcome.deserialization, /invalid type: boolean `false`/);
    }
    console.log(`PASS /wrong-type ${mode}`);
  }
  for (const [path, diagnostic] of [
    ["/nonfinite", "Response JSON contains a non-finite number"],
    ["/surrogate", "unexpected end of hex escape at line 1 column 8"],
    ["/deep", "recursion limit exceeded at line 1 column 128"],
  ]) {
    if (!path || !diagnostic) throw new Error("Missing boundary case");
    const result: unknown = await page.evaluate(async ({ path }) => window.runRustDecodeProbe(path, "unchecked"), { path });
    const failure = failureSchema.parse(result);
    assert.equal(failure.outcome.error.status, 502);
    assert.equal(failure.outcome.error.kind, "invalid-response");
    assert.equal(failure.outcome.error.message, `Response body is not valid JSON: ${diagnostic}`);
    evidence.push({ path, rust: result });
    console.log(`PASS ${path} shared-parser boundary`);
  }
  assert.deepEqual(pageErrors, []);
  await Bun.write(resolve(artifacts, "decoder-evidence.json"), JSON.stringify({ chromium: browser.version(), evidence, failures, pageErrors }, null, 2));
  assert.deepEqual(failures, []);
} finally {
  await browser.close();
  await server.stop(true);
}
