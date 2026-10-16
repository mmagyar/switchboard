import { open, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { parseArgs } from "node:util";
import { z } from "zod";
import { deadline, readBounded, startServer, stopChild } from "./processes.ts";
import { environment, readResources } from "./resources.ts";
import { fixtures, parseLoadResult, workloads } from "./workloads.ts";
import type { LoadResult, WorkloadName } from "./workloads.ts";

const integer = (max: number, minimum = 1): z.ZodCoercedNumber<unknown> => z.coerce.number().int().min(minimum).max(max);
const optionsSchema = z.object({
  candidate: z.string().min(1), loadgen: z.string().min(1), output: z.string().min(1),
  samples: integer(100, 3), "duration-ms": integer(300_000), "warmup-ms": integer(300_000),
  concurrency: integer(1024), "timeout-ms": integer(60_000), "startup-timeout-ms": integer(60_000),
}).strict();

export const parseOptions = (args: readonly string[]): z.infer<typeof optionsSchema> => {
  const parsed = parseArgs({ args: [...args], strict: true, options: {
    candidate: { type: "string" }, loadgen: { type: "string" },
    output: { type: "string", default: `.tmp/rust-bench-${Date.now()}.ndjson` },
    samples: { type: "string", default: "3" }, "duration-ms": { type: "string", default: "1000" },
    "warmup-ms": { type: "string", default: "500" }, concurrency: { type: "string", default: "16" },
    "timeout-ms": { type: "string", default: "5000" }, "startup-timeout-ms": { type: "string", default: "10000" },
  } });
  const result = optionsSchema.safeParse(parsed.values);
  if (!result.success) throw new Error(`Invalid benchmark arguments: ${result.error.message}`);
  return result.data;
};

type Phase = {
  readonly result: LoadResult;
  readonly loadgenExit: number;
  readonly resources: {
    readonly cpuSeconds: number;
    readonly rssBeforeBytes: number;
    readonly rssAfterBytes: number;
    readonly sampledMaxRssBytes: number;
    readonly processPeakRssBytes: number;
    readonly pollingIntervalMs: number;
  };
};

const runPhase = async (
  command: readonly string[], serverPid: number, clockTicks: number, timeoutMs: number,
): Promise<Phase> => {
  const before = await readResources(serverPid, clockTicks);
  const child = Bun.spawn([...command], { stdin: "ignore", stdout: "pipe", stderr: "inherit" });
  let done = false;
  let sampledMaxRssBytes = before.rssBytes;
  const poll = (async (): Promise<void> => {
    while (!done) {
      const resources = await readResources(serverPid, clockTicks);
      sampledMaxRssBytes = Math.max(sampledMaxRssBytes, resources.rssBytes);
      await Bun.sleep(25);
    }
  })();
  const observedPoll = poll.catch((error: unknown) => {
    child.kill("SIGKILL");
    throw error;
  });
  const capture = (async (): Promise<{ readonly text: string; readonly exit: number }> => {
    try {
      const [text, exit] = await deadline(Promise.all([readBounded(child.stdout), child.exited]), timeoutMs, "loadgen");
      return { text, exit };
    } finally { done = true; }
  })();
  try {
    const [captured] = await Promise.all([capture, observedPoll]);
    const after = await readResources(serverPid, clockTicks);
    const result = parseLoadResult(captured.text);
    return {
      result, loadgenExit: captured.exit,
      resources: {
        cpuSeconds: after.cpuSeconds - before.cpuSeconds,
        rssBeforeBytes: before.rssBytes, rssAfterBytes: after.rssBytes,
        sampledMaxRssBytes: Math.max(sampledMaxRssBytes, after.rssBytes),
        processPeakRssBytes: after.peakRssBytes, pollingIntervalMs: 25,
      },
    };
  } finally {
    done = true;
    await stopChild(child);
    await Promise.allSettled([capture, observedPoll]);
  }
};

const binaryDetails = async (path: string): Promise<{ readonly path: string; readonly bytes: number; readonly sha256: string }> => {
  const bytes = await Bun.file(path).arrayBuffer();
  return { path, bytes: bytes.byteLength, sha256: new Bun.CryptoHasher("sha256").update(bytes).digest("hex") };
};

export const runBenchmark = async (options: z.infer<typeof optionsSchema>): Promise<string> => {
  const outputPath = resolve(options.output);
  await mkdir(dirname(outputPath), { recursive: true });
  const output = await open(outputPath, "wx");
  const write = async (value: unknown): Promise<void> => { await output.write(`${JSON.stringify(value)}\n`); };
  try {
    const env = await environment();
    const candidate = resolve(options.candidate);
    const loadgen = resolve(options.loadgen);
    const reference = new URL("./reference.ts", import.meta.url).pathname;
    const commands = {
      bun: [process.execPath, reference, "--port", "0"],
      rust: [candidate, "--port", "0"],
    };
    await write({
      type: "environment", protocolVersion: 1, timestamp: new Date().toISOString(),
      environment: env.details, parameters: options, invocation: Bun.argv,
      binaries: { candidate: await binaryDetails(candidate), loadgen: await binaryDetails(loadgen) },
      serverCommands: commands,
      workloadFixtures: Object.fromEntries(workloads.map((name) => [name, {
        ...fixtures[name], requestBodyBytes: Buffer.byteLength(fixtures[name].requestBody ?? ""),
        responseBodyBytes: Buffer.byteLength(fixtures[name].responseBody),
      }])),
      routeCount: 5,
      browserAssets: { status: "pending", reason: "separate optional WASM artifact" },
    });
    for (let sample = 0; sample < options.samples; sample += 1) {
      const order: readonly ("bun" | "rust")[] = sample % 2 === 0 ? ["bun", "rust"] : ["rust", "bun"];
      for (const implementation of order) {
        console.error(`sample ${sample + 1}/${options.samples}: starting ${implementation}`);
        const server = await startServer(commands[implementation], options["startup-timeout-ms"]);
        try {
          await write({ type: "startup", sample: sample + 1, order, implementation,
            command: commands[implementation], pid: server.child.pid, port: server.port, startupMs: server.startupMs });
          for (const workload of workloads) {
            for (const phase of ["warmup", "measure"] as const) {
              const durationMs = phase === "warmup" ? options["warmup-ms"] : options["duration-ms"];
              const command = [loadgen, "--base-url", `http://127.0.0.1:${server.port}`, "--workload", workload,
                "--duration-ms", String(durationMs), "--concurrency", String(options.concurrency),
                "--timeout-ms", String(options["timeout-ms"])];
              console.error(`sample ${sample + 1}: ${implementation} ${workload} ${phase}`);
              const result = await runPhase(command, server.child.pid, env.clockTicks,
                durationMs + options["timeout-ms"] + 5000);
              await write({ type: "sample", sample: sample + 1, order, implementation, workload, phase, command, ...result });
              validatePhase(result, workload, durationMs, options.concurrency, options["timeout-ms"]);
              if (server.child.exitCode !== null) throw new Error(`${implementation} server exited unexpectedly`);
            }
          }
        } finally { await server.finish(); }
      }
    }
    await write({ type: "complete", samples: options.samples, measurements: options.samples * 2 * workloads.length });
    console.error(`raw results: ${outputPath}`);
    return outputPath;
  } catch (error) {
    await write({ type: "failure", message: error instanceof Error ? error.message : String(error) });
    throw error;
  } finally {
    try { await output.sync(); }
    finally { await output.close(); }
  }
};

export const validatePhase = (
  phase: Pick<Phase, "result" | "loadgenExit">, workload: WorkloadName,
  durationMs: number, concurrency: number, timeoutMs: number,
): void => {
  const result = phase.result;
  if (result.workload !== workload || result.durationMs !== durationMs || result.concurrency !== concurrency || result.timeoutMs !== timeoutMs) {
    throw new Error("loadgen result parameters do not match invocation");
  }
  if (phase.loadgenExit !== 0 || result.completed === 0 || result.transportErrors !== 0 || result.mismatches !== 0 || result.errors.length !== 0) {
    throw new Error(`loadgen validation failed: exit=${phase.loadgenExit}, ${JSON.stringify(result)}`);
  }
};

if (import.meta.main) {
  try { await runBenchmark(parseOptions(Bun.argv.slice(2))); }
  catch (error) {
    console.error(error);
    process.exitCode = 1;
  }
}
