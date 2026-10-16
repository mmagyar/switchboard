import { z } from "zod";

export const readySchema = z.object({ ready: z.literal(true), port: z.number().int().min(1).max(65535) }).strict();
export type Child = Bun.Subprocess<"ignore", "pipe", "inherit">;

export const deadline = async <T>(promise: Promise<T>, milliseconds: number, label: string): Promise<T> => {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const expired = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => reject(new Error(`${label} timed out after ${milliseconds}ms`)), milliseconds);
  });
  try { return await Promise.race([promise, expired]); }
  finally { clearTimeout(timer); }
};

export const stopChild = async (child: Child): Promise<void> => {
  if (child.exitCode === null) child.kill("SIGTERM");
  try { await deadline(child.exited, 1000, "SIGTERM join"); }
  catch {
    child.kill("SIGKILL");
    await deadline(child.exited, 2000, "SIGKILL join");
  }
};

export const readBounded = async (stream: ReadableStream<Uint8Array>, maxBytes = 65_536): Promise<string> => {
  const reader = stream.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let bytes = 0;
  let text = "";
  try {
    while (true) {
      const part = await reader.read();
      if (part.done) return text + decoder.decode();
      bytes += part.value.length;
      if (bytes > maxBytes) throw new Error(`Child stdout exceeds ${maxBytes} bytes`);
      text += decoder.decode(part.value, { stream: true });
    }
  } finally { reader.releaseLock(); }
};

export const startServer = async (command: readonly string[], startupTimeoutMs: number): Promise<{
  readonly child: Child;
  readonly port: number;
  readonly startupMs: number;
  readonly finish: () => Promise<void>;
}> => {
  const start = performance.now();
  const child = Bun.spawn([...command], { stdin: "ignore", stdout: "pipe", stderr: "inherit" });
  const ready = Promise.withResolvers<number>();
  const output = (async (): Promise<void> => {
    const reader = child.stdout.getReader();
    const decoder = new TextDecoder("utf-8", { fatal: true });
    let text = "";
    let bytes = 0;
    let receivedReady = false;
    try {
      while (true) {
        const part = await reader.read();
        if (part.done) break;
        bytes += part.value.length;
        if (bytes > 65_536) throw new Error("Server stdout exceeds 65536 bytes");
        text += decoder.decode(part.value, { stream: true });
        let newline = text.indexOf("\n");
        while (newline >= 0) {
          const line = text.slice(0, newline).trim();
          text = text.slice(newline + 1);
          if (line.length > 0) {
            if (receivedReady) throw new Error("Unexpected stdout after server readiness");
            const parsed = readySchema.safeParse(JSON.parse(line));
            if (!parsed.success) throw new Error(`Invalid readiness: ${parsed.error.message}`);
            receivedReady = true;
            ready.resolve(parsed.data.port);
          }
          newline = text.indexOf("\n");
        }
      }
      text += decoder.decode();
      if (text.trim().length > 0) throw new Error("Unterminated server stdout record");
      if (!receivedReady) throw new Error("Server exited before readiness");
    } finally { reader.releaseLock(); }
  })();
  // Observe pump failures immediately and retain them for the joined shutdown path.
  const observed = output.then(() => ({ ok: true as const }), (error: unknown) => {
    ready.reject(error);
    return { ok: false as const, error };
  });
  try {
    const port = await deadline(ready.promise, startupTimeoutMs, "server readiness");
    return {
      child, port, startupMs: performance.now() - start,
      finish: async () => {
        await stopChild(child);
        const result = await deadline(observed, 2000, "server stdout join");
        if (!result.ok) throw result.error;
      },
    };
  } catch (error) {
    await stopChild(child);
    await deadline(observed, 2000, "failed server stdout join");
    throw error;
  }
};
