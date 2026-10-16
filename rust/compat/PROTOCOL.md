# Differential protocol v1

This directory contains test tooling only. The product remains pure Rust. `reference.ts` imports the unchanged Bun implementation through `src/index.ts` and `src/clientExport.ts`. `protocol.ts` is the executable boundary specification, validated with Zod. `cases.ndjson` is the shared corpus. No expected result snapshots replace either implementation.

Run from the repository root:

```sh
bun rust/compat/reference.ts < rust/compat/cases.ndjson
bun rust/compat/check.ts ./rust/target/release/compat-runner
# Additional arguments are passed verbatim, without a shell:
bun rust/compat/check.ts bun rust/compat/reference.ts
bun run check
bunx tsc -p rust/compat/tsconfig.json
bun test
```

## Framing

Both runners read UTF-8 NDJSON on stdin and emit exactly one result object per nonblank input line, in input order, to stdout. No banners or diagnostics may appear on stdout. Each case is independent. Empty lines are ignored. IDs must be unique within the corpus. Diagnostics go to stderr. Invalid protocol input exits nonzero; it is not a product failure result. A runner must not suppress cases, retry cases, or emit partial success after crashing.

`check.ts <runner executable> [arguments...]` starts the reference and candidate with the same corpus bytes, a 30-second deadline per process, and `VERBOSE_ERROR_OUTPUT=false`. It validates both result streams, IDs/order/count and HTTP response counts, then compares all result fields. JSON object member order is immaterial. Array order, JSON types, string contents (including validation whitespace), headers, warning issues and complete error messages are significant. Missing fields are significant. Mismatches print both complete results. Nonzero process exits, invalid output, missing/extra results and differences all fail. It reports the corpus count and mismatched case count. A runner/protocol failure reports all cases as unverified mismatches and adds `runnerFailures:1`.

## Schema DSL

Schemas use Rust's adjacently tagged representation, with `type` and `value` keys. Unit variants omit `value`:

```json
{"type":"unknown"}
{"type":"string"}
{"type":"number"}
{"type":"boolean"}
{"type":"file"}
{"type":"literal","value":"fixed"}
{"type":"enum","value":["red","blue"]}
{"type":"object","value":{"id":{"type":"number"}}}
{"type":"array","value":{"type":"number"}}
{"type":"union","value":[{"type":"number"},{"type":"object","value":{}}]}
{"type":"optional","value":{"type":"string"}}
{"type":"nullable","value":{"type":"string"}}
{"type":"default","value":[{"type":"number"},7]}
{"type":"catch","value":[{"type":"number"},7]}
{"type":"readonly","value":{"type":"string"}}
```

The tuple `value` of default/catch is `[innerSchema, fallbackJson]`. Absent input and explicit null remain distinct. Object schemas strip unknown properties according to the reference. A literal accepts any JSON value in the wire schema; the reference rejects nonprimitive literals as unsupported protocol input. Enums and unions must be nonempty and have at least two entries respectively.

## Cases

Every record has `id` and `kind`:

* `http`: `routes`, `requests`, and optional `user` (default null), `auth` (`ok`, `forbidden`, `unauthenticated`; default `ok`), `errorParser` (`{status,message}`), `htmlErrors` (default false), `defaultResponse` (`{status,text,headers?}`). The configured auth result applies to every route, before request validation. `errorParser` always returns its configured result, including for built-in thrown errors. HTML errors use exactly `<error status="STATUS">MESSAGE</error>` with the reference message unchanged.
* `definition`: `routes`. Defines and registers in order. Returns first definition/registration error, or success. State after failure is discarded.
* `url`: `route`, `params`. Defines the route, then calls exported `defToUrl`. Does not register the route.

A route has `method` (lowercase get/post/put/patch/delete/head/options), `path`, optional `permission` (default `public`), `params`, `body`, `output`, `prefixes`, `handler`, `format`. Omitted params invokes auto-schema. Omitted body/output uses unknown. HEAD has no builder in the reference; its definition is a GET builder's validated definition with only `method` set to `head` before binding. This exercises the exported router's explicit HEAD support without modifying source.

Handlers:

