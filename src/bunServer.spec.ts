import { describe, expect, test } from "bun:test";
import { serveHotBuns } from "./bunServer.ts";
import { Router } from "./router.ts";

/** Grabs an OS-assigned free port. A tiny race remains between release and reuse — acceptable in tests. */
const freePort = (): number => {
  const probe = Bun.serve({ port: 0, fetch: () => new Response("") });
  const port = probe.port!;
  void probe.stop(true);
  return port;
};

const okRouter = () => new Router(async () => new Response("ok"));

describe("serveHotBuns", () => {
  test("stop() force-closes open hot-reload WebSockets and frees the port", async () => {
    const port = freePort();
    const reload = await serveHotBuns({ port }, okRouter());

    const ws = new WebSocket(`ws://localhost:${port}`);
    await new Promise((resolve) => ws.addEventListener("open", resolve));
    const closed = new Promise((resolve) => ws.addEventListener("close", resolve));

    reload.stop();
    // A graceful stop would wait on the open socket forever and time this test out
    await closed;
    await expect(fetch(`http://localhost:${port}`)).rejects.toThrow();
  });

  test("stop() closes the log file watcher", async () => {
    const port = freePort();
    let watcherClosed = false;
    const reload = await serveHotBuns(
      { port },
      okRouter(),
      undefined,
      async () => "logs",
      () => ({ close: () => (watcherClosed = true) }),
    );
    expect(watcherClosed).toBe(false);
    reload.stop();
    expect(watcherClosed).toBe(true);
  });

  test("calling the returned function still broadcasts without throwing", async () => {
    const port = freePort();
    const reload = await serveHotBuns({ port }, okRouter());
    expect(() => reload()).not.toThrow();
    reload.stop();
  });

  test("a failed server start closes the log watcher instead of leaking it", async () => {
    let watcherClosed = false;
    // An unbindable hostname makes the main Bun.serve throw without needing a second
    // server to occupy the port (which trips a Bun 1.3.14 segfault in the test runner).
    await expect(
      serveHotBuns(
        { port: freePort(), hostname: "invalid..host..name" },
        okRouter(),
        undefined,
        async () => "logs",
        () => ({ close: () => (watcherClosed = true) }),
      ),
    ).rejects.toThrow();
    expect(watcherClosed).toBe(true);
  });
});
