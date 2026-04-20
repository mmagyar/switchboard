# Switchboard

A type-safe router for Bun with Zod validation, permission handling, and streaming SSR support.

## Installation

```sh
bun add https://github.com/mmagyar/switchboard
```

## Features

- Type-safe route definitions with compile-time path validation
- Zod validation for params, body, and output
- Permission handling via a pluggable authorizer
- Automatic parameter parsing from path and query string
- Promisable handler return types — individual properties can be Promises (streaming SSR)
- Pluggable error handling, HTML error pages, and logging
- Built-in HTTPS for local development (self-signed cert generated on demand)
- WebSocket-based hot reload support

## Usage

### 1. Set up the definer

```typescript
import { define, RouteHandlerDefiner, Router, serveHotBuns } from "switchboard/server";
import { z } from "zod";

export type PermissionsType = "public" | "private";
export type User = { name: string };

export const def = define<PermissionsType>();

export const handle = RouteHandlerDefiner<User, PermissionsType>(
  async (_user, permissionsNeeded, _req) => {
    // Return "ok", "forbidden", or "unauthenticated"
    return permissionsNeeded === "public" ? "ok" : "forbidden";
  },
  async (_req): Promise<User> => {
    // Resolve the user from the request (JWT, session, etc.)
    return { name: "John" };
  },
  {
    // Optional: warn when handler output doesn't match the output schema
    outputErrorWarning: (error, data, method, url) => {
      console.warn("Output validation failed", { method, url, error, data });
    },
    // Optional: map thrown errors to HTTP status + message
    errorParser: async (error) => {
      if (error instanceof MyAppError) return { status: error.status, message: error.message };
    },
    // Optional: render a custom HTML error page
    errorHtmlFormatter: async (status, message, _req, _user) => {
      return `<html><body><h1>${status}</h1><p>${message}</p></body></html>`;
    },
    // Optional: override the default error logger (defaults to console.error)
    errorLogger: (...args) => myLogger.error(...args),
  },
);
```

### 2. Define routes

For **bodyless** methods (`get`, `delete`, etc.) the handler signature is `(params, user)`.
For **body** methods (`post`, `put`, `patch`) the handler signature is `(params, body, user)`.

```typescript
const listRoute = def.get(
  "/orders",
  "public",
  z.object({ orders: z.array(z.string()) }),
  z.object({ search: z.string().optional() }),
);

export const listHandler = handle(listRoute, async ({ search }, user) => {
  return { orders: search ? ["burger"] : ["burger", "fries"] };
});

const createRoute = def.post(
  "/orders",
  "private",
  z.object({ item: z.string() }),        // body schema
  z.object({ id: z.number() }),           // output schema
);

// post/put/patch: (params, body, user)
export const createHandler = handle(createRoute, async (_params, body, user) => {
  return { id: 42 };
});
```

### 3. Register routes and start the server

```typescript
const routes = [listHandler, createHandler];

const router = new Router();
// Pass the route object directly — method and path are already encoded in it.
// Do NOT call router.addRoute(r.method, r.path, r.handlerWrapped); that
// duplicates information that lives in the route definition and breaks the
// single-source-of-truth guarantee.
routes.forEach((r) => router.addRoute(r));

// Full-featured Bun server with optional HTTPS and hot reload
await serveHotBuns(
  {
    port: 8891,
    development: true,
    hostname: "localhost",
    https: "generate", // auto-generates a self-signed cert
  },
  router,
);

// Or, barebones — handle requests yourself
const res = await router.handleRequest(req);
```

---

## API Reference

### `define<PERMISSION>()`

Returns a set of route definition helpers (`get`, `post`, `put`, `patch`, `del`, `options`). Each enforces `ValidateOptionalUrl` on the path at the type level, catching malformed paths at compile time.

```typescript
const def = define<"public" | "admin">();

def.get(path, permissionsNeeded, outputSchema, paramsSchema?)
def.post(path, permissionsNeeded, bodySchema, outputSchema, paramsSchema?)
def.put(path, permissionsNeeded, bodySchema, outputSchema, paramsSchema?)
def.patch(path, permissionsNeeded, bodySchema, outputSchema, paramsSchema?)
def.del(path, permissionsNeeded, outputSchema, paramsSchema)
def.options(path, permissionsNeeded, outputSchema, paramsSchema?)
```

If no `paramsSchema` is provided, one is derived automatically from the path template (e.g. `/:id` → `{ id: z.number() }`).

---

### `RouteHandlerDefiner<USER, PERMISSION>(authorizer, getUserFromRequest, options?)`

Creates a `handle` function bound to your auth and user-resolution logic.

