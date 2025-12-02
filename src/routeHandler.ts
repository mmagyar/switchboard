/* eslint-disable max-lines-per-function */
import type { ZodError, z } from "zod";
import {
  type HTTPMethods,
  type HTTPMethodsWithBody,
  type HTTPMethodsWithoutBody,
  NotFoundError,
  RequestError,
  Unauthorized,
  httpMethodSuccessCodes,
} from "./staticDefs.ts";
import { forEach, merge } from "./util.ts";
import type { Route } from "./routeDef.ts";
import { parseBooleanFromForm, parseNumberFromForm, parseObjectFromForm } from "./routeParser.ts";
import { extractParams } from "./urlUtils.ts";
import { VerboseErrorOutput } from "./env.ts";

export type HandlerWithoutBodyFn<USER, PARAMS extends z.ZodType, OUT extends z.ZodType> = (
  params: z.infer<PARAMS>,
  user: USER,
) => Promise<z.infer<OUT>> | z.infer<OUT>;

export type HandlerWithBodyFn<USER, PARAMS extends z.ZodType, BODY extends z.ZodType, OUT extends z.ZodType> = (
  params: z.infer<PARAMS>,
  body: z.infer<BODY>,
  user: USER,
) => Promise<z.infer<OUT>> | z.infer<OUT>;

export type HandlerBothFn<
  METHOD extends HTTPMethods,
  USER,
  PARAMS extends z.ZodType,
  BODY extends z.ZodType,
  OUT extends z.ZodType,
> = METHOD extends HTTPMethodsWithBody
  ? HandlerWithBodyFn<USER, PARAMS, BODY, OUT>
  : METHOD extends HTTPMethodsWithoutBody
    ? HandlerWithoutBodyFn<USER, PARAMS, OUT>
    : never;

export type FormatOutputReturnStructure = {
  data?: BodyInit;
  headers: Headers;
  redirect?: true;
  status?: number;
};

export type FormatOutputReturn = Promise<FormatOutputReturnStructure> | FormatOutputReturnStructure;

export type FormatOutput<PARAMS extends z.ZodType, OUT extends z.ZodType, U> = (
  data: z.infer<OUT>,
  user: U,
  request: Request,
  params: z.infer<PARAMS>,
) => FormatOutputReturn;

type ReqRes = (req: Request) => Promise<Response>;

export type RouteWithHandler<
  METHOD extends HTTPMethods,
  PATH extends string,
  PERMISSION,
  PARAMS extends z.ZodType,
  BODY extends z.ZodType,
  OUT extends z.ZodType,
> = Route<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT> & {
  handlerWrapped: ReqRes;
};
export type DefineType<PERMISSION, USER> = <
  METHOD extends HTTPMethods,
  PATH extends string,
  PARAMS extends z.ZodType,
  BODY extends z.ZodType,
  OUT extends z.ZodType,
>(
  routeDef: Route<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT>,
  handler: HandlerBothFn<METHOD, USER, PARAMS, BODY, OUT>,
  formatOutput?: FormatOutput<PARAMS, OUT, USER>,
) => RouteWithHandler<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT>;

const errorResponse =
  (contentType: string, errorHtmlFormatter?: (status: number, message: string) => Promise<string>) =>
  async (message: string | object, status: number) => {
    const isJson = contentType.includes("json");
    const isHtml = contentType.includes("html") || !contentType.includes("plain");
    const messageString = isJson || typeof message === "object" ? JSON.stringify(message) : message;
    return new Response(
      isHtml && errorHtmlFormatter ? await errorHtmlFormatter?.(status, messageString) : messageString,
      {
        status: status,
        headers: {
          "Content-Type": isJson ? "application/json" : isHtml ? "text/html" : "text/plain",
        },
      },
    );
  };
/**
    * Wraps the handler with the necessary logic to handle the request.
    * This includes:
    * - Checking if the user has the necessary permissions
    * - Parsing the request parameters
    * - Validating the request parameters
    * - Validating the request body
    * - Running the handler
    * - Validating the output
    * - Formatting the output
    * - Handling errors

    * @param def The route definition
    * @param handler The handler function
    * @param formatOutput The function to format the output
    * @param authorizer Used to check permission, it's okay to ForbiddenHttpError in case of an unauthorized used
    * @param getUserFromRequest Create a type safe user object based on the request,
                             ideally a middleware should do the authentication and populate req.user,
                             But in some cases this method can be used to authenticated the request as well.
    * @param outputErrorWarning Handle cases when the output data does not match the validation.
                             (should be rare is type safety is kept,
                              but there are requirements that can not be express on the type level)
    * @returns wrapped handler
 */
