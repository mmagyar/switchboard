import type { Server, ServerWebSocket } from "bun";
import type { Router } from "./index.ts";
import { chmodSync } from "fs";

/* Extracts a PEM block by its label (e.g. "CERTIFICATE" or "(?:[A-Z]+ )?PRIVATE KEY").
 * The backreference ties BEGIN/END to the same label, so e.g. RSA PRIVATE KEY and
 * plain PRIVATE KEY (PKCS1 vs PKCS8, depending on the openssl version/config) both match.
 */
const extractPemBlock = (text: string, labelPattern: string): string | undefined => {
  const match = text.match(new RegExp(`-----BEGIN (${labelPattern})-----[\\s\\S]+?-----END \\1-----`));
  return match?.[0]?.trim();
};

/** Both blocks or nothing — a half-parsed bundle is never a usable TLS config. */
const readPem = (text: string): { key: string; cert: string } | undefined => {
  const key = extractPemBlock(text, "(?:[A-Z]+ )?PRIVATE KEY");
  const cert = extractPemBlock(text, "CERTIFICATE");
  return key && cert ? { key, cert } : undefined;
};

/* Allow using ssl for local development without any complicated workarounds
 * Since this cert will not be validated by a CA you'll need to bypass that warning message
 */
const genCert = async (domain: string = "localhost"): Promise<{ key: string; cert: string }> => {
  //save cert so it does not change between runs, triggering the warning, since it's not a "validated" cert
  const filePath = "./.genCert";
  const cached = Bun.file(filePath);
  // A cached file that no longer parses (truncated write, manual edit) is regenerated
  // rather than propagated as a broken TLS config.
  const fromCache = (await cached.exists()) ? readPem(await cached.text()) : undefined;
  if (fromCache) return fromCache;

  let caSpawn;
  try {
    caSpawn = Bun.spawnSync({
      cmd: [
        "openssl",
        "req",
        "-x509",
        "-newkey",
        "rsa:4096",
        "-days",
        "2650",
        "-keyout",
        "/dev/stdout",
        "-out",
        "/dev/stdout",
        "-subj",
        "/CN=" + domain,
        "-nodes",
      ],
    });
  } catch (error) {
    // Bun throws (ENOENT) before producing an exit code when the binary is absent —
    // the most likely failure this message exists for.
    throw new Error(
      `https: "generate" requires openssl, but spawning it failed: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  if (caSpawn.exitCode !== 0) {
    throw new Error(
      `https: "generate" requires openssl, but it exited with ${caSpawn.exitCode}: ${caSpawn.stderr.toString().trim()}`,
    );
  }
  const outstring = caSpawn.stdout.toString();
  const generated = readPem(outstring);
  if (!generated) {
    throw new Error("https: \"generate\" failed — openssl produced no PRIVATE KEY and CERTIFICATE pair");
  }
  await Bun.write(filePath, outstring);
  chmodSync(filePath, 0o600);
  return generated;
};

const accessLogDefault = (time: number, req: Request, res?: Response) => {
  console.info(
    `${req.method} ${req.url} ${res?.status ?? null} ${res?.headers.get("content-type") || "no body"} ${Math.round(time * 1000) / 1000}ms`,
  );
};

export const serveHotBuns = async (
  confIn: {
    port?: number;
    hostname?: string;
    development?: boolean;
    https?: "generate" | { cert: string; key: string };
    idleTimeout?: number;
  },
  r: Router,
  accessLog: (duration: number, req: Request, res: Response | undefined) => void = accessLogDefault,
  readLogs?: () => Promise<string>,
  // May return a closable handle (e.g. the fs.FSWatcher from logFileChangeWatcher). When it does,
  // the watcher is released by the returned `stop()` instead of being leaked for the process lifetime.
  watchLogs?: (onChange: () => void) => { close: () => void } | void,
): Promise<(() => void) & { stop: () => void }> => {
  let wsc = new Set<ServerWebSocket<unknown>>();
  const sendReload: (() => void) & { stop: () => void } = Object.assign(
    () => {
      wsc.forEach((x) => x.send("RELOAD"));
    },
    { stop: () => {} },
  );
  const ssl = confIn.https === "generate" ? await genCert() : confIn.https;
  // When using ssl, redirect non ssl requests to ssl port 443
  const sslPort = confIn.port ?? 443;
  let redirectServer: Server<undefined> | undefined;
  if (ssl && sslPort !== 80) {
    redirectServer = Bun.serve({
      port: 80,
      hostname: confIn.hostname || "0.0.0.0",
      development: confIn.development ?? true,
      idleTimeout: confIn.idleTimeout ?? 10,
      fetch: async (req): Promise<Response> => {
        //redirect to https:
        const url = new URL(req.url);
        url.protocol = "https";
        url.port = "443";
        return new Response(null, {
          status: 301,
          headers: {
            Location: url.toString(),
          },
        });
      },
    });
  }

  // Set up log watcher once — broadcasts to all connected WebSocket clients
  let logWatcher: { close: () => void } | void = undefined;
  if (readLogs && watchLogs) {
    logWatcher = watchLogs(async () => {
      const logs = await readLogs();
      wsc.forEach((ws) => {
        try {
          ws.send(logs);
        } catch {
          // ignore send errors for individual clients (they will be removed on close)
        }
      });
    });
  }

  let server: Server<undefined>;
  try {
    server = Bun.serve({
      port: confIn.port ?? (ssl ? 443 : 80),
      hostname: confIn.hostname || "0.0.0.0",
      development: confIn.development ?? true,
      idleTimeout: confIn.idleTimeout ?? 10,
      //generate new CA with openssl on the fly
      tls: ssl,
      fetch: async (req): Promise<Response | undefined> => {
        const url = req.url;
        const method = req.method;
        const success = server.upgrade(req);
        if (success) {
          // Bun automatically returns a 101 Switching Protocols
          // if the upgrade succeeds
          console.info(`${method} ${url} 101 Websocket`);
          return undefined;
        }
        let res;
        let startTime = performance.now();
        try {
          return (res = await r.handleRequest(req));
        } finally {
          accessLog?.(performance.now() - startTime, req, res);
        }
      },
      websocket: {
        async message(ws, _message) {
          if (readLogs) ws.send(await readLogs());
        }, // a message is received
        open(ws) {
          wsc.add(ws);
        }, // a socket is opened
        close(ws, _code, _message) {
          wsc.delete(ws);
        }, // a socket is closed
        drain(_ws) {}, // the socket is ready to receive more data
      },
    });
  } catch (error) {
    // A failed start (e.g. port in use) throws before the caller receives a stop() handle —
    // the already-started :80 redirect server and the log watcher must not leak.
    logWatcher?.close();
    void redirectServer?.stop(true);
    throw error;
  }
  console.log(`serving hot buns at: ${server.hostname}:${server.port}`);
  console.log(`development mode: ${confIn.development}`);
  // Still callable as the plain reload broadcaster it has always been; `stop` is additive.
  // The hot-reload WebSockets this server exists to hold would keep a graceful stop pending
  // indefinitely, so close them explicitly, then force-stop to cover any other connection.
  sendReload.stop = () => {
    logWatcher?.close();
    void redirectServer?.stop(true);
    wsc.forEach((ws) => ws.close());
    void server.stop(true);
  };
  return sendReload;
};
