import type { ZodNumber, ZodString, ZodType } from "zod";
import { z, ZodArray, ZodEnum, ZodLiteral, ZodObject, ZodOptional, ZodReadonly, ZodUnion } from "zod";
import type { HTTPMethods, HTTPMethodsWithBody, HTTPMethodsWithoutBody } from "./staticDefs.ts";
import type { FilterByIdEnding, PathToMandatoryKeys, PathToOptionalKeys, ValidateOptionalUrl } from "./urlType.ts";
import {
  extractMandatoryParamNames,
  extractOptionalParamNames,
  extractParamNames,
  getIdNames,
  getNonIdNames,
} from "./urlUtils.ts";
import type { RouteWithBody, RouteWithoutBody } from "./routeBaseType.ts";

/**
 *
 * @param path of the request, if a prefix is given it will be joined
 * @param permissionsNeeded Declare the necessary permissions for the method, it will be checked by the authorizer function
 * @param paramsValidation Validate path params and query string - don't forget,
 *                         by default all data here are strings under the keys, but zod can parse them to number while validating
 *                         This will be the first parameter in the handler
 *                         Path params (sections starting with /: ) are validated to be present in the schema at definition time.
 * @param bodyValidation body validator, it will run on the request body (req.body),
 *                       type of the second handler argument is derived from this
 * @param outputValidation  Output Validator, used to make sure that we are sending back correct data,
 *  but it will only issue a warning
 *  If it does not, but will still return, the return type of the handler function is derived from this.
 **/
export type Route<
  METHOD extends HTTPMethods,
  PATH extends string,
  PERMISSION,
  PARAMS extends ZodType,
  BODY extends ZodType = z.ZodTypeAny,
  OUT extends ZodType = z.ZodTypeAny,
  PREFIX extends string = never,
> = METHOD extends HTTPMethodsWithBody
  ? RouteWithBody<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT, PREFIX>
  : METHOD extends HTTPMethodsWithoutBody
    ? RouteWithoutBody<METHOD, PATH, PERMISSION, PARAMS, OUT, PREFIX>
    : never;

type MaybeZodType = ZodType | undefined;

/**
 * Returns true if a schema branch is "string-origin" — i.e. it can receive a raw string
 * from a URL and produce a meaningful result after the coercion pre-pass.
 * Two or more string-origin branches in the same ZodUnion are ambiguous because the coercion
 * system cannot determine which branch the consumer intended.
 */
function isStringOrigin(schema: ZodType): boolean {
  if (schema instanceof ZodReadonly) return isStringOrigin(schema.unwrap() as ZodType);
  if (schema instanceof ZodOptional) return isStringOrigin(schema.unwrap() as ZodType);
  if (schema instanceof z.ZodString) return true;
  if (schema instanceof z.ZodNumber) return true;
  if (schema instanceof z.ZodBoolean) return true;
  if (schema instanceof ZodEnum) return true;
  if (schema instanceof ZodLiteral) return typeof schema.value === "string";
  return false;
}

/**
 * Recursively walks a params schema and throws if any ZodUnion contains more than one
 * string-origin branch. Such unions are inherently ambiguous in a URL context because all
 * incoming values are plain strings and the coercion system cannot decide which branch wins.
 *
 * @param schema - the schema to validate
 * @param path - dot-separated field path for error messages (e.g. "filters.0.value")
 */
export function assertNoAmbiguousUnions(schema: ZodType, path = ""): void {
  if (schema instanceof ZodReadonly) return assertNoAmbiguousUnions(schema.unwrap() as ZodType, path);
  if (schema instanceof ZodOptional) return assertNoAmbiguousUnions(schema.unwrap() as ZodType, path);

  if (schema instanceof ZodUnion) {
    const stringOriginBranches = (schema.options as ZodType[]).filter(isStringOrigin);
    if (stringOriginBranches.length > 1) {
      const at = path ? ` at field "${path}"` : "";
      const branches = stringOriginBranches.map((b) => b.constructor.name).join(", ");
      throw new Error(
        `paramsValidation contains an ambiguous union${at}: multiple branches (${branches}) can all match a raw URL string. ` +
          `Use a single unambiguous type, or use z.preprocess() to handle the conversion explicitly.`,
      );
    }
    // recurse into each branch to catch nested unions
    for (const branch of schema.options as ZodType[]) {
      assertNoAmbiguousUnions(branch, path);
    }
    return;
  }

  if (schema instanceof ZodObject) {
    for (const [key, value] of Object.entries(schema.shape as Record<string, ZodType>)) {
      assertNoAmbiguousUnions(value, path ? `${path}.${key}` : key);
    }
    return;
  }

  if (schema instanceof ZodArray) {
    assertNoAmbiguousUnions(schema.element as ZodType, path ? `${path}[]` : "[]");
    return;
  }
}