export const wrapHandler = <
  USER,
  PERMISSION,
  METHOD extends HTTPMethods,
  PATH extends string,
  PARAMS extends z.ZodType,
  BODY extends z.ZodType,
  OUT extends z.ZodType,
>(
  def: Route<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT>,
  handler: HandlerBothFn<METHOD, USER, PARAMS, BODY, OUT>,
  formatOutput: FormatOutput<PARAMS, OUT, USER> | undefined,
  authorizer: (
    user: USER,
    permissionsNeeded: PERMISSION,
    req: Request,
  ) => Promise<"ok" | "unauthorized" | "unauthenticated">,
  getUserFromRequest: (req: Request) => Promise<USER>,
  outputErrorWarning?: (error: ZodError<unknown>, data: unknown, method: string, url: string) => void,
  errorParser?: (error: unknown) => Promise<{ status: number; message: string } | undefined>,
  errorHtmlFormatter?: (status: number, message: string, request: Request, user?: USER) => Promise<string>,
): ReqRes => {
  const { path, permissionsNeeded, paramsValidation, outputValidation, method } = def;

  return async (req: Request): Promise<Response> => {
    let user: USER;
    try {
      user = await getUserFromRequest(req);
      const auth = await authorizer(user, permissionsNeeded, req);
      if (auth !== "ok") {
        // Throw when the user is not authorized
        throw new Unauthorized(
          auth === "unauthorized" ? 403 : 401,
          "Missing the necessary permissions, permissions needed to visit this page: " + permissionsNeeded,
        );
      }
      const url = new URL(req.url);
      let searchParams: Record<
        string,
        string | string[] //| number | number[] | boolean | boolean[] | object | object[]
      > = {};
      for (const [key, value] of url.searchParams.entries()) {
        if (searchParams[key] === undefined) {
          searchParams[key] = value;
        } else {
          if (Array.isArray(searchParams[key])) {
            searchParams[key].push(value);
          } else {
            searchParams[key] = [searchParams[key], value];
          }
        }
      }
      let fromUrl: Record<string, string | string[]> = {
        ...searchParams,
        ...extractParams(path, url),
      };

      //Parse back objects
      const parsedObjectDepth = merge(fromUrl, parseObjectFromForm(fromUrl));
      //Make sure numbers are actually treated as numbers, boolean, so we can use proper zod schema
      let parsedNumbersOnly = parseNumberFromForm(paramsValidation, parsedObjectDepth);
      if (typeof parsedNumbersOnly !== "object") {
        // It should not really happen unless the value is undefined
        // Here to please the typescript complier
        parsedNumbersOnly = undefined;
      }

      //TODO this nonEmpty does not work for undefined objects
      // (??? What does this mean, i don't even know anymore)
      const parsedNumbers = merge(parsedObjectDepth, parsedNumbersOnly, "nonEmpty");

      let parsedBooleans = parseBooleanFromForm(paramsValidation, parsedNumbers);
      if (typeof parsedBooleans !== "object") {
        // It should not really happen unless the value is undefined
        // Here to please the typescript complier
        parsedBooleans = undefined;
      }

      const parsedBooleansAndNumbers = merge(parsedNumbers, parsedBooleans, "nonEmpty");
      // This will throw a validation error we are not matching schema
      const queryParams = paramsValidation.safeParse(parsedBooleansAndNumbers);
      if (!queryParams.success) {
        return new Response("Path or query params did not match defined schema:" + queryParams.error, { status: 400 });
      }
      let result;
      const hasBody = method === "post" || method === "put" || method === "patch";
      if (hasBody) {
        //body readable stream to string
        //check if the body is a form data
        const contentType = req.headers.get("content-type");
        let data: any = {};
        if (contentType?.includes("form")) {
          const formData = await req.formData();
          formData.forEach((value, key) => {
            if (!Reflect.has(data, key)) {
              data[key] = value;
              return;
            }
            if (!Array.isArray(data[key])) {
              data[key] = [data[key]];
            }
            data[key].push(value);
          });
          //TODO support encoded objects, use merge function
          //Form data does not have data types, everything is a string,
          // so we convert data that could be a number to a number type before passing it to zod parse
          forEach(parseNumberFromForm(def.bodyValidation, data) || {}, (value, key) => (data[key] = value));
          forEach(parseBooleanFromForm(def.bodyValidation, data) || {}, (value, key) => (data[key] = value));
        } else data = await req.json();
        const body = def.bodyValidation.safeParse(data);
        if (!body.success) {
          return new Response("Body does not match defined schema:" + body.error, { status: 400 });
        }
        //The casts are not nice but there is no other way, we know the type is correct, it's enforced on calls,
        // But TS cannot make the distinction based on the the if statement above
        result = await (handler as HandlerWithBodyFn<USER, PARAMS, BODY, OUT>)(queryParams.data, body.data, user);
      } else {
        result = await (handler as HandlerWithoutBodyFn<USER, PARAMS, OUT>)(queryParams.data, user);
      }

      const output = outputValidation.safeParse(result);
      if (!output.success) {
        outputErrorWarning?.(output.error, result, method, req.url);
      } else {
        //If the validation was successful, use that, since zod will strip extra parameters
        result = output.data;
      }

      if (formatOutput) {
        //TODO maybe skip this if output schema validation fails? otherwise it may lead to unexpeted errors
        result = await formatOutput(result, user, req, queryParams.data);

        return new Response(result.data, {
          status: (result.status ?? result.redirect) ? 303 : httpMethodSuccessCodes[method],
          headers: result.headers,
        });
      } else {
        return new Response(JSON.stringify(result), {
          status: httpMethodSuccessCodes[method],
          headers: { "Content-Type": "application/json" },
        });
      }
    } catch (error) {
      const errorParsed = await errorParser?.(error);
      const acceptType = req.headers.get("accept") || "";
      const er = errorResponse(
        acceptType,
        errorHtmlFormatter ? (status, message) => errorHtmlFormatter(status, message, req, user) : undefined,
      );
      if (errorParsed) {
        return er(errorParsed.message, errorParsed.status);
      }
      if (error instanceof NotFoundError) {
        //TODO if output is html, reroute to 404 html
        return er("Not Found - Missing entry", 404);
      }
      if (error instanceof Unauthorized) {
        return er(error.message, error.status);
      }

      if (error instanceof RequestError) {
        return er(error.message, error.status);
      }

      let msg = "Internal Server Error";

      let additionalInfo = "";
      if (error && typeof error === "object" && "message" in error) {
        additionalInfo = `${error.message}`;
      }

      let stack;
      if (error instanceof Error) stack = error.stack;

      console.error(msg, req.url, additionalInfo, error);

      return er(VerboseErrorOutput ? { error: msg, message: additionalInfo, stack } : msg, 500);
    }
  };
};

