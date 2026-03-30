/* eslint-disable max-lines-per-function */
import { ZodObject } from "zod";
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
import { forEach } from "./util.ts";
import type { Route } from "./routeDef.ts";
import { parseBooleanFromForm, parseNumberFromForm, parseUrl } from "./routeParser.ts";
import { VerboseErrorOutput } from "./env.ts";

export type Promisable<T> = T | Promise<T>;
export type PromisableProperties<T> = T extends object ? { [K in keyof T]: Promisable<T[K]> } : T;

export type HandlerWithoutBodyFn<USER, PARAMS extends z.ZodType, OUT extends z.ZodType> = (
  params: z.infer<PARAMS>,
  user: USER,
) => Promisable<PromisableProperties<z.infer<OUT>>>;

export type HandlerWithBodyFn<USER, PARAMS extends z.ZodType, BODY extends z.ZodType, OUT extends z.ZodType> = (
  params: z.infer<PARAMS>,
  body: z.infer<BODY>,
  user: USER,
) => Promisable<PromisableProperties<z.infer<OUT>>>;

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

/** Formats the handler output into a Response. `data` has the exact type the handler returned:
 *  plain values stay plain, Promise-valued properties stay as Promises — enabling streaming SSR.
 *  `RAW` is inferred automatically from the handler's return type at the call site. */
export type FormatOutput<
  PARAMS extends z.ZodType,
  OUT extends z.ZodType,
  U,
  RAW = PromisableProperties<z.infer<OUT>>,
> = (data: RAW, user: U, request: Request, params: z.infer<PARAMS>) => FormatOutputReturn;

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
  HANDLER extends HandlerBothFn<METHOD, USER, PARAMS, BODY, OUT>,
>(
  routeDef: Route<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT>,
  handler: HANDLER,
  formatOutput?: FormatOutput<PARAMS, OUT, USER, Awaited<ReturnType<HANDLER>>>,
) => RouteWithHandler<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT>;

const errorResponse =
  (contentType: string, errorHtmlFormatter?: (status: number, message: string) => Promise<string>) =>
  async (message: string | object, status: number) => {
    const isJson = contentType.includes("json");
    const isHtml = !isJson && (contentType.includes("html") || !contentType.includes("plain"));
    const messageString = isJson || typeof message === "object" ? JSON.stringify(message) : message;
    return new Response(
      isHtml && errorHtmlFormatter ? await errorHtmlFormatter(status, messageString) : messageString,
      {
        status,
        headers: {
          "Content-Type": isJson ? "application/json" : isHtml ? "text/html" : "text/plain",
        },
      },
    );
  };

const resolveAllProperties = async (obj: Record<string, unknown>): Promise<Record<string, unknown>> => {
  const resolved = await Promise.all(Object.entries(obj).map(async ([k, v]) => [k, await v] as const));
  return Object.fromEntries(resolved);
};

