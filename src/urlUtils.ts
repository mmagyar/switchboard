import { forEach, map } from "./util.ts";
import type {
  FilterByIdEnding,
  PathToAllKeys,
  PathToMandatoryKeys,
  PathToOptionalKeys,
  ValidateOptionalUrl,
} from "./urlType.ts";
import type { z, ZodType, ZodTypeAny } from "zod";
import type { RouteBase } from "./routeBaseType.ts";

/**
 * Strips the declared query string from a route definition string.
 * A `?` followed by anything other than `/` or end-of-string is treated as
 * the query-string separator; everything from that `?` onwards is removed.
 * Optional path-param markers (`:param?`) are preserved because their `?`
 * is followed by `/` or end-of-string.
 */
export const stripQueryFromRoute = (url: string): string => {
  for (let i = 0; i < url.length; i++) {
    if (url[i] === "?") {
      const next = url[i + 1];
      if (next === undefined || next === "/") continue;
      return url.slice(0, i);
    }
  }
  return url;
};

export const decomposeUrl = (route: string): string[] => {
  const pathOnly = stripQueryFromRoute(route);
  if (pathOnly.endsWith("/")) {
    return pathOnly.substring(0, pathOnly.length - 1).split("/");
  }
  return pathOnly.split("/");
};

export const checkRouteOptionalParameterOrder = <T extends string>(route: T): route is ValidateOptionalUrl<T> => {
  const routeParts = decomposeUrl(route);
  let hasOptional = false;
  for (let i = 0; i < routeParts.length; i++) {
    if (routeParts[i]?.endsWith("?")) {
      hasOptional = true;
    } else if (hasOptional) {
      throw new Error("Cannot have a non optional parameter after an optional parameter");
    }
  }
  return true;
};

export const extractOptionalParamNames = <T extends string>(route: ValidateOptionalUrl<T>): PathToOptionalKeys<T> => {
  checkRouteOptionalParameterOrder(route);
  return decomposeUrl(route)
    .filter((x) => x.endsWith("?"))
    .map((x) => x.slice(1, -1)) as PathToOptionalKeys<T>;
};

export const extractMandatoryParamNames = <T extends string>(route: ValidateOptionalUrl<T>): PathToMandatoryKeys<T> => {
  checkRouteOptionalParameterOrder(route);
  return decomposeUrl(route)
    .filter((x) => !x.endsWith("?") && x.startsWith(":"))
    .map((x) => x.substring(1)) as PathToMandatoryKeys<T>;
};

export const extractParamNames = <T extends string>(route: ValidateOptionalUrl<T>): PathToAllKeys<T> => {
  checkRouteOptionalParameterOrder(route);
  return decomposeUrl(route)
    .filter((x) => x.startsWith(":"))
    .map((x) => (x.endsWith("?") ? x.slice(1, -1) : x.substring(1))) as PathToAllKeys<T>;
};

/** Parse a raw query-param declaration into its schema key and type-derivation placeholder. */
const parseQueryParamDecl = (raw: string): { key: string; placeholder: string } => {
  const eqIdx = raw.indexOf("=");
  if (eqIdx !== -1 && raw[eqIdx + 1] === ":") {
    return { key: raw.slice(0, eqIdx), placeholder: raw.slice(eqIdx + 2) };
  }
  return { key: raw, placeholder: raw };
};

const extractRawQueryParts = (url: string): string[] => {
  for (let i = 0; i < url.length; i++) {
    if (url[i] === "?") {
      const next = url[i + 1];
      if (next === undefined || next === "/") continue;
      return url
        .slice(i + 1)
        .split("&")
        .filter((k) => k.length > 0);
    }
  }
  return [];
};

/**
 * Extracts the schema key names declared in a route definition URL's query string.
 * Supports both `"key=:placeholder"` and bare `"key"` syntax.
 * E.g. `"/items/:id?page=:page&limit=:limitId"` → `["page", "limit"]`.
 */
export const extractQueryParamNames = (url: string): string[] =>
  extractRawQueryParts(url).map((raw) => parseQueryParamDecl(raw).key);

/**
 * Extracts the full query-param declarations from a route definition URL.
 * Each entry contains:
 *   - `key`         — the actual URL query-param name (also the schema key)
 *   - `placeholder` — the name used for type derivation (Id/non-Id suffix convention)
 * E.g. `"/items/:id?page=:page&limit=:limitId"` →
 *   `[{ key: "page", placeholder: "page" }, { key: "limit", placeholder: "limitId" }]`
 */