* Omitted or `{kind:"echo"}` returns `{params,body,user,prefix}`. Absent body/prefix becomes JSON null. User is the case's value (default null).
* `{kind:"value",value:JSON}` returns the exact value.
* `{kind:"deferred",value:NODE}` materializes deferred nodes recursively. Nodes are `{type:"ready",value:JSON}`, `{type:"future",value:NODE}`, `{type:"object",value:{key:NODE}}`, `{type:"array",value:[NODE]}`. Futures resolve asynchronously through a Promise microtask, without wall-clock sleeps.
* `{kind:"throw",error:"request"|"notFound"|"unauthorized"|"generic",status?,message?}` throws the corresponding original error. Defaults are status 422/message `fixture error` for request, status 401/message `fixture error` for unauthorized, message `fixture error` for generic. Unauthorized status must be 401 or 403. Not-found ignores message/status.

Formats:

* `{kind:"constant",text?,headers?,status?,redirect?:true}` returns configured bytes/headers with the reference's default status/303 behavior. Omitted text creates a null body, useful for DELETE/204.
* `{kind:"inspectDeferred"}` walks the raw value before resolution and returns JSON `{pending:[PATH...]}` with application/json. Paths use dot-separated object keys or array indices; a root Promise has path `""`. It does not await deferred values and stops traversal at a Promise. This verifies unresolved formatter input, not network chunk timing.

Requests have `method` (uppercase HTTP method, including unsupported methods), absolute `url`, optional string-map `headers` and optional raw string `body`. A form request supplies its real encoded bytes and content-type. The reference creates a real `Request` and dispatches through `Router.handleRequest`. Requests execute sequentially, sharing only the case's immutable route definitions.

## Results

Success examples:

```json
{"id":"one","kind":"http","responses":[{"status":200,"headers":[["content-type","application/json"]],"body":{"type":"json","value":{"ok":true}}}],"warnings":[],"errors":[]}
{"id":"two","kind":"definition"}
{"id":"three","kind":"url","url":"/items/4?q=yes"}
{"id":"four","kind":"error","error":{"name":"Error","message":"Duplicate route: GET /items"}}
```

Application headers are lowercase and sorted lexicographically by name, then value. Only `date`, `connection`, `keep-alive` are ignored as transport metadata. Content type and length, custom headers, cookies, location and all other headers remain. Body is parsed as JSON only if content-type contains `json` and bytes are nonempty. Empty JSON responses (HEAD) use `{type:"text",value:""}`. JSON string error bodies retain their string type. Non-JSON bodies are exact UTF-8 strings.

Each warning is `{message,issues,data,method,url}` with the complete Zod error message and complete issue array, JSON.stringify snapshot of the original invalid data (nested Promises and Files become `{}`), lowercase method and effective request URL (rewritten for prefixes, suffixed with `#field` for formatter property validation). The runner drains all fixture futures and background validation microtasks before taking its warning snapshot. Warning order is observable.

`errors` captures each call to the configured error logger as a JSON-encoded string of its arguments. Error objects are represented as `{name,message}`. Stack traces are excluded at this logging boundary because they describe runner locations, not product behavior. All other arguments are retained as JSON. No validation or response strings are normalized. Production verbose stacks are disabled explicitly in both processes.

## Coverage and limits

The corpus covers methods, explicit/fallback HEAD, misses/custom defaults, static-first fallback, optional/trailing paths, prefix rewriting (including form bodies and regex metacharacters), nested/repeated/indexed queries, path precedence, defaults/catch/readonly/nullable/unions/enums/literals, JSON and flat form bodies, auth precedence, error negotiation/parsers/HTML, custom formatting/redirects, output stripping/warnings, nested deferred objects/arrays and formatter pending values, route conflicts and URL generation. Invalid cases intentionally retain all reference validation details.

Bun 1.4.0 has observable runtime-specific behavior: arbitrary unknown request methods such as BREW become GET when constructing a Request (TRACE remains TRACE and tests 405); an in-memory DELETE 204 can contain JSON bytes; encoded path segments remain percent-encoded in handler params; an empty prefix successfully registers. These observations remain visible in the corpus instead of being silently corrected to conventional HTTP behavior.

The corpus does not claim network-level streaming timing, rejected futures, client transport errors, TLS, WebSockets, browser behavior or safe sparse-array stress coverage. Those require separate product tests or a versioned extension. Sparse query cases here use small bounded indices. The optional full-HTTP benchmark is not part of this protocol. No performance numbers are fabricated by this tooling.