export const RouteHandlerDefiner = <USER, PERMISSION>(
  authorizer: (
    user: USER,
    permissionsNeeded: PERMISSION,
    req: Request,
  ) => Promise<"ok" | "unauthorized" | "unauthenticated">,
  getUserFromRequest: (req: Request) => Promise<USER>,
  outputErrorWarning?: (error: ZodError<unknown>, data: unknown, method: string, url: string) => void,
  errorParser?: (error: unknown) => Promise<{ status: number; message: string } | undefined>,
  errorHtmlFormatter?: (status: number, message: string, request: Request, user?: USER) => Promise<string>,
): DefineType<PERMISSION, USER> => {
  return <
    METHOD extends HTTPMethods,
    PATH extends string,
    PARAMS extends z.ZodType,
    BODY extends z.ZodType,
    OUT extends z.ZodType,
  >(
    routeDef: Route<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT>,
    handler: HandlerBothFn<METHOD, USER, PARAMS, BODY, OUT>,
    formatOutput?: FormatOutput<PARAMS, OUT, USER>,
  ) => {
    return {
      ...routeDef,
      handlerWrapped: wrapHandler<USER, PERMISSION, METHOD, PATH, PARAMS, BODY, OUT>(
        routeDef,
        handler,
        formatOutput,
        authorizer,
        getUserFromRequest,
        outputErrorWarning,
        errorParser,
        errorHtmlFormatter,
      ),
    };
  };
};
