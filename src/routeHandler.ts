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
  prefix?: string,
) => Promisable<PromisableProperties<z.infer<OUT>>>;

export type HandlerWithBodyFn<USER, PARAMS extends z.ZodType, BODY extends z.ZodType, OUT extends z.ZodType> = (
  params: z.infer<PARAMS>,
  body: z.infer<BODY>,
  user: USER,
  prefix?: string,
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

/** First constructor argument of the ambient `Response`. Derived instead of naming `BodyInit`
 *  directly so it stays assignable even when a consumer's type environment (e.g.
 *  `@cloudflare/workers-types`) redeclares `BodyInit`/`Response` incompatibly with the DOM lib. */
export type ResponseBody = NonNullable<ConstructorParameters<typeof Response>[0]>;

export type FormatOutputReturnStructure = {
  data?: ResponseBody;
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

export type RouteWithHandler<
  METHOD extends HTTPMethods,
  PATH extends string,
  PERMISSION,
  PARAMS extends z.ZodType,
  BODY extends z.ZodType,
  OUT extends z.ZodType,
  PREFIX extends string = never,
> = Route<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT, PREFIX> & {
  handlerWrapped: (req: Request, prefix?: string) => Promise<Response>;
};

export type DefineType<PERMISSION, USER> = <
  METHOD extends HTTPMethods,
  PATH extends string,
  PARAMS extends z.ZodType,
  BODY extends z.ZodType,
  OUT extends z.ZodType,
  PREFIX extends string = never,
  HANDLER extends HandlerBothFn<METHOD, USER, PARAMS, BODY, OUT> = HandlerBothFn<METHOD, USER, PARAMS, BODY, OUT>,
>(
  routeDef: Route<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT, PREFIX>,
  handler: HANDLER,
  formatOutput?: FormatOutput<PARAMS, OUT, USER, Awaited<ReturnType<HANDLER>>>,
) => RouteWithHandler<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT, PREFIX>;

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

/** Only plain objects are traversed — Date, Map, class instances etc. are values, not containers. */
const isPlainObject = (value: unknown): value is Record<string, unknown> => {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const proto = Object.getPrototypeOf(value) as object | null;
  return proto === Object.prototype || proto === null;
};

/** Structural read-only view of any non-null object's own enumerable properties. */
const isObjectRecord = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);

/**
 * Non-plain objects (class instances) are values, not containers — except for direct
 * Promise-valued properties, which JSON.stringify would serialise as `{}`. Resolving
 * those (one level, own enumerable props only) matches what 0.1.x did.
 */
const hasDirectPromiseValues = (value: Record<string, unknown>): boolean =>
  Object.values(value).some((v) => v instanceof Promise);

/**
 * Synchronous, allocation-free scan for Promises at any depth. Gating the resolve pass on this
 * keeps a fully-materialised response (the common case) from allocating a Promise per element.
 */
const containsPromise = (value: unknown): boolean => {
  if (value instanceof Promise) return true;
  if (Array.isArray(value)) return value.some(containsPromise);
  if (isPlainObject(value)) return Object.values(value).some(containsPromise);
  if (isObjectRecord(value)) return hasDirectPromiseValues(value);
  return false;
};

/** Resolves Promises at any depth, in arrays and plain objects alike. */
const resolveDeep = async (value: unknown): Promise<unknown> => {
  const awaited = await value;
  if (Array.isArray(awaited)) return await Promise.all(awaited.map(resolveDeep));
  if (isPlainObject(awaited)) {
    const entries = await Promise.all(
      Object.entries(awaited).map(async ([key, v]) => [key, await resolveDeep(v)] as const),
    );
    return Object.fromEntries(entries);
  }
  if (isObjectRecord(awaited) && hasDirectPromiseValues(awaited)) {
    // Class instance with direct Promise values: resolve one level, as 0.1.x did. Promise-free
    // instances (Date, Map, DTOs with a toJSON) pass through below, keeping their prototype.
    const entries = await Promise.all(Object.entries(awaited).map(async ([key, v]) => [key, await v] as const));
    return Object.fromEntries(entries);
  }
  return awaited;
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
  route: Route<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT, string>,
  handler: HandlerBothFn<METHOD, USER, PARAMS, BODY, OUT>,
  formatOutput: FormatOutput<PARAMS, OUT, USER> | undefined,
  authorizer: (
    user: USER,
    permissionsNeeded: PERMISSION,
    req: Request,
  ) => Promise<"ok" | "forbidden" | "unauthenticated">,
  getUserFromRequest: (req: Request) => Promise<USER>,
  options?: {
    outputErrorWarning?: (error: ZodError<unknown>, data: unknown, method: string, url: string) => void;
    errorParser?: (error: unknown) => Promise<{ status: number; message: string } | undefined>;
    errorHtmlFormatter?: (status: number, message: string, request: Request, user?: USER) => Promise<string>;
    errorLogger?: (...args: unknown[]) => void;
  },
): ((req: Request, prefix?: string) => Promise<Response>) => {
  const { outputErrorWarning, errorParser, errorHtmlFormatter, errorLogger = console.error } = options ?? {};
  const { path, permissionsNeeded, paramsValidation, outputValidation, method } = route;

  const validatePerProperty = (obj: unknown, reqUrl: string) => {
    if (obj === null || typeof obj !== "object") return;
    if (!(outputValidation instanceof ZodObject)) return;
    const shape = outputValidation.shape;
    for (const [key, value] of Object.entries(obj)) {
      const fieldSchema: z.ZodType | undefined = shape[key];
      if (!fieldSchema) continue;
      const warn = (v: unknown) => {
        const parsed = fieldSchema.safeParse(v);
        if (!parsed.success) outputErrorWarning?.(parsed.error, v, method, reqUrl + "#" + key);
      };
      if (value instanceof Promise) void value.then(warn).catch(() => undefined);
      else warn(value);
    }
  };

  const handler_ = async (req: Request, prefix?: string): Promise<Response> => {
    let user: USER | undefined;
    const acceptType = req.headers.get("accept") || "";
    const er = (message: string | object, status: number) =>
      errorResponse(acceptType, errorHtmlFormatter ? (s, m) => errorHtmlFormatter(s, m, req, user) : undefined)(
        message,
        status,
      );
    try {
      user = await getUserFromRequest(req);
      const auth = await authorizer(user, permissionsNeeded, req);
      if (auth !== "ok") {
        throw new Unauthorized(
          auth === "forbidden" ? 403 : 401,
          VerboseErrorOutput
            ? "Missing the necessary permissions: " + String(permissionsNeeded)
            : auth === "forbidden"
              ? "Forbidden"
              : "Unauthorized",
        );
      }
      const url = new URL(req.url);
      const queryParams = parseUrl(url, path, paramsValidation);
      if (!queryParams.success) {
        return er("Path or query params did not match defined schema: " + queryParams.error.message, 400);
      }

      let result;
      const hasBody = method === "post" || method === "put" || method === "patch";
      if (hasBody) {
        const contentType = req.headers.get("content-type");
        let data: unknown;
        if (contentType?.includes("form")) {
          const formParsed: Record<string, unknown> = {};
          const formData = await req.formData();
          formData.forEach((value, key) => {
            if (!Reflect.has(formParsed, key)) {
              formParsed[key] = value;
              return;
            }
            const existing = formParsed[key];
            formParsed[key] = Array.isArray(existing) ? [...existing, value] : [existing, value];
          });
          // Form data does not have data types, everything is a string,
          // so we convert data that could be a number to a number type before passing it to zod parse
          const numConversions = parseNumberFromForm(route.bodyValidation, formParsed);
          if (numConversions !== null && typeof numConversions === "object" && !Array.isArray(numConversions)) {
            forEach(numConversions, (value, key) => (formParsed[key] = value));
          }
          const boolConversions = parseBooleanFromForm(route.bodyValidation, formParsed);
          if (boolConversions !== null && typeof boolConversions === "object" && !Array.isArray(boolConversions)) {
            forEach(boolConversions, (value, key) => (formParsed[key] = value));
          }
          data = formParsed;
        } else if (contentType === null || contentType.includes("json")) {
          const bodyText = await req.text();
          if (bodyText.trim() !== "") {
            try {
              data = JSON.parse(bodyText) as unknown;
            } catch {
              return new Response("Body is not valid JSON", { status: 400 });
            }
          }
        } else {
          return new Response(`Unsupported content type: ${contentType}`, { status: 415 });
        }
        const body = route.bodyValidation.safeParse(data);
        if (!body.success) {
          return er("Body does not match defined schema: " + body.error.message, 400);
        }
        // The cast is needed because TS cannot narrow the handler type based on the if statement above
        result = await (handler as HandlerWithBodyFn<USER, PARAMS, BODY, OUT>)(
          queryParams.data,
          body.data,
          user,
          prefix,
        );
      } else {
        result = await (handler as HandlerWithoutBodyFn<USER, PARAMS, OUT>)(queryParams.data, user, prefix);
        // ditto — cast is for method narrowing only, not type unsafety
      }

      if (formatOutput) {
        // Format path: pass data as-is, validate each property in the background as it resolves
        validatePerProperty(result, req.url);
        const formatted = await formatOutput(result as PromisableProperties<z.infer<OUT>>, user, req, queryParams.data);
        return new Response(formatted.data, {
          status: formatted.status ?? (formatted.redirect ? 303 : httpMethodSuccessCodes[method]),
          headers: formatted.headers,
        });
      }

      // JSON path: resolve all promises, validate the whole object, then JSON.stringify
      if (containsPromise(result)) {
        result = await resolveDeep(result);
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
      errorLogger(msg, req.url, additionalInfo, error);

      return er(VerboseErrorOutput ? { error: msg, message: additionalInfo, stack } : msg, 500);
    }
  };
  return handler_;
};

export type RouteHandlerOptions<USER> = {
  outputErrorWarning?: (error: ZodError<unknown>, data: unknown, method: string, url: string) => void;
  errorParser?: (error: unknown) => Promise<{ status: number; message: string } | undefined>;
  errorHtmlFormatter?: (status: number, message: string, request: Request, user?: USER) => Promise<string>;
  errorLogger?: (...args: unknown[]) => void;
};

export const RouteHandlerDefiner = <USER, PERMISSION>(
  authorizer: (
    user: USER,
    permissionsNeeded: PERMISSION,
    req: Request,
  ) => Promise<"ok" | "forbidden" | "unauthenticated">,
  getUserFromRequest: (req: Request) => Promise<USER>,
  options?: RouteHandlerOptions<USER>,
): DefineType<PERMISSION, USER> => {
  return <
    METHOD extends HTTPMethods,
    PATH extends string,
    PARAMS extends z.ZodType,
    BODY extends z.ZodType,
    OUT extends z.ZodType,
    PREFIX extends string = never,
    HANDLER extends HandlerBothFn<METHOD, USER, PARAMS, BODY, OUT> = HandlerBothFn<METHOD, USER, PARAMS, BODY, OUT>,
  >(
    routeDef: Route<METHOD, PATH, PERMISSION, PARAMS, BODY, OUT, PREFIX>,
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
        options,
      ),
    };
  };
};
