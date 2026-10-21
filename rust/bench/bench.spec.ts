import { describe, expect, test } from "bun:test";
import { createReferenceRouter } from "./reference.ts";
import { fixtures, parseLoadResult, workloads } from "./workloads.ts";
import { parseCpuTicks, parseMemory } from "./resources.ts";
import { parseOptions, validatePhase } from "./run.ts";
import { startServer } from "./processes.ts";

const loadResult = {
  workload: "static", durationMs: 10, concurrency: 2, timeoutMs: 100,
  elapsedSeconds: 0.012, completed: 20, transportErrors: 0, mismatches: 0,
  requestsPerSecond: 20 / 0.012,
  latency: { p50Ms: 0.1, p95Ms: 0.2, p99Ms: 0.3, histogramRelativeError: 0.01 }, errors: [],
};

describe("benchmark reference full HTTP pipeline", () => {
  test("all workloads match exact status, content type and bytes", async () => {
    const router = createReferenceRouter();
    const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: (request) => router.handleRequest(request) });
    try {
      for (const name of workloads) {
        const fixture = fixtures[name];
        const response = await fetch(`http://127.0.0.1:${server.port}${fixture.path}`, {
          method: fixture.method, body: fixture.requestBody,
          headers: { authorization: "Bearer bench-token", "content-type": "application/json" },
        });
        expect(response.status).toBe(fixture.status);
        expect(response.headers.get("content-type")).toBe(fixture.contentType);
        expect(await response.text()).toBe(fixture.responseBody);
      }
    } finally { await server.stop(true); }
  });

  test("authorization and parameter/body validation run before handlers", async () => {
    const router = createReferenceRouter();
    const unauthorized = await router.handleRequest(new Request("http://localhost/static"));
    expect(unauthorized.status).toBe(401);
    for (const path of ["/items/not-a-number", "/search?filter.active=wrong&values.0=1"]) {
      const response = await router.handleRequest(new Request(`http://localhost${path}`, {
        headers: { authorization: "Bearer bench-token" },
      }));
      expect(response.status).toBe(400);
    }
    const response = await router.handleRequest(new Request("http://localhost/items", {
      method: "POST", body: '{"name":"sample","tags":[2]}',
      headers: { authorization: "Bearer bench-token", "content-type": "application/json" },
    }));
    expect(response.status).toBe(400);
  });
});

describe("benchmark result and resource boundaries", () => {
  test("parses kernel accounting with whitespace and parentheses in process names", () => {
    const fields = Array.from({ length: 20 }, (_, index) => index === 11 ? "120" : index === 12 ? "30" : "0");
    expect(parseCpuTicks(`42 (bench (worker) name) ${fields.join(" ")}`)).toBe(150);
    expect(parseMemory("Name:\tbench\nVmRSS:\t  123 kB\nVmHWM:\t 456 kB\n"))
      .toEqual({ rssBytes: 125952, peakRssBytes: 466944 });
    expect(() => parseCpuTicks("invalid")).toThrow("Invalid /proc stat");
    expect(() => parseMemory("VmRSS: 123 kB\n")).toThrow("VmHWM");
  });

  test("rejects malformed output and mismatches, transport failures and nonzero exits", () => {
    const valid = parseLoadResult(JSON.stringify(loadResult));
    expect(valid.completed).toBe(20);
    expect(() => parseLoadResult('{"workload":"static"}')).toThrow("Invalid loadgen output");
    expect(() => parseLoadResult(JSON.stringify({ ...loadResult, completed: -1 }))).toThrow();
    validatePhase({ result: valid, loadgenExit: 0 }, "static", 10, 2, 100);
    for (const change of [{ completed: 0 }, { mismatches: 1 }, { transportErrors: 1 }, { errors: ["failed"] }]) {
      expect(() => validatePhase({ result: { ...valid, ...change }, loadgenExit: 0 }, "static", 10, 2, 100)).toThrow("validation failed");
    }
    expect(() => validatePhase({ result: valid, loadgenExit: 1 }, "static", 10, 2, 100)).toThrow("validation failed");
    expect(() => validatePhase({ result: valid, loadgenExit: 0 }, "html", 10, 2, 100)).toThrow("parameters");
  });

  test("requires a candidate and at least three bounded samples", () => {
    expect(parseOptions(["--candidate", "/candidate", "--loadgen", "/loadgen"]).samples).toBe(3);
    expect(() => parseOptions(["--candidate", "/candidate", "--loadgen", "/loadgen", "--samples", "2"])).toThrow("Invalid benchmark arguments");
    expect(() => parseOptions(["--candidate", "/candidate"])).toThrow("Invalid benchmark arguments");
    expect(() => parseOptions(["--candidate", "/candidate", "--loadgen", "/loadgen", "--concurrency", "0"])).toThrow();
  });
});

describe("benchmark subprocess lifecycle", () => {
  test("starts and joins the reference server using port-zero NDJSON", async () => {
    const server = await startServer([process.execPath, new URL("./reference.ts", import.meta.url).pathname, "--port", "0"], 3000);
    const response = await fetch(`http://127.0.0.1:${server.port}/static`, { headers: { authorization: "Bearer bench-token" } });
    expect(await response.text()).toBe('{"ok":true}');
    await server.finish();
    expect(server.child.exitCode).toBe(0);
  });

  test("rejects invalid readiness and early exit", async () => {
    await expect(startServer([process.execPath, "-e", 'console.log(JSON.stringify({ready:true,port:0}))'], 1000))
      .rejects.toThrow("Invalid readiness");
    await expect(startServer([process.execPath, "-e", "process.exit(7)"], 1000))
      .rejects.toThrow("before readiness");
  });

  test("bounds readiness and forcibly joins a server that ignores termination", async () => {
    await expect(startServer([process.execPath, "-e", "setInterval(()=>{},1000)"], 80))
      .rejects.toThrow("timed out");
    const server = await startServer([process.execPath, "-e",
      'process.on("SIGTERM",()=>{}); console.log(JSON.stringify({ready:true,port:12345})); setInterval(()=>{},1000)'], 1000);
    await server.finish();
    expect(server.child.signalCode).toBe("SIGKILL");
  });
});