| Parameter | Type | Description |
|---|---|---|
| `authorizer` | `(user, permissionsNeeded, req) => Promise<"ok" \| "forbidden" \| "unauthenticated">` | Checks whether the resolved user may access the route |
| `getUserFromRequest` | `(req) => Promise<USER>` | Resolves the current user from the request |
| `options` | `RouteHandlerOptions<USER>` | Optional callbacks — see below |

#### `RouteHandlerOptions<USER>`

```typescript
type RouteHandlerOptions<USER> = {
  outputErrorWarning?: (error: ZodError, data: unknown, method: string, url: string) => void;
  errorParser?: (error: unknown) => Promise<{ status: number; message: string } | undefined>;
  errorHtmlFormatter?: (status: number, message: string, request: Request, user?: USER) => Promise<string>;
  errorLogger?: (...args: unknown[]) => void; // defaults to console.error
};
```

---

### Built-in error classes

Switchboard exports three error classes that handlers can throw directly. When one of these is thrown, switchboard automatically produces the correct HTTP response — no `errorParser` needed.

| Class | Constructor | Status | Response body |
|---|---|---|---|
| `NotFoundError` | `new NotFoundError()` | `404` | `"Not Found - Missing entry"` |
| `Unauthorized` | `new Unauthorized(status?, message?)` | `401` or `403` | The `message` argument (defaults to `"Unauthorized"`) |
| `RequestError` | `new RequestError(status, message)` | Any | The `message` argument |

```typescript
import { NotFoundError, Unauthorized, RequestError } from "switchboard/server";

handle(def.get("/items/:id", "public", outputSchema), async ({ id }) => {
  const item = await db.findItem(id);
  if (!item) throw new NotFoundError();

  if (!item.isPublished) throw new RequestError(422, "Item is not published yet");

  return item;
});
```

Anything that is not one of these three classes falls through to a generic `500 Internal Server Error`.

If you supply an `errorParser` in `RouteHandlerOptions`, it runs **before** these checks and can override any of them — return `{ status, message }` from it to short-circuit the built-in handling.

#### `ApiError` *(client-side)*

`ApiError` is a proper `Error` subclass thrown by the function returned from `createClient` when the server responds with an HTTP status ≥ 400. It exposes a `.status: number` field so callers can branch on the exact status code:

```typescript
import { ApiError, createClient } from "switchboard/client";

const call = createClient("https://api.example.com");
try {
  const result = await call(myRoute, params);
} catch (err) {
  if (err instanceof ApiError) {
    console.error(`HTTP ${err.status}: ${err.message}`);
  }
}
```

---

### `handle(routeDef, handler, formatOutput?)`

Binds a handler function to a route definition.

- For **bodyless** methods (`get`, `delete`, etc.) the handler signature is `(params, user)`.
- For **body** methods (`post`, `put`, `patch`) the handler signature is `(params, body, user)`.

```typescript
const myRoute = handle(
  def.get("/items/:id", "public", outputSchema),
  async ({ id }, user) => {
    return { item: await db.findItem(id) };
  },
  (data, user, req, params) => {
    // Optional: format the response yourself (e.g. render HTML)
    return { data: renderHtml(data), headers: new Headers({ "Content-Type": "text/html" }) };
  },
);
```

#### `formatOutput` return shape

The object returned by `formatOutput` has the following fields:

| Field | Type | Description |
|---|---|---|
| `data` | `BodyInit \| undefined` | The response body |
| `headers` | `Headers` | Response headers (e.g. `Content-Type`) |
| `redirect` | `true \| undefined` | When set, the response status defaults to `303 See Other` |
| `status` | `number \| undefined` | Explicit status code; takes priority over the `redirect` default and the method default |

#### Promisable handler returns

Handlers may return an object whose **individual properties are Promises**. When a `formatOutput` function is provided, those Promise-valued properties are passed through as-is, enabling streaming SSR. Output validation fires per-property in the background as each Promise resolves.

When no `formatOutput` is provided (JSON path), all properties are awaited before serialization.

```typescript
handle(routeDef, async (params) => {
  return {
    user: db.getUser(params.id),        // Promise — resolved lazily
    settings: db.getSettings(params.id), // Promise — resolved lazily
  };
}, renderHtml(MyView));
```

---

### `serveHotBuns(conf, router, accessLog?, readLogs?, watchLogs?)`

Starts a Bun HTTP(S) server wired to the router, with optional WebSocket hot-reload support.

