export const httpMethodWithBodySuccessCodes = {
  post: 201,
  put: 200,
  patch: 200,
};

export const httpMethodWithoutBodySuccessCodes = {
  get: 200,
  delete: 204,
  head: 200,
  options: 200,
};

export const httpMethodSuccessCodes = {
  ...httpMethodWithBodySuccessCodes,
  ...httpMethodWithoutBodySuccessCodes,
};

export type HTTPMethodsWithBody = keyof typeof httpMethodWithBodySuccessCodes;
export type HTTPMethodsWithoutBody = keyof typeof httpMethodWithoutBodySuccessCodes;
export type HTTPMethods = keyof typeof httpMethodSuccessCodes;
export const parseHTTPMethod = (methodIn: string): HTTPMethods => {
  const method = methodIn.toLowerCase() as HTTPMethods;
  if (method in httpMethodSuccessCodes) return method as HTTPMethods;
  throw new Error(`Invalid HTTP method: ${method}`);
};
export class RequestError extends Error {
  constructor(
    public readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

/***
 * Throw this when you want to trigger a 404 response
 */
export class NotFoundError extends Error {
  constructor() {
    super("Not Found");
  }
}

export class Unauthorized extends Error {
  constructor(
    public readonly status: 401 | 403 = 401,
    message = "Unauthorized",
  ) {
    super(message);
  }
}
