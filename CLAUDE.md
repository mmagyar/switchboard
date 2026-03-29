# Project Rules & Preferences

## Working Directory

Always work inside `slx/bun/`. That is the active application. Ignore `slx/remix/` — it is a legacy project.

## Parallelism

Always split work across as many agents in parallel as possible. If a task touches multiple files that don't depend on each other, spawn one agent per file/concern and run them concurrently. Only work sequentially when there is an explicit data dependency between steps.

## Code Style

- **Simplest solution wins.** Before writing code, ask: what is the minimum amount of code that correctly solves this? Prefer deleting code over adding it.
- **No `any`.** Never use `any` in TypeScript. If you don't know the type yet, use `unknown` and narrow it.
- **No type casting to lie to the compiler.** Do not use `as SomeType` to silence a type error. The only accepted exception is branding a validated primitive (e.g. `s as UUID` after `z.string().uuid()` has confirmed the format), where the cast carries zero runtime risk and encodes a real semantic guarantee.
- **Full type safety throughout.** All function signatures, return types, and data structures must be explicitly and correctly typed.

## External Data & Validation

All data that crosses a trust boundary must be validated with Zod at the boundary. This includes:

- HTTP request params, query strings, and bodies (handled by switchboard, but the schemas you pass must be real)
- Database query results
- Third-party API responses
- Anything parsed from `JSON.parse`

**Never use `z.custom<T>()`** — it performs zero runtime validation and is indistinguishable from `z.unknown()`. Write the actual `z.object({...})` schema for every field.

**Never use `z.any()`** — use `z.unknown()` if the shape is genuinely opaque.

### Zod patterns in this codebase

- `z.string().uuid().transform(s => s as UUID)` — validates UUID format, preserves the branded type
- `z.string().min(1).transform(s => s as UUID62)` — UUID62 is base-62; no format validator exists, so non-empty string is the correct check
- `z.date()` — postgres rows return real `Date` objects; use this for timestamp/date columns
- `z.string().nullable()` — for `string | null` DB columns
- `z.union([schema, z.undefined()])` — for a required key whose value may be `undefined` (distinct from `.optional()` which makes the key itself absent, relevant because `exactOptionalPropertyTypes` is enabled in tsconfig)
- Output schema mismatches emit a console warning (not an error) so the app keeps running, but schemas should still be correct

## Architecture

### Stack

- Runtime: **Bun**
- Rendering: **React** (`renderToString` / `renderToStaticMarkup`) — SSR only, no client-side React
- Router: **switchboard** (bespoke library, see below)
- Database: **postgres** tagged-template client (`db\`SELECT ...\``)
- Validation: **Zod**
- Client JS: small hand-written TypeScript bundles built on demand with `Bun.build()`, served at `/scripts/*.js`

### Switchboard

Switchboard is the routing and handler library. The route definition is the **single source of truth** for the method and path — never repeat them at registration time.

```ts
// 1. Define the route: method, path, permission, output schema, optional params schema
export const myRoute = rhd(
  def.get("/my-path", "public", outputSchema, paramsSchema),
  async (params, user): Promise<OutputType> => {
    // business logic — return data matching outputSchema
    return { ... };
  },
  formatOutput, // e.g. renderHtml(MyView, meta) or renderOrRedirect(MyView)
);

// 2. Collect all routes and register — method and path come from the route itself
const routes = [myRoute, otherRoute, ...];
routes.forEach(r => router.addRoute(r.method, r.path, r.handlerWrapped));
```

**Do not** pass the method string and path string again to `router.addRoute` — the route object already carries them. Duplicating them is a violation of single source of truth and will silently diverge.

**Every page route is simultaneously an HTML endpoint and a JSON API.** The `renderHtml` and `renderOrRedirect` helpers check the `Accept` header automatically:
- `Accept: text/html` (browser) → React renders to a full HTML document
- `Accept: application/json` (fetch/API client) → raw handler data returned as JSON, React skipped

You never need a separate API endpoint for data that a page already returns. Fetch the page route with `Accept: application/json`.

Permission levels: `"public"` | `"user"` | `"admin"`.

### Format output helpers (`src/renderer.tsx`)

| Helper | Use case |
|---|---|
| `renderHtml(View, meta)` | Route always renders HTML; never redirects |
| `renderOrRedirect(View)` | Route may return `{ _type: "redirect", _url: "..." }` or props |
| `redirectTo(fn)` | Route always redirects; URL computed from handler output |

The output schema's inferred TypeScript type flows into the format function's `data` parameter. Keep schemas accurate so this stays type-safe.

`PageOutput<T>` is the return type for handlers that may redirect: `T | RedirectOutput`.

Use `redirectOutput(url)` to construct the redirect branch and `redirectOutputSchema` in the union schema.

### Locale

- Supported locales: `"en"` and `"hu"` (typed as the branded `Lang`)
- Locale is derived **from the URL path** via the `X-Url-Locale` header set by the locale-stripping middleware — not from cookies
- Non-English paths have a `/<lang>/` prefix (e.g. `/hu/plans`)
- Use `addLocaleToPath(path, locale)` when building internal links
- OAuth state carries the locale across the Spotify redirect round-trip (`loginUrl(locale)`)

### Client-side JS

- Each page that needs JS renders its own `<script defer src="/scripts/foo.js" />` tag directly in its view component
- Scripts are built from `src/client/*.ts` via `Bun.build()`
- Prefer **progressive enhancement**: the page must work without JS; JS is an enhancement only
- Prefer **HTML-over-the-wire** for dynamic updates: fetch the same SSR page, parse with `DOMParser`, swap the relevant DOM section. Avoid manual DOM building in JS.
- Keep client scripts as short as possible — the SSR already knows how to render every state

### `exactOptionalPropertyTypes`

`tsconfig.json` has `exactOptionalPropertyTypes: true`. This means:
- `{ foo?: string }` — key may be absent
- `{ foo: string | undefined }` — key must be present, value may be undefined

These are **not interchangeable**. Use `z.union([schema, z.undefined()])` (not `.optional()`) when the TypeScript type has `: T | undefined` (required key).

## Single Source of Truth

Avoid duplicating information. If something is already expressed in one place, read it from there rather than restating it:
- Route method and path live in the `def.*` call — do not repeat them in `router.addRoute`
- Locale comes from the URL — do not also store it in a cookie
- SSR already knows how to render every page state — do not reimplement that logic in client JS
