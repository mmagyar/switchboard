# Changelog

## 0.2.0

Pre-1.0 minor release. No runtime API was removed, but two type signatures were tightened
and one definition-time check got stricter. Both are surfaced below with migrations.

### Breaking: stricter ambiguous-union detection on params schemas

`assertNoAmbiguousUnions` now looks through `.default()`, `.nullable()`, `.catch()`,
`.transform()`/`.pipe()` and `.readonly()` wrappers when deciding whether a union branch can
receive a raw string from a URL. Schemas that previously passed and parsed unpredictably now
throw at definition time.

```ts
// Threw nothing before, throws now — the coercion pre-pass cannot pick a branch
def.get("/x", "public", out, z.object({ v: z.union([z.string(), z.number().default(5)]) }));
```

**Migration:** collapse the union to the single branch a URL string can produce, or move the
alternative behind an explicit `.transform()` on one branch. This only applies to a
`paramsValidation` schema you pass explicitly — auto-derived param schemas and body schemas are
unaffected.

### Breaking (types): `createClient` calls resolve to `z.infer<OUT> | undefined`

The client has always returned `undefined` for a DELETE/HEAD, a 204/205, an empty 200 body, or
any call with `methodOverride: "HEAD"` — regardless of the declared output schema. The old
signature claimed `Promise<z.infer<OUT>>`, so that `undefined` arrived typed as present data.

**Migration:** narrow at the call site, or declare the schema `.optional()` if an empty response
is expected for that route. Runtime behaviour is unchanged.

### Breaking (types): opting out of validation must be statically visible

The `validateReturn: false` overload returns `Promise<unknown>`; the default overload now accepts
only `validateReturn?: true`. A settings object typed as plain `CallSettings` (where
`validateReturn` is `boolean`) matches neither overload.

**Migration:** pass the literal (`{ validateReturn: false }`), or branch so each path passes a
statically-known value. Previously such a call type-checked as validated output while returning
raw, unvalidated JSON.

### Dependencies

- typescript 7.0.2 (the native compiler — used for `tsc --noEmit` only; note TS 7 no longer
  ships `tsserver`, so editors using the workspace TypeScript for IntelliSense need the native
  language server / an updated TypeScript extension), @types/bun 1.3.14, zod 4.4.3.

### Fixed

- `createClient` no longer reclassifies non-network failures as network errors. Exceptions thrown
  by `onUnauthorized`/`onForbidden` callbacks propagate unchanged instead of surfacing as
  `ApiError(0)` and discarding the real status. Likewise, a body that cannot be serialised
  (circular reference, BigInt) now throws the underlying `TypeError` before any fetch is
  attempted, instead of a bogus network `ApiError`.
- `createClient` now sends schema-valid falsy bodies (`0`, `false`, `""`, `null`). Previously a
  truthiness check silently dropped them, producing a request with no body and no `Content-Type`.
- `ApiError` carries a `kind` (`"http" | "network" | "invalid-response"`), so a client-detected
  unusable 2xx payload (`INVALID_RESPONSE_STATUS`, 502) is distinguishable from a genuine
  upstream 502.
- An unparseable JSON body now throws `ApiError(INVALID_RESPONSE_STATUS /* 502 */)` instead of an
  `ApiError` carrying the response's own 2xx status, which defeated `status >= 400` checks.
- `NETWORK_ERROR_STATUS`, `INVALID_RESPONSE_STATUS`, and the `ApiClient`, `ClientOptions` and
  `CallSettings` types are now exported from both `switchboard/client` and `switchboard/server`.
- Handler results are resolved for Promises at any depth, in arrays and plain objects alike.
  Previously `[[Promise]]` and other nested shapes reached `JSON.stringify` unresolved and
  serialised as `{}`. A cheap synchronous pre-scan keeps fully-materialised responses from
  allocating a Promise per element.
- `serveHotBuns` returns a reload function that now also carries `.stop()`, releasing the log
  file watcher and both servers. Open hot-reload WebSockets are closed explicitly so the stop
  cannot hang on them. Calling the returned value directly still broadcasts a reload.
- A failed `serveHotBuns` start (e.g. port in use) no longer leaks the already-started :80
  redirect server or the log file watcher.
- Direct Promise-valued properties on class instances (non-plain objects) in handler results are
  still resolved, as in 0.1.x. Promise-free instances (`Date`, `Map`, DTOs with a `toJSON`) pass
  through untouched, keeping their prototype.
- `https: "generate"` now throws a clear error when openssl is missing or produces no key/cert
  pair, instead of handing `Bun.serve` a TLS config with `undefined` fields. A cached `.genCert`
  that no longer parses is regenerated rather than propagated.
