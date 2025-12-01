import type { ZodType } from "zod";
import type { HTTPMethods, HTTPMethodsWithBody, HTTPMethodsWithoutBody } from "./staticDefs.ts";

export interface RouteBase<PATH extends string, PERMISSION, PARAMS extends ZodType, OUT extends ZodType> {
  method: HTTPMethods;
  path: PATH;
  permissionsNeeded: PERMISSION;
  paramsValidation: PARAMS;
  bodyValidation?: undefined;
  outputValidation: OUT;
}

export interface RouteWithoutBody<
  METHOD extends HTTPMethodsWithoutBody,
  PATH extends string,
  PERMISSION,
  PARAMS extends ZodType,
  OUT extends ZodType,
> extends Omit<RouteBase<PATH, PERMISSION, PARAMS, OUT>, "method"> {
  method: METHOD;
}

export type RouteWithBody<
  METHOD extends HTTPMethodsWithBody,
  PATH extends string,
  PERMISSION,
  PARAMS extends ZodType,
  BODY extends ZodType,
  OUT extends ZodType,
> = Omit<RouteBase<PATH, PERMISSION, PARAMS, OUT>, "method" | "bodyValidation"> & {
  method: METHOD;
  bodyValidation: BODY;
};