| Parameter | Default | Description |
|---|---|---|
| `conf.port` | `80` / `443` | Listening port |
| `conf.hostname` | `"0.0.0.0"` | Bind address |
| `conf.development` | `true` | Enables Bun development mode |
| `conf.https` | — | `"generate"` to auto-generate a self-signed cert, or `{ cert, key }` |
| `conf.idleTimeout` | `10` | Idle connection timeout in seconds passed to `Bun.serve()` |
| `accessLog` | built-in | Custom access log function `(duration, req, res?) => void` |
| `readLogs` | — | Optional: `() => Promise<string>` — supplies log content to WebSocket clients |
| `watchLogs` | — | Optional: `(onChange: () => void) => void` — notifies when logs change |

`readLogs` and `watchLogs` are injected externally, keeping the server decoupled from any specific logging implementation. Both must be provided together for live log streaming to work.

Returns a `sendReload` function that pushes a `RELOAD` message to all connected WebSocket clients.

---

### `createClient(baseUrl, options?)` *(client-side)*

Factory that returns a type-safe HTTP client bound to a base URL and shared options.

```typescript
import { createClient, define } from "switchboard/client";

const call = createClient("https://api.example.com", {
  onUnauthorized: () => { /* e.g. redirect to login */ },
});

const result = await call(listRoute, { search: "burger" });
```

#### `ClientOptions`

| Field | Description |
|---|---|
| `onUnauthorized` | Callback invoked when the server returns `401`; use to redirect to login |
| `onForbidden` | Callback invoked when the server returns `403`; use to show an access-denied message |
| `withCredentials` | Send cookies on every call made by this client (can be overridden per-call via `CallSettings`) |

#### `call(route, params, body?, settings?)`

`createClient` returns a type-safe fetch function directly. For bodyless methods (`get`, `delete`, etc.) the signature is `call(route, params, settings?)`; for body methods (`post`, `put`, `patch`) it is `call(route, params, body, settings?)`.

#### `settings`

| Field | Description |
|---|---|
| `authTokenOverride` | Set to a bearer token string to add an `Authorization` header; `null` to send no auth |
| `methodOverride` | Override the HTTP method string |
| `validateReturn` | Set to `false` to skip Zod parsing of the response (default: `true`) |
| `baseUrlOverride` | Override the base URL for this individual call |
| `withCredentials` | Send cookies with the request |

Auth is only applied when `authTokenOverride` is explicitly set to a non-null string. There is no implicit token source.

---

## Path parameter conventions

Path segments starting with `:` are extracted as route parameters. Segments ending in `Id` (camelCase) or `_id` (snake_case) are parsed as numbers; all others are kept as strings.

```
/orders/:orderId          → { orderId: z.number() }
/orders/:order_id         → { order_id: z.number() }
/users/:username          → { username: z.string() }
/items/:itemId?           → { itemId: z.optional(z.number()) }
```

---

## Internals

### Routing algorithm

Route lookup uses a **trie** (prefix tree) keyed by path segment. Each node in the trie represents one segment; the root's children are the first segments of all registered paths. Matching walks the trie one segment at a time, giving **O(segments)** lookup regardless of how many routes are registered.

At each trie level, static segments are tried before parameter segments (`:param`), so `/users/settings` will always win over `/users/:id` when both are registered.

### Why not Bun's built-in router?

Bun ships two routing facilities that were considered:

- **`Bun.FileSystemRouter`** — designed for Next.js-style file-system conventions (maps files on disk to URL paths). It is not a general-purpose path matcher and requires a directory of files to scan.
- **`routes` option of `Bun.serve`** — pattern matching is baked into the server instance itself and cannot be used as a standalone path-matching primitive outside of a `Bun.serve` call.

Neither can be used as a drop-in path matcher decoupled from a specific server setup, so the trie implementation is used instead. It provides the same O(segments) performance with no external dependencies.

---

## Known Limitations

- **Form body nested objects**: POST/PUT/PATCH bodies sent as `application/x-www-form-urlencoded` with dot-notation keys (e.g. `address.city=London`) are not parsed into nested objects — only query params support this. Use `application/json` bodies if nesting is needed.

- **Route parser types**: The form/query string parser (`routeParser.ts`) uses several internal `any` casts; type safety there is weaker than the rest of the library.

- **Union schema edge case**: When a `ZodUnion` contains both a `string` member and a type that can be parsed from a string, the parser may coerce the string value, potentially invalidating it.

- **`defToUrl` empty arrays**: Serialising a parameter whose value is an empty array omits the key entirely from the URL rather than preserving an explicit empty state.

- **TLS cert domain change**: The auto-generated self-signed cert (`"generate"` option) is cached in `.genCert`. If the domain name changes, delete that file manually to force regeneration.

- **Query string type-level validation**: The `ValidateOptionalUrl` type constraint only validates path segment syntax. It does not validate query string parameter definitions at the type level.
