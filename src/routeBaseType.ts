import type { ZodType } from "zod";
import type { HTTPMethods, HTTPMethodsWithBody, HTTPMethodsWithoutBody } from "./staticDefs.ts";

export interface RouteBase<
  PATH extends string,
  PERMISSION,
  PARAMS extends ZodType,
  OUT extends ZodType,
  PREFIX extends string = never,
> {
  method: HTTPMethods;
  path: PATH;
  prefixes?: readonly PREFIX[];
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
  PREFIX extends string = never,
> extends Omit<RouteBase<PATH, PERMISSION, PARAMS, OUT, PREFIX>, "method"> {
  method: METHOD;
}

export type RouteWithBody<
  METHOD extends HTTPMethodsWithBody,
  PATH extends string,
  PERMISSION,
  PARAMS extends ZodType,
  BODY extends ZodType,
  OUT extends ZodType,
  PREFIX extends string = never,
> = Omit<RouteBase<PATH, PERMISSION, PARAMS, OUT, PREFIX>, "method" | "bodyValidation"> & {
  method: METHOD;
  bodyValidation: BODY;
};
