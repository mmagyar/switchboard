import type { ServerWebSocket } from "bun";
import type { Router } from "./index.ts";

/* Allow using ssl for local development without any complicated workarounds
 * Since this cert will not be validated by a CA you'll need to bypass that warning message
 */
const genCert = async (domain: string = "localhost") => {
  //save cert so it does not change between runs, triggering the warning, since it's not a "validated" cert
  const filePath = "./.genCert";
  const cert = Bun.file(filePath);
  let outstring: string;
  if (await cert.exists()) {
    outstring = await cert.text();
  } else {
    const caSpawn = Bun.spawnSync({
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
    outstring = caSpawn.stdout.toString();
    await Bun.write(filePath, outstring);
    await Bun.$`chmod 600 ${filePath}`;
  }
  const parts = outstring.split(/(?=-----BEGIN CERTIFICATE-----)/);
  return {
    key: parts[0]?.trim(),
    cert: parts[1]?.trim(),
  };
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
  watchLogs?: (onChange: () => void) => void,
): Promise<() => void> => {
  let wsc = new Set<ServerWebSocket<unknown>>();
  const sendReload = () => {
    wsc.forEach((x) => x.send("RELOAD"));
  };
  const ssl = confIn.https === "generate" ? await genCert() : confIn.https;
  // When using ssl, redirect non ssl requests to ssl port 443
  const sslPort = confIn.port ?? 443;
  if (ssl && sslPort !== 80) {
    Bun.serve({
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
  if (readLogs && watchLogs) {
    watchLogs(async () => {
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

  const server = Bun.serve({
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
  console.log(`serving hot buns at: ${server.hostname}:${server.port}`);
  console.log(`development mode: ${confIn.development}`);
  return sendReload;
};
