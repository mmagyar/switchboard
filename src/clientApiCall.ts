import type { ZodType, z } from "zod";
import type { HTTPMethods, HTTPMethodsWithBody } from "./staticDefs.ts";
import { type Route } from "./routeDef.ts";
import { defToUrl } from "./urlUtils.ts";

/**
 * What failed, independent of `status`:
 * - `"http"`: the server answered with this status (including a genuine upstream 502).
 * - `"network"`: the request or response transport failed; `status` is `NETWORK_ERROR_STATUS`.
 * - `"invalid-response"`: the server answered 2xx but the payload was unusable;
 *   `status` is `INVALID_RESPONSE_STATUS`. This is what tells a client-detected bad
 *   body apart from a real gateway 502, which shares the numeric status.
 */
export type ApiErrorKind = "http" | "network" | "invalid-response";

export class ApiError extends Error {
  readonly status: number;
  readonly kind: ApiErrorKind;

  constructor(status: number, message: string, kind: ApiErrorKind = "http") {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.kind = kind;
  }
}

/** Sentinel `ApiError.status` used when the request never reached the server (e.g. offline, DNS failure, CORS). */
export const NETWORK_ERROR_STATUS = 0;

/**
 * `ApiError.status` used when the server answered successfully but the payload was unusable
 * (e.g. `Content-Type: application/json` with a truncated body). 502 keeps the error in the
 * `>= 400` range, so callers that branch on status class treat it as the failure it is.
 */
export const INVALID_RESPONSE_STATUS = 502;

export type ClientOptions = {
  onUnauthorized?: () => void;
  onForbidden?: () => void;
  withCredentials?: boolean;
};

export type CallSettings = {
  methodOverride?: string;
  validateReturn?: boolean;
  authTokenOverride?: string | null;
  baseUrlOverride?: string;
  withCredentials?: boolean;
};

export type ApiClient = {
  <
    METHOD extends HTTPMethods,
    PATH extends string,
    PERMISSION,
    PARAMS extends ZodType,
    BODY extends ZodType,
    OUT extends ZodType,
  >(
    route: Route<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT>,
    params: z.infer<PARAMS>,
    body: (METHOD extends HTTPMethodsWithBody ? z.infer<BODY> : undefined) | undefined,
    settings: CallSettings & { validateReturn: false },
  ): Promise<unknown>;
  <
    METHOD extends HTTPMethods,
    PATH extends string,
    PERMISSION,
    PARAMS extends ZodType,
    BODY extends ZodType,
    OUT extends ZodType,
  >(
    route: Route<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT>,
    params: z.infer<PARAMS>,
    body?: METHOD extends HTTPMethodsWithBody ? z.infer<BODY> : undefined,
    // `validateReturn?: true` forces a call that opts out to go through the overload above.
    // A settings object typed as plain `CallSettings` (validateReturn: boolean) matches neither
    // overload, which is correct: the return type cannot be known without narrowing it first.
    settings?: CallSettings & { validateReturn?: true },
    // `| undefined` is not pessimism: a DELETE/HEAD, a 204/205, an empty 200 body, or a
    // `methodOverride: "HEAD"` on any route all resolve to undefined by design, whatever
    // `outputValidation` declares. Declare the schema `.optional()` if that is expected.
  ): Promise<z.infer<OUT> | undefined>;
};

export const createClient = (baseUrl: string, options: ClientOptions = {}): ApiClient =>
  (async <
    METHOD extends HTTPMethods,
    PATH extends string,
    PERMISSION,
    PARAMS extends ZodType,
    BODY extends ZodType,
    OUT extends ZodType,
  >(
    route: Route<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT>,
    params: z.infer<PARAMS>,
    body?: METHOD extends HTTPMethodsWithBody ? z.infer<BODY> : undefined,
    settings: CallSettings = {},
  ): Promise<unknown> => {
    const fullPath = defToUrl(route, params);

    let auth: { Authorization: string } | null = null;
    if (settings.authTokenOverride != null) {
      auth = { Authorization: settings.authTokenOverride };
    }

    const describe = (error: unknown) => (error instanceof Error ? error.message : String(error));

    // `!== undefined` rather than truthiness: 0, false, "" and null are schema-valid bodies
    // and must be sent. Serialised outside the transport guard below — a non-serialisable
    // body (circular reference, BigInt) is a caller bug, not a network failure.
    const serializedBody = body !== undefined ? JSON.stringify(body) : undefined;

    // Only the transport itself is guarded here. Everything after this — including the
    // onUnauthorized/onForbidden callbacks — must be able to throw on its own terms, or a
    // failure in consumer code would be reported as a bogus network error and lose the status.
    let raw: Response;
    try {
      raw = await fetch(`${settings.baseUrlOverride ?? baseUrl}${fullPath}`, {
        method: settings.methodOverride ?? route.method.toUpperCase(),
        headers: {
          ...auth,
          ...(serializedBody !== undefined ? { "Content-Type": "application/json" } : {}),
          Accept: "application/json",
        },
        body: serializedBody,
        credentials: (settings.withCredentials ?? options.withCredentials) ? "include" : undefined,
      });
    } catch (error) {
      throw new ApiError(NETWORK_ERROR_STATUS, `Network request failed: ${describe(error)}`, "network");
    }

    if (raw.status >= 400) {
      if (raw.status === 401) {
        options.onUnauthorized?.();
      }
      if (raw.status === 403) {
        options.onForbidden?.();
      }
      // Read the body once. Calling .json() first and falling back to .text() cannot work:
      // .json() consumes the stream, so the fallback always fails and the detail is lost.
      let errorText = `Unknown error, status: ${raw.status}`;
      try {
        const bodyText = await raw.text();
        if (bodyText !== "") {
          try {
            errorText = JSON.stringify(JSON.parse(bodyText));
          } catch {
            errorText = bodyText;
          }
        }
      } catch {
        // body unreadable — keep the status-only default
      }
      throw new ApiError(raw.status, errorText);
    }

    const effectiveMethod = (settings.methodOverride ?? route.method).toLowerCase();
    const noBodyExpected = effectiveMethod === "delete" || effectiveMethod === "head";

    let response: unknown;
    if (noBodyExpected || raw.status === 204 || raw.status === 205) {
      response = undefined;
    } else {
      const contentType = raw.headers.get("Content-Type") ?? "";
      if (contentType.includes("application/json")) {
        try {
          response = await raw.json();
        } catch (error) {
          throw new ApiError(
            INVALID_RESPONSE_STATUS,
            `Response body is not valid JSON: ${describe(error)}`,
            "invalid-response",
          );
        }
      } else {
        let text: string;
        try {
          text = await raw.text();
        } catch (error) {
          throw new ApiError(NETWORK_ERROR_STATUS, `Failed to read response body: ${describe(error)}`, "network");
        }
        response = text.length > 0 ? text : undefined;
      }
    }

    // validateReturn: false means the caller opted out — the ApiClient overload above types
    // this call's result as `unknown` rather than lying about it.
    // An absent body is the documented empty-response contract, not data to validate; the
    // overload's `| undefined` is what makes returning it here honest.
    if (settings.validateReturn === false || response === undefined) return response;

    return route.outputValidation.parse(response);
  });
