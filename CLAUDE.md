# Project Rules & Preferences

## Agent Behavior & Output Rules

- **Code over conversation:** Do not apologize, do not write transitional filler phrases (e.g., "Here is the updated code..."). Output only the necessary explanations and the code blocks.
- **Thinking:** You may use thinking tokens to plan complex architecture or debug, but keep the final output strictly focused on the solution.
- **Precision:** When modifying files, only output the specific code blocks that need changing with enough surrounding context to find the insertion point. Do not rewrite entire files unless requested.
- **Parallelism:** Always split work across as many agents in parallel as possible. If a task touches multiple files that don't depend on each other, spawn one agent per file/concern and run them concurrently. Only work sequentially when there is an explicit data dependency between steps.

## Working Directory

The repo root is `switchboard/`. All source lives in `src/`. There is no sub-application directory — work directly in the repo root.

## Commands (How to verify your work)

```sh
bun run check   # type-check with tsc --noEmit (zero errors required)
bun test        # run all *.spec.ts tests
```

Run both before considering any change complete. Fix type errors before running tests.

## Code Style & Conventions

- **Simplest solution wins.** Before writing code, ask: what is the minimum amount of code that correctly solves this? Prefer deleting code over adding it.
- **Exports:** Use named exports exclusively (`export const foo = ...`). Avoid default exports — they break refactoring and make imports ambiguous.
- **No `any`.** Never use `any` in TypeScript. If the type is genuinely dynamic, use `unknown` and narrow it.
- **No type casting to lie to the compiler.** Do not use `as SomeType` to silence a type error. The only accepted exception is branding a validated primitive (e.g. after a Zod `.safeParse()` confirms the format), where the cast carries zero runtime risk and encodes a real semantic guarantee.
- **Full type safety throughout.** All function signatures, return types, and data structures must be explicitly and correctly typed.
- **No `exactOptionalPropertyTypes`** — it is explicitly `false` in `tsconfig.json`. Do not write code that depends on it being enabled.
- **Strict null checks are on.** Do not assume values are non-null without proof.

## External Data & Validation

All data that crosses a trust boundary must be validated with Zod at the boundary. This includes request params, query strings, bodies, and anything from `JSON.parse`.

**Never use `z.custom<T>()`** — it performs zero runtime validation and is indistinguishable from `z.unknown()`. Write the actual `z.object({...})` schema for every field.

**Never use `z.any()`** — use `z.unknown()` if the shape is genuinely opaque.

Output schema mismatches emit a console warning (not an error) via `outputErrorWarning` so the app keeps running, but schemas should still be accurate.

## Architecture

### Stack

- Runtime: **Bun**
- Language: **TypeScript** (strict mode — see `tsconfig.json`)
- Validation: **Zod**
- Tests: **Bun's built-in test runner** (`bun test`, files named `*.spec.ts`)

This is a **library**, not an application. It has no database, no rendering framework, no client-side JS. Those are concerns of the consumer application.

### Core Concepts

#### Route Definition — `define<PERMISSION>()`

`define` is a generic factory. Call it once with your permission type, then use the returned builder to define typed routes:

```ts
const def = define<"public" | "user" | "admin">();

const myRoute = def.get(
  "/items/:id",        // path — mandatory params ending in "Id" become z.number(), others z.string()
  "user",              // permission
  outputSchema,        // Zod schema — inferred return type of the handler
  paramsSchema,        // optional override; if omitted, auto-derived from path
);
```

Methods available: `.get()`, `.del()`, `.options()`, `.post()`, `.put()`, `.patch()`

Body methods (`.post()`, `.put()`, `.patch()`) take `bodyValidation` as the third argument, before `outputValidation`.

Path params are validated at definition time — every `:param` in the path must appear as a key in the params schema (if one is provided), or an error is thrown immediately.

#### Handler Wrapping — `RouteHandlerDefiner`

`RouteHandlerDefiner` is instantiated once with your auth logic and returns the `define`-style handler function:

```ts
const rhd = RouteHandlerDefiner(
  async (user, permissionsNeeded, req) => { /* return "ok" | "forbidden" | "unauthenticated" */ },
  async (req) => { /* return USER from request */ },
  { outputErrorWarning, errorParser, errorHtmlFormatter, errorLogger }, // optional
);

export const myRoute = rhd(
  def.get("/items/:id", "user", outputSchema),
  async (params, user) => {
    // params is fully typed from the schema
    return { ... };
  },
  formatOutput, // optional: (data, user, req, params) => FormatOutputReturn
);
```

For body routes, the handler receives `(params, body, user)`.

If `formatOutput` is omitted, the handler output is JSON-serialised and returned automatically.

#### Router

```ts
const router = new Router(defaultHandler); // defaultHandler is optional (404 fallback)

// Register — method and path come from the route object; do not duplicate them
routes.forEach(r => router.addRoute(r.method, r.path, r.handlerWrapped));

// Dispatch
const response = await router.handleRequest(req);
```

`Router.addRoute` throws on duplicate `method + path` registrations. `HEAD` requests automatically fall back to the matching `GET` handler with a body-less response.

#### URL Parameter Auto-Schema

When no `paramsValidation` is passed to `def.get()` (etc.), `urlToZodSchema` auto-generates one:
- `:fooId` → `z.number()`
- `:foo` → `z.string()`
- `:fooId?` → `z.optional(z.number())`
- `:foo?` → `z.optional(z.string())`

Optional params must come after mandatory params in the path — this is enforced at route registration.

#### Streaming SSR Support

`FormatOutput` receives handler data **before all Promises are resolved**, enabling streaming. If a handler returns `{ title: Promise<string>, body: Promise<string> }`, `formatOutput` receives those Promises directly and can stream them. Zod validates each property in the background as it resolves.

If `formatOutput` is not provided, all Promises are awaited and the full result is validated and JSON-serialised.

#### `serveHotBuns`

A convenience wrapper around `Bun.serve` with hot-reload support for development. Accepts the same options as `Bun.serve` plus a `router` instance.

### Exports

The library has two entry points defined in `package.json`:

| Entry point | File | Contents |
|---|---|---|
| `switchboard/server` | `src/index.ts` | `define`, `RouteHandlerDefiner`, `Router`, `serveHotBuns`, error classes, types |
| `switchboard/client` | `src/clientExport.ts` | Client-safe utilities (no server-only imports) |

### Error Classes

| Class | Status | Use case |
|---|---|---|
| `NotFoundError` | 404 | Resource does not exist |
| `Unauthorized` | 401 or 403 | Auth failed (pass status in constructor) |
| `RequestError` | any | General client error with explicit status |

Throw any of these from a handler — `wrapHandler` catches them and returns the correct HTTP response.

## Single Source of Truth

- Route method and path live in the `def.*` call — do not repeat them in `router.addRoute`.
- The output Zod schema is the single source for the handler's return type — do not write a separate TypeScript interface that duplicates it.
- Tests live alongside source as `*.spec.ts` files — do not create a separate `tests/` directory.