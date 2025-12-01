import type { ServerWebSocket } from "bun";
import { logFileChangeWatcher, readLogfile } from "./logger.ts";
import type { Router } from "./index.ts";
import { VerboseErrorOutput } from "./env.ts";

/* Allow using ssl for local development without any complicated workarounds
 * Since this cert will not be validated by a CA you'll need to bypass that warning message
 * TODO domain is not validated, so if it changes it will stilluse the file, so delete it manually
 */
const genCert = async (domain: string = "matebookpro.local") => {
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
    Bun.write(filePath, outstring);
  }
  const parts = outstring.split(/(?=-----BEGIN CERTIFICATE-----)/);
  return {
    key: parts[0]?.trim(),
    cert: parts[1]?.trim(),
  };
};

const accessLogDefault = (time: number, req: Request, res?: Response) => {
  const url = new URL(req.url);
  //Do not log requiests to resources in public folder
  if (url.pathname.startsWith("/public")) {
    return;
  }

  // Do not log app.log requests, as that would just same the logs
  if (url.pathname.startsWith("/app.log")) {
    return;
  }

  const ao: {
    method: string;
    url: string;
    status: number | null;
    contentType: string;
    authenticated: boolean;
    time: number;
  } = {
    method: req.method,
    url: req.url,
    status: res?.status ?? null,
    contentType: res?.headers.get("content-type") || "no body",
    authenticated: false,
    time: time,
  };
  console.info(
    `${ao.method} ${ao.url} ${ao.status} ${ao.contentType} ${Math.round(ao.time * 1000) / 1000}ms ${ao.authenticated} `,
  );
};

export const serveHotBuns = async (
  confIn: {
    port?: number;
    hostname?: string;
    development?: boolean;
    https?: "generate" | { cert: string; key: string };
  },
  r: Router,
  accessLog: (duration: number, req: Request, res: Response | undefined) => void = accessLogDefault,
): Promise<() => void> => {
  let wsc: ServerWebSocket<unknown>[] | undefined;
  if (!wsc) wsc = [];
  const sendReload = () => {
    wsc?.forEach((x) => x.send("RELOAD"));
  };
  const ssl = confIn.https === "generate" ? await genCert() : confIn.https;
  // When using ssl, redirect non ssl requests to ssl port 443
  if (ssl && (!confIn.port || confIn.port === 443 || confIn.port === 80)) {
    Bun.serve({
      port: 80,
      hostname: confIn.hostname || "0.0.0.0",
      development: confIn.development || true,
      idleTimeout: 10,
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

  const server = Bun.serve({
    port: confIn.port ?? (ssl ? 443 : 80),
    hostname: confIn.hostname || "0.0.0.0",
    development: confIn.development || true,
    idleTimeout: 10,
    //generate new CA with openssl on the fly
    tls: ssl,
    fetch: async (req): Promise<Response> => {
      const url = req.url;
      const method = req.method;
      const success = server.upgrade(req);
      if (success) {
        // Bun automatically returns a 101 Switching Protocols
        // if the upgrade succeeds
        console.info(`${method} ${url} 101 Websocket`);
        return undefined as any;
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
        if (VerboseErrorOutput) ws.send(await readLogfile());
      }, // a message is received
      open(ws) {
        if (VerboseErrorOutput) {
          wsc?.push(ws);
          logFileChangeWatcher(async () => {
            ws.send(await readLogfile());
          });
        }
      }, // a socket is opened
      close(ws, _code, _message) {
        //remove ws from wsc
        wsc = wsc?.filter((w) => w !== ws);
      }, // a socket is closed
      drain(_ws) {}, // the socket is ready to receive more data
    },
  });
  console.log(`serving hot buns at: ${server.hostname}:${server.port}`);
  console.log(`development mode: ${confIn.development}`);
  return sendReload;
};
