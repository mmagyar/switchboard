import type { ZodType, z } from "zod";
import type { HTTPMethods, HTTPMethodsWithBody } from "./staticDefs.ts";
import { type Route } from "./routeDef.ts";
import { defToUrl } from "./urlUtils.ts";

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

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

export const createClient =
  (baseUrl: string, options: ClientOptions = {}) =>
  async <
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
  ): Promise<z.infer<OUT>> => {
    const fullPath = defToUrl(route, params);

    let auth: { Authorization: string } | null = null;
    if (settings.authTokenOverride != null) {
      auth = { Authorization: settings.authTokenOverride };
    }

    const response = await fetch(`${settings.baseUrlOverride ?? baseUrl}${fullPath}`, {
      method: settings.methodOverride ?? route.method.toUpperCase(),
      headers: {
        ...auth,
        ...(body ? { "Content-Type": "application/json" } : {}),
        Accept: "application/json",
      },
      body: body ? JSON.stringify(body) : undefined,
      credentials: (settings.withCredentials ?? options.withCredentials) ? "include" : undefined,
    }).then(async (response) => {
      if (response.status >= 400) {
        if (response.status === 401) {
          options.onUnauthorized?.();
        }
        if (response.status === 403) {
          options.onForbidden?.();
        }
        const errDefault = `Unknown error, status: ${response.status}`;
        let errorText = errDefault;
        try {
          const json = await response.json();
          errorText = JSON.stringify(json);
        } catch {
          errorText = await response.text().catch(() => errDefault);
        }
        throw new ApiError(response.status, errorText);
      } else {
        const effectiveMethod = (settings.methodOverride ?? route.method).toLowerCase();
        if (
          effectiveMethod === "delete" ||
          effectiveMethod === "head" ||
          response.status === 204 ||
          response.status === 205
        ) {
          return undefined;
        }
        const contentType = response.headers.get("Content-Type") ?? "";
        if (contentType.includes("application/json")) {
          return response.json();
        }
        const text = await response.text();
        return text.length > 0 ? text : undefined;
      }
    });

    if (settings.validateReturn !== false && response !== undefined) {
      return route.outputValidation.parse(response);
    }

    return response;
  };