function assertPathParamsInSchema(path: string, paramsValidation: ZodType): void {
  const pathParams = extractParamNames(path as Parameters<typeof extractParamNames>[0]);
  if (pathParams.length === 0) return;

  if (!(paramsValidation instanceof ZodObject)) {
    throw new Error(
      `Route "${path}" has path params [${pathParams.join(", ")}] but paramsValidation is not a ZodObject`,
    );
  }

  const schemaKeys = Object.keys(paramsValidation.shape);
  const missingParams = pathParams.filter((param) => !schemaKeys.includes(param));
  if (missingParams.length > 0) {
    throw new Error(
      `Route "${path}" has path params [${missingParams.join(", ")}] missing from paramsValidation schema`,
    );
  }
}

function validateParams(path: string, paramsValidation: ZodType | undefined): void {
  if (paramsValidation) {
    assertPathParamsInSchema(path, paramsValidation);
    assertNoAmbiguousUnions(paramsValidation);
  }
}

function assertValidPrefixes(prefixes: readonly string[]): void {
  for (const p of prefixes) {
    if (p.startsWith("/")) throw new Error(`Prefix "${p}" must not start with a slash`);
    if (p.includes(":")) throw new Error(`Prefix "${p}" must not contain path params`);
  }
}

type MaybeUrl<PATH extends string, T extends MaybeZodType = undefined> = T extends ZodType ? T : UrlParamsSchema<PATH>;

export const define = <PERMISSION>() => {
  return {
    /**
     * @param path
     * @param permissionsNeeded
     * @param outputValidation
     * @param paramsValidation
     * @param prefixes
     */
    get: <
      PATH extends string,
      PARAMS extends MaybeZodType = undefined,
      OUT extends ZodType = never,
      PREFIXES extends readonly string[] = never[],
    >(
      path: ValidateOptionalUrl<PATH>,
      permissionsNeeded: PERMISSION,
      outputValidation: OUT,
      paramsValidation?: PARAMS,
      prefixes?: readonly [...PREFIXES],
    ): RouteWithoutBody<"get", PATH, PERMISSION, MaybeUrl<PATH, PARAMS>, OUT, PREFIXES[number]> => {
      validateParams(path, paramsValidation);
      if (prefixes && prefixes.length > 0) assertValidPrefixes(prefixes);
      return {
        method: "get",
        path,
        permissionsNeeded,
        paramsValidation: (paramsValidation ?? urlToZodSchema(path)) as MaybeUrl<PATH, PARAMS>,
        outputValidation,
        ...(prefixes && prefixes.length > 0 ? { prefixes } : {}),
      };
    },

    del: <
      PATH extends string,
      PARAMS extends MaybeZodType = undefined,
      OUT extends ZodType = never,
      PREFIXES extends readonly string[] = never[],
    >(
      path: ValidateOptionalUrl<PATH>,
      permissionsNeeded: PERMISSION,
      outputValidation: OUT,
      paramsValidation?: PARAMS,
      prefixes?: readonly [...PREFIXES],
    ): RouteWithoutBody<"delete", PATH, PERMISSION, MaybeUrl<PATH, PARAMS>, OUT, PREFIXES[number]> => {
      validateParams(path, paramsValidation);
      if (prefixes && prefixes.length > 0) assertValidPrefixes(prefixes);
      return {
        method: "delete",
        path,
        permissionsNeeded,
        paramsValidation: (paramsValidation ?? urlToZodSchema(path)) as MaybeUrl<PATH, PARAMS>,
        outputValidation,
        ...(prefixes && prefixes.length > 0 ? { prefixes } : {}),
      };
    },

    options: <
      PATH extends string,
      PARAMS extends MaybeZodType = undefined,
      OUT extends ZodType = never,
      PREFIXES extends readonly string[] = never[],
    >(
      path: ValidateOptionalUrl<PATH>,
      permissionsNeeded: PERMISSION,
      outputValidation: OUT,
      paramsValidation?: PARAMS,
      prefixes?: readonly [...PREFIXES],
    ): RouteWithoutBody<"options", PATH, PERMISSION, MaybeUrl<PATH, PARAMS>, OUT, PREFIXES[number]> => {
      validateParams(path, paramsValidation);
      if (prefixes && prefixes.length > 0) assertValidPrefixes(prefixes);
      return {
        method: "options",
        path,
        permissionsNeeded,
        paramsValidation: (paramsValidation ?? urlToZodSchema(path)) as MaybeUrl<PATH, PARAMS>,
        outputValidation,
        ...(prefixes && prefixes.length > 0 ? { prefixes } : {}),
      };
    },

    post: <
      PATH extends string,
      PARAMS extends MaybeZodType = undefined,
      BODY extends ZodType = never,
      OUT extends ZodType = never,
      PREFIXES extends readonly string[] = never[],
    >(
      path: ValidateOptionalUrl<PATH>,
      permissionsNeeded: PERMISSION,
      bodyValidation: BODY,
      outputValidation: OUT,
      paramsValidation?: PARAMS,
      prefixes?: readonly [...PREFIXES],
    ): RouteWithBody<"post", PATH, PERMISSION, MaybeUrl<PATH, PARAMS>, BODY, OUT, PREFIXES[number]> => {
      validateParams(path, paramsValidation);
      if (prefixes && prefixes.length > 0) assertValidPrefixes(prefixes);
      return {
        method: "post",
        path,
        permissionsNeeded,
        paramsValidation: (paramsValidation ?? urlToZodSchema(path)) as MaybeUrl<PATH, PARAMS>,
        bodyValidation,
        outputValidation,
        ...(prefixes && prefixes.length > 0 ? { prefixes } : {}),
      };
    },

    put: <
      PATH extends string,
      PARAMS extends MaybeZodType = undefined,
      BODY extends ZodType = never,
      OUT extends ZodType = never,
      PREFIXES extends readonly string[] = never[],
    >(
      path: ValidateOptionalUrl<PATH>,
      permissionsNeeded: PERMISSION,
      bodyValidation: BODY,
      outputValidation: OUT,
      paramsValidation?: PARAMS,
      prefixes?: readonly [...PREFIXES],
    ): RouteWithBody<"put", PATH, PERMISSION, MaybeUrl<PATH, PARAMS>, BODY, OUT, PREFIXES[number]> => {
      validateParams(path, paramsValidation);
      if (prefixes && prefixes.length > 0) assertValidPrefixes(prefixes);
      return {
        method: "put",
        path,
        permissionsNeeded,
        paramsValidation: (paramsValidation ?? urlToZodSchema(path)) as MaybeUrl<PATH, PARAMS>,
        bodyValidation,
        outputValidation,
        ...(prefixes && prefixes.length > 0 ? { prefixes } : {}),
      };
    },

    patch: <
      PATH extends string,
      PARAMS extends MaybeZodType = undefined,
      BODY extends ZodType = never,
      OUT extends ZodType = never,
      PREFIXES extends readonly string[] = never[],
    >(
      path: ValidateOptionalUrl<PATH>,
      permissionsNeeded: PERMISSION,
      bodyValidation: BODY,
      outputValidation: OUT,
      paramsValidation?: PARAMS,
      prefixes?: readonly [...PREFIXES],
    ): RouteWithBody<"patch", PATH, PERMISSION, MaybeUrl<PATH, PARAMS>, BODY, OUT, PREFIXES[number]> => {
      validateParams(path, paramsValidation);
      if (prefixes && prefixes.length > 0) assertValidPrefixes(prefixes);
      return {
        method: "patch",
        path,
        permissionsNeeded,
        paramsValidation: (paramsValidation ?? urlToZodSchema(path)) as MaybeUrl<PATH, PARAMS>,
        bodyValidation,
        outputValidation,
        ...(prefixes && prefixes.length > 0 ? { prefixes } : {}),
      };
    },
  };
};

