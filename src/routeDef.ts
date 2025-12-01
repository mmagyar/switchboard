import type { ZodNumber, ZodOptional, ZodString, ZodType } from "zod";
import { z } from "zod";
import type { HTTPMethods, HTTPMethodsWithBody, HTTPMethodsWithoutBody } from "./staticDefs.ts";
import type { FilterByIdEnding, PathToMandatoryKeys, PathToOptionalKeys, ValidateOptionalUrl } from "./urlType.ts";
import { extractMandatoryParamNames, extractOptionalParamNames, getIdNames, getNonIdNames } from "./urlUtils.ts";
import type { RouteWithBody, RouteWithoutBody } from "./routeBaseType.ts";

/**
 *
 * @param path of the request, if a prefix is given it will be joined
 * @param permissionsNeeded Declare the necessary permissions for the method, it will be checked by the authorizer function
 * @param paramsValidation Validate path params and query string - don't forget,
 *                         by default all data here are strings under the keys, but zod can parse them to number while validating
 *                         This will be the first parameter in the handler
 *                         TODO implement that if the path has params (sections starting with /: ) they MUST be in this validatin
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
> = METHOD extends HTTPMethodsWithBody
  ? RouteWithBody<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT>
  : METHOD extends HTTPMethodsWithoutBody
    ? RouteWithoutBody<METHOD, PATH, PERMISSION, PARAMS, OUT>
    : never;
type MaybeZodType = ZodType | undefined;
type MaybeUrl<PATH extends string, T extends MaybeZodType = undefined> = T extends ZodType ? T : UrlParamsSchema<PATH>;
export const define = <PERMISSION>() => {
  return {
    /**
     * @param path
     * @param permissionsNeeded
     * @param outputValidation
     * @param paramsValidation
     */
    get: <PATH extends string, PARAMS extends MaybeZodType = undefined, OUT extends ZodType = never>(
      path: ValidateOptionalUrl<PATH>,
      permissionsNeeded: PERMISSION,
      outputValidation: OUT,
      paramsValidation?: PARAMS,
    ): RouteWithoutBody<"get", PATH, PERMISSION, PARAMS extends ZodType ? PARAMS : UrlParamsSchema<PATH>, OUT> => {
      return {
        method: "get",
        path,
        permissionsNeeded,
        //Can't really get the compiler to realize the correct type without casting
        paramsValidation: (paramsValidation ?? urlToZodSchema(path)) as PARAMS extends ZodType
          ? PARAMS
          : UrlParamsSchema<PATH>,
        outputValidation,
      };
    },

    del: <PATH extends string, PARAMS extends MaybeZodType = undefined, OUT extends ZodType = never>(
      path: PATH,
      permissionsNeeded: PERMISSION,
      outputValidation: OUT,
      paramsValidation: PARAMS,
    ): RouteWithoutBody<"delete", PATH, PERMISSION, MaybeUrl<PATH, PARAMS>, OUT> => {
      return {
        method: "delete",
        path,
        permissionsNeeded,
        paramsValidation: (paramsValidation ?? urlToZodSchema(path)) as MaybeUrl<PATH, PARAMS>,
        outputValidation,
      };
    },
    options: <PATH extends string, PARAMS extends MaybeZodType = undefined, OUT extends ZodType = never>(
      path: PATH,
      permissionsNeeded: PERMISSION,
      outputValidation: OUT,
      paramsValidation?: PARAMS,
    ): RouteWithoutBody<"options", PATH, PERMISSION, MaybeUrl<PATH, PARAMS>, OUT> => {
      return {
        method: "options",
        path,
        permissionsNeeded,
        paramsValidation: (paramsValidation ?? urlToZodSchema(path)) as MaybeUrl<PATH, PARAMS>,
        outputValidation,
      };
    },
    post: <
      PATH extends string,
      PARAMS extends MaybeZodType = undefined,
      BODY extends ZodType = never,
      OUT extends ZodType = never,
    >(
      path: PATH,
      permissionsNeeded: PERMISSION,
      bodyValidation: BODY,
      outputValidation: OUT,
      paramsValidation?: PARAMS,
    ): RouteWithBody<"post", PATH, PERMISSION, MaybeUrl<PATH, PARAMS>, BODY, OUT> => {
      return {
        method: "post",
        path,
        permissionsNeeded,
        paramsValidation: (paramsValidation ?? urlToZodSchema(path)) as MaybeUrl<PATH, PARAMS>,
        bodyValidation,
        outputValidation,
      };
    },

    put: <
      PATH extends string,
      PARAMS extends MaybeZodType = undefined,
      BODY extends ZodType = never,
      OUT extends ZodType = never,
    >(
      path: PATH,
      permissionsNeeded: PERMISSION,
      bodyValidation: BODY,
      outputValidation: OUT,
      paramsValidation?: PARAMS,
    ): RouteWithBody<"put", PATH, PERMISSION, MaybeUrl<PATH, PARAMS>, BODY, OUT> => {
      return {
        method: "put",
        path,
        permissionsNeeded,
        paramsValidation: (paramsValidation ?? urlToZodSchema(path)) as MaybeUrl<PATH, PARAMS>,
        bodyValidation,
        outputValidation,
      };
    },

    patch: <
      PATH extends string,
      PARAMS extends MaybeZodType = undefined,
      BODY extends ZodType = never,
      OUT extends ZodType = never,
    >(
      path: PATH,
      permissionsNeeded: PERMISSION,
      bodyValidation: BODY,
      outputValidation: OUT,
      paramsValidation?: PARAMS,
    ): RouteWithBody<"patch", PATH, PERMISSION, MaybeUrl<PATH, PARAMS>, BODY, OUT> => {
      return {
        method: "patch",
        path,
        permissionsNeeded,
        paramsValidation: (paramsValidation ?? urlToZodSchema(path)) as MaybeUrl<PATH, PARAMS>,
        bodyValidation,
        outputValidation,
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
