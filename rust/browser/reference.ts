// Reference/test tooling only. The product browser application is authored in Rust.
import { ApiError, createClient, define } from "../../src/clientExport.ts";
import { z } from "zod";

const paramsSchema = z.object({ itemId: z.number().int().nonnegative().max(4_294_967_295), query: z.string() });
const outputSchema = z.object({ title: z.literal("Item 42"), query: z.string() });
const def = define<"public">();
const route = def.get("/api/items/:itemId", "public", outputSchema, paramsSchema);
const bad = def.get("/api/refinement/:itemId", "public", outputSchema, paramsSchema);
const client = createClient("");
const params = { itemId: 42, query: "a b&c" };

const element = (id: string): HTMLElement => {
  const value = document.getElementById(id);
  if (!value) throw new Error(`Missing #${id}`);
  return value;
};
const item = await client(route, params);
if (!item) throw new Error("Missing item");
element("fetch").textContent = `${item.title}; query: ${item.query}`;
try {
  await client(bad, params);
  throw new Error("Expected schema rejection");
} catch (error: unknown) {
  if (!(error instanceof z.ZodError)) throw error;
  element("validation").textContent = `Schema rejected: ${JSON.stringify(error.issues, null, 2)}`;
}
const value: unknown = await client(bad, params, undefined, { validateReturn: false });
element("unchecked").textContent = `Untrusted JSON: ${JSON.stringify(value)}`;

const negativeRoute = def.post("/health", "public", z.boolean(), z.unknown());
const failures: string[] = [];
for (const method of ["GET", "HEAD", "TRACE", "Invalid header"]) {
  try {
    await client(negativeRoute, {}, method === "Invalid header" ? undefined : false, {
      methodOverride: method === "Invalid header" ? "GET" : method,
      authTokenOverride: method === "Invalid header" ? "bad\r\nvalue" : undefined,
    });
    throw new Error("Expected fetch initialization failure");
  } catch (error: unknown) {
    if (!(error instanceof ApiError) || error.kind !== "network" || error.status !== 0) throw error;
    failures.push(`Network/0: ${error.message}`);
  }
}
element("transport").textContent = failures.join("\n");

const increment = new AbortController();
const controls = new AbortController();
let count = 0;
element("increment").addEventListener("click", () => {
  count += 1;
  element("count").textContent = String(count);
}, { signal: increment.signal });
const socket = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/hot`);
const message = (event: MessageEvent<unknown>): void => {
  if (typeof event.data !== "string") return;
  if (event.data === "RELOAD") location.reload();
  else element("logs").textContent = event.data;
};
socket.addEventListener("message", message);
socket.addEventListener("open", () => {
  element("socket").textContent = "Open";
  socket.send("");
}, { signal: controls.signal });
socket.addEventListener("close", () => { element("socket").textContent = "Closed"; }, { signal: controls.signal });
const dispose = (): void => {
  increment.abort();
  socket.removeEventListener("message", message);
  socket.close();
};
element("dispose").addEventListener("click", () => {
  dispose();
  element("disposal").textContent = "Disposed";
}, { signal: controls.signal });
window.addEventListener("pagehide", () => {
  dispose();
  controls.abort();
}, { once: true, signal: controls.signal });
element("status").textContent = "Ready";