export type Params<T extends { paramsValidation: z.ZodTypeAny }> = z.infer<T["paramsValidation"]>;
export type Body<T extends { bodyValidation: z.ZodTypeAny }> = z.infer<T["bodyValidation"]>;
export type Output<T extends { outputValidation: z.ZodTypeAny }> = z.infer<T["outputValidation"]>;

export type UrlParamsSchema<T extends string> = z.ZodObject<
  Record<FilterByIdEnding<PathToMandatoryKeys<T>>[number], ZodNumber> &
    Record<FilterByIdEnding<PathToMandatoryKeys<T>, false>[number], ZodString> &
    Record<FilterByIdEnding<PathToOptionalKeys<T>>[number], ZodOptional<ZodNumber>> &
    Record<FilterByIdEnding<PathToOptionalKeys<T>, false>[number], ZodOptional<ZodString>>
>;

export function urlToZodSchema<T extends string>(url: ValidateOptionalUrl<T>): UrlParamsSchema<T> {
  const params = extractMandatoryParamNames(url);
  type ParamsId = FilterByIdEnding<typeof params>[number];
  type ParamsNonId = FilterByIdEnding<typeof params, false>[number];
  let paramsId: Record<ParamsId, ZodNumber> = {};
  let paramsNonId: Record<ParamsNonId, ZodString> = {};

  const optional = extractOptionalParamNames(url);
  type OptionalsId = FilterByIdEnding<typeof optional>[number];
  type OptionalsNonId = FilterByIdEnding<typeof optional, false>[number];
  let optionalsId: Record<OptionalsId, ZodOptional<ZodNumber>> = {};
  let optionalsNonId: Record<OptionalsNonId, ZodOptional<ZodString>> = {};

  for (const param of getIdNames(params) as ParamsId[]) {
    paramsId[param] = z.number();
  }
  for (const param of getNonIdNames(params) as ParamsNonId[]) {
    paramsNonId[param] = z.string();
  }
  for (const param of getIdNames(optional) as OptionalsId[]) {
    optionalsId[param] = z.optional(z.number());
  }
  for (const param of getNonIdNames(optional) as OptionalsNonId[]) {
    optionalsNonId[param] = z.optional(z.string());
  }

  return z.object({ ...paramsId, ...paramsNonId, ...optionalsId, ...optionalsNonId });
}