const hasPromiseValues = (obj: unknown): obj is Record<string, unknown> =>
  obj !== null &&
  typeof obj === "object" &&
  Object.values(obj as Record<string, unknown>).some((v) => v instanceof Promise);

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
 *
 * @param def The route definition
 * @param handler The handler function
 * @param formatOutput Formats the handler output. Receives data exactly as the handler returned it:
 *                     plain values are plain, Promise-valued properties remain as Promises.
 *                     Per-property validation fires in the background as each promise resolves.
 *                     If omitted, all promises are resolved, output is validated, and JSON is returned.
 * @param authorizer Used to check permission, it's okay to throw Unauthorized in case of an unauthorized user
 * @param getUserFromRequest Create a type safe user object based on the request.
 *                           Ideally a middleware should do the authentication and populate req.user,
 *                           but in some cases this method can be used to authenticate the request as well.
 * @param outputErrorWarning Handle cases when the output data does not match the validation.
 *                           (should be rare if type safety is kept,
 *                            but there are requirements that cannot be expressed on the type level)
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

  const validatePerProperty = (obj: unknown) => {
    if (obj === null || typeof obj !== "object") return;
    if (!(outputValidation instanceof ZodObject)) return;
    const shape = outputValidation.shape;
    for (const [key, value] of Object.entries(obj)) {
      const fieldSchema: z.ZodType | undefined = shape[key];
      if (!fieldSchema) continue;
      const warn = (v: unknown) => {
        const parsed = fieldSchema.safeParse(v);
        if (!parsed.success) outputErrorWarning?.(parsed.error, v, method, key);
      };
      if (value instanceof Promise) void value.then(warn).catch(() => undefined);
      else warn(value);
    }
  };

  return async (req: Request): Promise<Response> => {
    let user: USER | undefined;
    try {
      user = await getUserFromRequest(req);
      const auth = await authorizer(user, permissionsNeeded, req);
      if (auth !== "ok") {
        throw new Unauthorized(
          auth === "unauthorized" ? 403 : 401,
          "Missing the necessary permissions, permissions needed to visit this page: " + permissionsNeeded,
        );
      }
      const url = new URL(req.url);
      const queryParams = parseUrl(url, path, paramsValidation);
      if (!queryParams.success) {
        return new Response("Path or query params did not match defined schema:" + queryParams.error, { status: 400 });
      }

      let result;
      const hasBody = method === "post" || method === "put" || method === "patch";
      if (hasBody) {
        const contentType = req.headers.get("content-type");
        let data: Record<string, unknown> = {};
        if (contentType?.includes("form")) {
          const formData = await req.formData();
          formData.forEach((value, key) => {
            if (!Reflect.has(data, key)) {
              data[key] = value;
              return;
            }
            const existing = data[key];
            data[key] = Array.isArray(existing) ? [...existing, value] : [existing, value];
          });
          // TODO: support encoded objects, use merge function
          // Form data does not have data types, everything is a string,
          // so we convert data that could be a number to a number type before passing it to zod parse
          forEach(parseNumberFromForm(def.bodyValidation, data) || {}, (value, key) => (data[key] = value));
          forEach(parseBooleanFromForm(def.bodyValidation, data) || {}, (value, key) => (data[key] = value));
        } else if (contentType === null || contentType.includes("json")) {
          try {
            data = (await req.json()) as Record<string, unknown>;
          } catch {
            return new Response("Body is not valid JSON", { status: 400 });
          }
        } else {
          return new Response(`Unsupported content type: ${contentType}`, { status: 415 });
        }
        const body = def.bodyValidation.safeParse(data);
        if (!body.success) {
          return new Response("Body does not match defined schema:" + body.error, { status: 400 });
        }
        // The casts are not nice but there is no other way, we know the type is correct, it's enforced on calls,
        // but TS cannot make the distinction based on the if statement above
        result = await (handler as HandlerWithBodyFn<USER, PARAMS, BODY, OUT>)(queryParams.data, body.data, user);
      } else {
        result = await (handler as HandlerWithoutBodyFn<USER, PARAMS, OUT>)(queryParams.data, user);
      }

      if (formatOutput) {
        // Format path: pass data as-is, validate each property in the background as it resolves
        validatePerProperty(result);
        const formatted = await formatOutput(result as PromisableProperties<z.infer<OUT>>, user, req, queryParams.data);
        return new Response(formatted.data, {
          status: formatted.status ?? (formatted.redirect ? 303 : httpMethodSuccessCodes[method]),
          headers: formatted.headers,
        });
      }

      // JSON path: resolve all promises, validate the whole object, then JSON.stringify
      if (hasPromiseValues(result)) {
        result = await resolveAllProperties(result);
      }
      const output = outputValidation.safeParse(result);
      if (!output.success) {
        outputErrorWarning?.(output.error, result, method, req.url);
      } else {
        // If the validation was successful, use that, since zod will strip extra parameters
        result = output.data;
      }
      return new Response(JSON.stringify(result), {
        status: httpMethodSuccessCodes[method],
        headers: { "Content-Type": "application/json" },
      });
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
    HANDLER extends HandlerBothFn<METHOD, USER, PARAMS, BODY, OUT>,
  >(
    routeDef: Route<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT>,
    handler: HANDLER,
    formatOutput?: FormatOutput<PARAMS, OUT, USER, Awaited<ReturnType<HANDLER>>>,
  ) => {
    return {
      ...routeDef,
      handlerWrapped: wrapHandler<USER, PERMISSION, METHOD, PATH, PARAMS, BODY, OUT>(
        routeDef,
        handler,
        // Safe: Awaited<ReturnType<HANDLER>> is always a subtype of PromisableProperties<z.infer<OUT>>
        formatOutput as FormatOutput<PARAMS, OUT, USER> | undefined,
        authorizer,
        getUserFromRequest,
        outputErrorWarning,
        errorParser,
        errorHtmlFormatter,
      ),
    };
  };
};
