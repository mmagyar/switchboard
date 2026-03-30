import type { ZodType, z } from "zod";
import type { HTTPMethods } from "./staticDefs.ts";
import { type Route } from "./routeDef.ts";
import { defToUrl } from "./urlUtils.ts";

const callConfig = {
  baseUrl: "/",
};

export const setBaseUrl = (baseUrl: string) => {
  callConfig.baseUrl = baseUrl;
};

let clientConfig: { onUnauthorized?: () => void } = {};

export const configureClient = (options: { onUnauthorized?: () => void }): void => {
  clientConfig = { ...clientConfig, ...options };
};

export const call = async <
  METHOD extends HTTPMethods,
  PATH extends string,
  PERMISSION,
  PARAMS extends ZodType,
  BODY extends ZodType,
  OUT extends ZodType,
>(
  route: Route<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT>,
  params: z.infer<PARAMS>,
  body?: z.infer<BODY>,
  settings: {
    methodOverride?: string;
    validateReturn?: boolean;
    authTokenOverride?: string | null;
    baseUrlOverride?: string;
    withCredentials?: boolean;
  } = { validateReturn: true },
): Promise<z.infer<OUT>> => {
  const fullPath = defToUrl(route, params);

  let auth: { Authorization: string } | null = null;
  if (settings.authTokenOverride != null) {
    auth = { Authorization: settings.authTokenOverride };
  }

  const response = await fetch(`${settings.baseUrlOverride ?? callConfig.baseUrl}${fullPath}`, {
    method: settings?.methodOverride ?? route.method.toUpperCase(),
    headers: {
      ...auth,
      ...(body ? { "Content-Type": "application/json" } : {}),
      Accept: "application/json",
    },
    body: body ? JSON.stringify(body) : undefined,
    credentials: settings.withCredentials ? "include" : undefined,
  }).then(async (response) => {
    if (response.status >= 400) {
      if (response.status === 401) {
        clientConfig.onUnauthorized?.();
      }
      const errDefault = `Unknown error, status: ${response.status}`;
      let err = errDefault;
      try {
        err = await response.json();
      } catch (e) {
        err = await response.text().catch(() => errDefault);
      }
      throw err;
    } else {
      return route.method !== "delete" ? response.json() : undefined;
    }
  });

  if (!(settings.validateReturn === false || route.method === "delete")) {
    return route.outputValidation.parse(response) as z.infer<OUT>;
  }

  return response;
};
