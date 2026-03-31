import { afterEach, describe, expect, spyOn, test } from "bun:test";
import { z } from "zod";
import { ApiError, createClient } from "./clientApiCall.ts";
import { define } from "./routeDef.ts";

const def = define<"public">();

// ---------------------------------------------------------------------------
// ApiError
// ---------------------------------------------------------------------------

describe("ApiError", () => {
  test("is an instance of Error", () => {
    const err = new ApiError(404, "Not Found");
    expect(err).toBeInstanceOf(Error);
  });

  test("is an instance of ApiError", () => {
    const err = new ApiError(500, "Server Error");
    expect(err).toBeInstanceOf(ApiError);
  });

  test("carries the correct status code", () => {
    const err = new ApiError(403, "Forbidden");
    expect(err.status).toBe(403);
  });

  test("carries the correct message", () => {
    const err = new ApiError(400, "Bad Request");
    expect(err.message).toBe("Bad Request");
  });

  test("has name 'ApiError'", () => {
    const err = new ApiError(422, "Unprocessable Entity");
    expect(err.name).toBe("ApiError");
  });

  test("instanceof checks survive a try/catch boundary", () => {
    let caught: unknown;
    try {
      throw new ApiError(401, "Unauthorized");
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(Error);
    expect(caught).toBeInstanceOf(ApiError);
    expect((caught as ApiError).status).toBe(401);
    expect((caught as ApiError).message).toBe("Unauthorized");
  });
});

// ---------------------------------------------------------------------------
// createClient
// ---------------------------------------------------------------------------

describe("createClient", () => {
  let fetchSpy: ReturnType<typeof spyOn<typeof globalThis, "fetch">>;

  afterEach(() => {
    fetchSpy?.mockRestore();
  });

  const mockOkFetch = (body: unknown): void => {
    fetchSpy = spyOn(globalThis, "fetch").mockImplementation(
      (async () =>
        new Response(JSON.stringify(body), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        })) as unknown as typeof globalThis.fetch,
    );
  };

  const mockErrorFetch = (status: number, body: unknown = { error: "error" }): void => {
    fetchSpy = spyOn(globalThis, "fetch").mockImplementation(
      (async () =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        })) as unknown as typeof globalThis.fetch,
    );
  };

  // -- URL routing ----------------------------------------------------------

  test("uses the provided base URL", async () => {
    mockOkFetch({ value: 42 });
    const call = createClient("http://my-server.com");
    const route = def.get("/ping", "public", z.object({ value: z.number() }));
    await call(route, {});
    expect(fetchSpy.mock.calls).toHaveLength(1);
    expect(fetchSpy.mock.calls[0]![0]).toBe("http://my-server.com/ping");
  });

  test("two instances with different base URLs are fully independent", async () => {
    mockOkFetch({ value: 1 });
    const callA = createClient("http://server-a.com");
    const callB = createClient("http://server-b.com");
    const route = def.get("/ping", "public", z.object({ value: z.number() }));
    await callA(route, {});
    await callB(route, {});
    expect(fetchSpy.mock.calls[0]![0]).toBe("http://server-a.com/ping");
    expect(fetchSpy.mock.calls[1]![0]).toBe("http://server-b.com/ping");
  });

  test("baseUrlOverride in CallSettings takes precedence over the client base URL", async () => {
    mockOkFetch({ value: 1 });
    const call = createClient("http://default.com");
    const route = def.get("/ping", "public", z.object({ value: z.number() }));
    await call(route, {}, undefined, { baseUrlOverride: "http://override.com" });
    expect(fetchSpy.mock.calls[0]![0]).toBe("http://override.com/ping");
  });

  // -- Error handling -------------------------------------------------------

  test("throws ApiError on a 4xx response", async () => {
    mockErrorFetch(404, { message: "not found" });
    const call = createClient("http://example.com");
    const route = def.get("/missing", "public", z.object({ value: z.number() }));
    await expect(call(route, {})).rejects.toBeInstanceOf(ApiError);
  });

  test("ApiError carries the HTTP status code", async () => {
    mockErrorFetch(403, { message: "forbidden" });
    const call = createClient("http://example.com");
    const route = def.get("/private", "public", z.object({ value: z.number() }));
    try {
      await call(route, {});
      expect.unreachable("should have thrown");
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      expect((e as ApiError).status).toBe(403);
    }
  });

  test("throws ApiError on a 5xx response", async () => {
    mockErrorFetch(500, { message: "internal server error" });
    const call = createClient("http://example.com");
    const route = def.get("/boom", "public", z.object({ value: z.number() }));
    const err = await call(route, {}).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(500);
  });

  test("calls onUnauthorized callback when status is 401", async () => {
    mockErrorFetch(401, { message: "unauthorized" });
    let called = false;
    const call = createClient("http://example.com", {
      onUnauthorized: () => {
        called = true;
      },
    });
    const route = def.get("/secure", "public", z.object({ value: z.number() }));
    await expect(call(route, {})).rejects.toBeInstanceOf(ApiError);
    expect(called).toBe(true);
  });

  test("does not call onUnauthorized for 403 errors", async () => {
    mockErrorFetch(403);
    let called = false;
    const call = createClient("http://example.com", {
      onUnauthorized: () => {
        called = true;
      },
    });
    const route = def.get("/secure", "public", z.object({ value: z.number() }));
    await expect(call(route, {})).rejects.toBeInstanceOf(ApiError);
    expect(called).toBe(false);
  });

  test("calls onForbidden callback when status is 403", async () => {
    mockErrorFetch(403, { message: "forbidden" });
    let called = false;
    const call = createClient("http://example.com", {
      onForbidden: () => {
        called = true;
      },
    });
    const route = def.get("/admin", "public", z.object({ value: z.number() }));
    await expect(call(route, {})).rejects.toBeInstanceOf(ApiError);
    expect(called).toBe(true);
  });

  test("does not call onForbidden for 401 errors", async () => {
    mockErrorFetch(401, { message: "unauthorized" });
    let called = false;
    const call = createClient("http://example.com", {
      onForbidden: () => {
        called = true;
      },
    });
    const route = def.get("/admin", "public", z.object({ value: z.number() }));
    await expect(call(route, {})).rejects.toBeInstanceOf(ApiError);
    expect(called).toBe(false);
  });

  test("onUnauthorized and onForbidden fire independently on their respective statuses", async () => {
    let unauthorizedCalled = false;
    let forbiddenCalled = false;
    const call = createClient("http://example.com", {
      onUnauthorized: () => {
        unauthorizedCalled = true;
      },
      onForbidden: () => {
        forbiddenCalled = true;
      },
    });
    const route = def.get("/secure", "public", z.object({ value: z.number() }));

    mockErrorFetch(401);
    await expect(call(route, {})).rejects.toBeInstanceOf(ApiError);
    expect(unauthorizedCalled).toBe(true);
    expect(forbiddenCalled).toBe(false);

    unauthorizedCalled = false;
    mockErrorFetch(403);
    await expect(call(route, {})).rejects.toBeInstanceOf(ApiError);
    expect(unauthorizedCalled).toBe(false);
    expect(forbiddenCalled).toBe(true);
  });

  // -- Output validation ----------------------------------------------------

  test("returns validated data on a successful response", async () => {
    mockOkFetch({ value: 99 });
    const call = createClient("http://example.com");
    const route = def.get("/data", "public", z.object({ value: z.number() }));
    const result = await call(route, {});
    expect(result).toEqual({ value: 99 });
  });

  test("validateReturn defaults to true — rejects when response fails the output schema", async () => {
    mockOkFetch({ wrong: "shape" }); // does not match z.object({ value: z.number() })
    const call = createClient("http://example.com");
    const route = def.get("/data", "public", z.object({ value: z.number() }));
    await expect(call(route, {})).rejects.toThrow();
  });

  test("validateReturn: false skips output schema validation", async () => {
    mockOkFetch({ wrong: "shape" }); // schema mismatch, but validation is disabled
    const call = createClient("http://example.com");
    const route = def.get("/data", "public", z.object({ value: z.number() }));
    const result = await call(route, {}, undefined, { validateReturn: false });
    expect(result as unknown).toEqual({ wrong: "shape" });
  });

  // -- Credentials ----------------------------------------------------------

  test("does not send credentials by default", async () => {
    mockOkFetch({ value: 1 });
    const call = createClient("http://example.com");
    const route = def.get("/ping", "public", z.object({ value: z.number() }));
    await call(route, {});
    const init = fetchSpy.mock.calls[0]![1] as RequestInit;
    expect(init.credentials).toBeUndefined();
  });

  test("global withCredentials option sends credentials on every call", async () => {
    mockOkFetch({ value: 1 });
    const call = createClient("http://example.com", { withCredentials: true });
    const route = def.get("/ping", "public", z.object({ value: z.number() }));
    await call(route, {});
    const init = fetchSpy.mock.calls[0]![1] as RequestInit;
    expect(init.credentials).toBe("include");
  });

  test("per-call withCredentials overrides global withCredentials: false", async () => {
    mockOkFetch({ value: 1 });
    const call = createClient("http://example.com", { withCredentials: false });
    const route = def.get("/ping", "public", z.object({ value: z.number() }));
    await call(route, {}, undefined, { withCredentials: true });
    const init = fetchSpy.mock.calls[0]![1] as RequestInit;
    expect(init.credentials).toBe("include");
  });

  test("per-call withCredentials: false overrides global withCredentials: true", async () => {
    mockOkFetch({ value: 1 });
    const call = createClient("http://example.com", { withCredentials: true });
    const route = def.get("/ping", "public", z.object({ value: z.number() }));
    await call(route, {}, undefined, { withCredentials: false });
    const init = fetchSpy.mock.calls[0]![1] as RequestInit;
    expect(init.credentials).toBeUndefined();
  });
});
