import { parseArgs } from "node:util";
import { z } from "zod";
import { define, RouteHandlerDefiner, Router } from "../../src/index.ts";

export const createReferenceRouter = (): Router => {
  const router = new Router();
  const def = define<"bench">();
  const bind = RouteHandlerDefiner(
    async (user: string | null, permission: "bench") =>
      permission === "bench" && user === "Bearer bench-token" ? "ok" : "unauthenticated",
    async (request: Request) => request.headers.get("authorization"),
    { outputErrorWarning: (error) => { throw new Error(`Benchmark output invalid: ${error.message}`); } },
  );
  const id = z.object({ id: z.number() });
  const search = z.object({ filter: z.object({ active: z.boolean() }), values: z.array(z.number()) });
  router.addRoute(bind(def.get("/static", "bench", z.object({ ok: z.boolean() })), () => ({ ok: true })));
  router.addRoute(bind(def.get("/items/:id", "bench", id, id), (params) => ({ id: params.id })));
  router.addRoute(bind(def.get("/search", "bench", search, search), (params) => params));
  router.addRoute(bind(def.post("/items", "bench",
    z.object({ name: z.string(), tags: z.array(z.string()) }),
    z.object({ name: z.string(), count: z.number() })),
  (_params, body) => ({ name: body.name, count: body.tags.length })));
  router.addRoute(bind(def.get("/html/:id", "bench", id, id), (params) => ({ id: params.id }),
    (data) => ({
      data: `<main><h1>Item ${data.id}</h1><p>Switchboard</p></main>`,
      headers: new Headers({ "content-type": "text/html; charset=utf-8" }),
    })));
  return router;
};

if (import.meta.main) {
  const args = parseArgs({ args: Bun.argv.slice(2), options: { port: { type: "string", default: "0" } }, strict: true });
  const port = z.coerce.number().int().min(0).max(65535).safeParse(args.values.port);
  if (!port.success) throw new Error(`Invalid port: ${port.error.message}`);
  const router = createReferenceRouter();
  const server = Bun.serve({ hostname: "127.0.0.1", port: port.data, fetch: (request) => router.handleRequest(request) });
  const shutdown = (): void => { void server.stop(true); };
  process.on("SIGTERM", shutdown);
  process.on("SIGINT", shutdown);
  console.log(JSON.stringify({ ready: true, port: server.port }));
}