export const extractQueryParamDecls = (url: string): { key: string; placeholder: string }[] =>
  extractRawQueryParts(url).map(parseQueryParamDecl);

export const getIdNames = <T extends string[]>(keys: T): FilterByIdEnding<T> => {
  return keys.filter((x) => x.endsWith("_id") || x.endsWith("Id")) as FilterByIdEnding<T>;
};

export const getNonIdNames = <T extends string[]>(keys: T): FilterByIdEnding<T, false> => {
  return keys.filter((x) => !(x.endsWith("_id") || x.endsWith("Id"))) as FilterByIdEnding<T, false>;
};

export const extractParams = (route: string, path: URL): Record<string, string> => {
  const routeParts = decomposeUrl(route);
  const pathParts = decomposeUrl(path.pathname);

  const params: Record<string, string> = {};

  for (let i = 0; i < routeParts.length; i++) {
    if (routeParts[i]?.startsWith(":")) {
      let paramName = routeParts[i]!.substring(1);
      if (routeParts[i]?.endsWith("?")) {
        paramName = paramName.slice(0, -1);
      }
      // The ! is load-bearing for mandatory params: writing undefined into the record
      // intentionally shadows any same-named query param when the path segment is absent.
      // Zod then correctly rejects the request for the missing required field.
      // For optional params the value is genuinely undefined and Zod accepts it.
      params[paramName] = pathParts[i]!;
    }
  }

  return params;
};

const encodeWithSingleQuote = (str?: unknown) =>
  typeof str === "undefined" || str === null ? "" : encodeURIComponent(String(str)).replace(/'/g, "%27");

const encodeKeyValue = (key: string | number | symbol, value: unknown, search: URLSearchParams) =>
  search.append(String(key), String(value));

const objectStringify = (
  parentKeys: string | number | symbol,
  obj: Record<string | number | symbol, unknown>,
  urlSearch: URLSearchParams,
) =>
  forEach(obj, (v, k) => {
    if (typeof v === "object" && v !== null) {
      return objectStringify(
        `${encodeWithSingleQuote(parentKeys)}.${encodeWithSingleQuote(k)}`,
        v as Record<string | number | symbol, unknown>,
        urlSearch,
      );
    }
    return encodeKeyValue(`${String(parentKeys)}.${String(k)}`, v, urlSearch);
  });

//TODO maybe empty array should be preserved? otherwise some schemas might behave wierd
export const defToUrl = <PATH extends string, PARAMS extends ZodType>(
  def: Omit<RouteBase<PATH, unknown, PARAMS, ZodTypeAny>, "bodyValidation">,
  input: z.infer<PARAMS>,
) => {
  const routeParams = extractParamNames(def.path) as string[];
  const optionalParams = extractOptionalParamNames(def.path) as string[];
  const paramsIn = def.paramsValidation.parse(input);
  if (typeof paramsIn !== "object") throw new Error("Invalid params - only object should be used as params");

  const search = new URLSearchParams();

  forEach(paramsIn, (value, key) => {
    if (routeParams.includes(String(key))) return undefined;
    if (typeof value === "undefined") {
      return;
    }
    if (typeof value === "object" && value !== null) {
      return objectStringify(key, value as Record<string | number | symbol, unknown>, search);
    }
    return encodeKeyValue(key, value, search);
  });

  const pathParams = map(paramsIn, (value, key) => (routeParams.includes(String(key)) ? value : undefined));
  let path = def.path.endsWith("/") ? def.path.slice(0, -1) : def.path;
  //If we are missing an optional parameter, ignore anything that would be after it.
  //TODO maybe add a wildcard character?
  let hadUndefined = false;
  for (const c of routeParams) {
    const replacement = hadUndefined ? undefined : (pathParams as Record<string, unknown>)[c]; //TODO validate

    if (replacement === undefined) hadUndefined = true;
    const isOptional = optionalParams.includes(c);
    const addLeadingSlash = !replacement && isOptional;

    path = path.replace(
      `${addLeadingSlash ? "/" : ""}:${c}${isOptional ? "?" : ""}`,
      encodeWithSingleQuote(replacement),
    );
  }

  const output = `${path}${search.size > 0 ? "?" : ""}${search.toString()}`;
  return output;
};
