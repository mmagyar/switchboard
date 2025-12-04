import type { ZodType, z } from "zod";
import type { HTTPMethods } from "./staticDefs.ts";
import { type Route } from "./routeDef.ts";
import { defToUrl } from "./urlUtils.ts";

export const config = {
  baseUrl: "/",
};
const getSessionTokens = () => {
  return { tokens: { access_token: "" } };
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

  //Assume tokens are kept up to date
  let auth: { Authorization: string } | null = null;
  if (typeof settings.authTokenOverride === "undefined") {
    const { tokens } = getSessionTokens();
    auth = tokens ? { Authorization: `Bearer ${tokens.access_token}` } : null;
  } else if (settings.authTokenOverride !== null) {
    auth = { Authorization: settings.authTokenOverride };
  }

  const response = await fetch(`${settings.baseUrlOverride ?? config.baseUrl}${fullPath}`, {
    method: settings?.methodOverride ?? route.method.toUpperCase(),
    headers: {
      ...auth,
      "Content-Type": "application/json",
    },
    body: body ? JSON.stringify(body) : undefined,
    credentials: settings.withCredentials ? "include" : undefined,
  }).then(async (response) => {
    if (response.status >= 400) {
      if (
        response.status === 401 &&
        !location.pathname.startsWith("/login") &&
        !location.pathname.startsWith("/auth")
      ) {
        //       Redirect to authorize Revise / test if it causes a problem
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
