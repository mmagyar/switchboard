import { expect, test } from "bun:test";
import { Router, type RegisterableRoute } from "./router.ts";
import { extractParams } from "./urlUtils.ts";

const url = "https://example.com";

test("router match simple route", () => {
  const r = new Router();
  r.addRoute("get", "/hello", () => new Response("Hello"));
  const route = r.getRoute("get", new URL("/hello", url));
  expect(route).toBeDefined();
  //negative test
  const route2 = r.getRoute("get", new URL("/hello2", url));
  expect(route2).toBeUndefined();
  //test for not matching different method
  const route3 = r.getRoute("post", new URL("/hello", url));
  expect(route3).toBeUndefined();
});

test("router match route with param", () => {
  const r = new Router();
  r.addRoute("get", "/hello/:name", (_req) => new Response("Hello "));
  const route = r.getRoute("get", new URL("/hello/John", url));
  expect(route).toBeDefined();
  //negative test
  const route2 = r.getRoute("get", new URL("/hello", url));
  expect(route2).toBeUndefined();
  //test for not matching different method
  const route3 = r.getRoute("post", new URL("/hello/John", url));
  expect(route3).toBeUndefined();
});

test("extract params from route", () => {
  const r = new Router();
  r.addRoute("get", "/hello/:name", (_req) => new Response("Hello "));
  const route = r.getRoute("get", new URL("/hello/John", url));
  expect(route).toBeDefined();
  const params = extractParams(route!.route, new URL("/hello/John", url));
  expect(params).toEqual({ name: "John" });
});

test("don't match early a not fully conforming route", async () => {
  const r = new Router();
  r.addRoute("get", "/hello/:id", () => new Response("HELLOID"));
  r.addRoute("get", "/hello/:id/item/:item_id", (_req) => new Response("HELLOITEMID"));
  r.addRoute("get", "/hello/:id/item/", () => new Response("ITEM"));
  r.addRoute("get", "/hello", () => new Response("HELLO"));

  const genTester = async (path: string, expected: string) => {
    const route = r.getRoute("get", new URL(path, url));
    expect(route).toBeDefined();
    const res = await route?.handler(new Request("https://example.com"));
    expect(await res?.text()).toBe(expected);
  };

  await genTester("/hello/123", "HELLOID");
  await genTester("/hello/123/item/456", "HELLOITEMID");
  await genTester("/hello/123/item/", "ITEM");
  await genTester("/hello", "HELLO");
});

test("Handle routes not starting with a slash", async () => {
  const r = new Router();
  r.addRoute("get", "hello/:id", () => new Response("HELLOID"));
  r.addRoute("get", "hello/:id/item/:item_id", (_req) => new Response("HELLOITEMID"));
  r.addRoute("get", "hello/:id/item/", () => new Response("ITEM"));
  r.addRoute("get", "hello", () => new Response("HELLO"));

  const genTester = async (path: string, expected: string) => {
    const route = r.getRoute("get", new URL(path, url));
    expect(route).toBeDefined();
    const res = await route?.handler(new Request("https://example.com"));
    expect(await res?.text()).toBe(expected);
  };

  await genTester("/hello/123", "HELLOID");
  await genTester("/hello/123/item/456", "HELLOITEMID");
  await genTester("/hello/123/item/", "ITEM");
  await genTester("/hello", "HELLO");
});

test("handle optional path arguments", () => {
  const r = new Router();
  r.addRoute("get", "/hello/:id/:name?", (_req) => new Response("HELLO"));
  const route2 = r.getRoute("get", new URL("/hello/3/John", url));
  expect(route2).toBeDefined();
  const route = r.getRoute("get", new URL("/hello/3", url));
  expect(route).toBeDefined();
  const route3 = r.getRoute("get", new URL("/hello/3?hell=o", url));
  expect(route3).toBeDefined();
  //test extract
  const params = extractParams(route2!.route, new URL("/hello/3/ohai", url));
  expect(params).toEqual({ id: "3", name: "ohai" });
});

test("throws if a non optional parameter follows an optional", () => {
  const r = new Router();
  //cast is needed since we are actually disallowing it on a type level
  expect(() => r.addRoute("get", "/hello/:id?/:name" as any, () => new Response("HELLO"))).toThrow();
});

test("throws on duplicate route registration", () => {
  const r = new Router();
  r.addRoute("get", "/hello", () => new Response("Hello"));
  expect(() => r.addRoute("get", "/hello", () => new Response("Hello2"))).toThrow();
  // different method on same path is allowed
  expect(() => r.addRoute("post", "/hello", () => new Response("Post"))).not.toThrow();
});

test("throws when mandatory param conflicts with existing optional param at same position", () => {
  const r = new Router();
  r.addRoute("get", "/hello/:id?", () => new Response("OPTIONAL"));
  expect(() => r.addRoute("get", "/hello/:id", () => new Response("MANDATORY"))).toThrow(/conflicting param edges/i);
});

test("throws when optional param conflicts with existing mandatory param at same position", () => {
  const r = new Router();
  r.addRoute("get", "/hello/:id", () => new Response("MANDATORY"));
  expect(() => r.addRoute("get", "/hello/:id?", () => new Response("OPTIONAL"))).toThrow(/conflicting param edges/i);
});

test("throws on mandatory/optional param conflict even across different methods", () => {
  const r = new Router();
  r.addRoute("get", "/hello/:id", () => new Response("GET MANDATORY"));
  // POST route with optional param at the same position still shares the trie node
  expect(() => r.addRoute("post", "/hello/:id?", () => new Response("POST OPTIONAL"))).toThrow(
    /conflicting param edges/i,
  );
});

test("handleRequest dispatches to the matching handler", async () => {
  const r = new Router();
  r.addRoute("get", "/hello/:name", (req) => {
    const url = new URL(req.url);
    return new Response(`hi ${extractParams("/hello/:name", url)["name"]}`);
  });
  const res = await r.handleRequest(new Request("https://example.com/hello/world"));
  expect(res.status).toBe(200);
  expect(await res.text()).toBe("hi world");
});

test("handleRequest falls back to defaultRoute when no route matches", async () => {
  const r = new Router();
  r.addRoute("get", "/exists", () => new Response("found"));
  const res = await r.handleRequest(new Request("https://example.com/missing"));
  expect(res.status).toBe(404);

  const custom = new Router(() => new Response("custom 404", { status: 404 }));
  const res2 = await custom.handleRequest(new Request("https://example.com/missing"));
  expect(res2.status).toBe(404);
  expect(await res2.text()).toBe("custom 404");
});

test("trie: static segments take priority over param segments", async () => {
  const r = new Router();
  r.addRoute("get", "/users/me", () => new Response("ME"));
  r.addRoute("get", "/users/:id", () => new Response("USER"));

  const meRoute = r.getRoute("get", new URL("/users/me", url));
  expect(await (await meRoute?.handler(new Request("https://example.com")))?.text()).toBe("ME");

  const idRoute = r.getRoute("get", new URL("/users/42", url));
  expect(await (await idRoute?.handler(new Request("https://example.com")))?.text()).toBe("USER");
});

test("trie: deeply nested static + param routes resolve correctly", async () => {
  const r = new Router();
  r.addRoute("get", "/a/b/c", () => new Response("STATIC"));
  r.addRoute("get", "/a/:x/c", () => new Response("PARAM"));

  const staticRoute = r.getRoute("get", new URL("/a/b/c", url));
  expect(await (await staticRoute?.handler(new Request("https://example.com")))?.text()).toBe("STATIC");

  const paramRoute = r.getRoute("get", new URL("/a/other/c", url));
  expect(await (await paramRoute?.handler(new Request("https://example.com")))?.text()).toBe("PARAM");
});

test("trie: multiple optional trailing params", async () => {
  const r = new Router();
  r.addRoute("get", "/search/:term?/:page?", () => new Response("SEARCH"));

  expect(r.getRoute("get", new URL("/search", url))).toBeDefined();
  expect(r.getRoute("get", new URL("/search/foo", url))).toBeDefined();
  expect(r.getRoute("get", new URL("/search/foo/2", url))).toBeDefined();
});

test("trie: different methods on same path are independent", async () => {
  const r = new Router();
  r.addRoute("get", "/resource", () => new Response("GET"));
  r.addRoute("post", "/resource", () => new Response("POST"));
  r.addRoute("delete", "/resource", () => new Response("DELETE"));

  expect(
    await (await r.getRoute("get", new URL("/resource", url))?.handler(new Request("https://example.com")))?.text(),
  ).toBe("GET");
  expect(
    await (await r.getRoute("post", new URL("/resource", url))?.handler(new Request("https://example.com")))?.text(),
  ).toBe("POST");
  expect(
    await (await r.getRoute("delete", new URL("/resource", url))?.handler(new Request("https://example.com")))?.text(),
  ).toBe("DELETE");
});

test("HEAD falls back to GET handler, preserving status and headers with no body", async () => {
  const r = new Router();
  r.addRoute(
    "get",
    "/resource",
    () =>
      new Response("GET body", {
        status: 200,
        headers: { "X-Custom": "header-value", "Content-Type": "text/plain" },
      }),
  );
  const res = await r.handleRequest(new Request("https://example.com/resource", { method: "HEAD" }));
  expect(res.status).toBe(200);
  expect(res.headers.get("X-Custom")).toBe("header-value");
  expect(await res.text()).toBe("");
});

test("HEAD uses registered HEAD handler directly when one exists", async () => {
  const r = new Router();
  r.addRoute("head", "/resource", () => new Response(null, { status: 204, headers: { "X-From": "head-handler" } }));
  r.addRoute("get", "/resource", () => new Response("GET body", { headers: { "X-From": "get-handler" } }));
  const res = await r.handleRequest(new Request("https://example.com/resource", { method: "HEAD" }));
  expect(res.status).toBe(204);
  expect(res.headers.get("X-From")).toBe("head-handler");
});

test("HEAD falls through to defaultRoute when no GET handler is registered", async () => {
  const r = new Router();
  r.addRoute("get", "/other", () => new Response("other"));
  const res = await r.handleRequest(new Request("https://example.com/missing", { method: "HEAD" }));
  expect(res.status).toBe(404);
});

test("unknown HTTP method returns 405 Method Not Allowed even when a matching path exists", async () => {
  const r = new Router();
  r.addRoute("get", "/resource", () => new Response("tea"));
  // CONNECT is not in our supported method list; the registered GET route must not be reached
  const res = await r.handleRequest(new Request("https://example.com/resource", { method: "CONNECT" }));
  expect(res.status).toBe(405);
  expect(await res.text()).toContain("Method Not Allowed");
});

test("unknown HTTP method returns 405 even when no routes are registered", async () => {
  const r = new Router();
  const res = await r.handleRequest(new Request("https://example.com/anything", { method: "CONNECT" }));
  expect(res.status).toBe(405);
});

// ── conflicting param-name detection ─────────────────────────────────────────

test("throws when a second route uses a different param name at the same trie position", () => {
  const r = new Router();
  r.addRoute("get", "/users/:id", () => new Response("by id"));
  // ":name" is structurally identical to ":id" but has a different label —
  // both routes would share the same paramChild node and the first handler
  // would be silently overwritten without the guard.
  expect(() => r.addRoute("get", "/users/:name", () => new Response("by name"))).toThrow(/conflicting param names/i);
});

test("throws on param-name conflict even when the methods differ", () => {
  // The trie is shared across methods, so a POST with ":name" at the same
  // position as a GET with ":id" must also be rejected.
  const r = new Router();
  r.addRoute("get", "/users/:id", () => new Response("GET by id"));
  expect(() => r.addRoute("post", "/users/:name", () => new Response("POST by name"))).toThrow(
    /conflicting param names/i,
  );
});

test("throws on param-name conflict at a deeply nested position", () => {
  const r = new Router();
  r.addRoute("get", "/a/:x/b", () => new Response("x"));
  expect(() => r.addRoute("get", "/a/:y/b", () => new Response("y"))).toThrow(/conflicting param names/i);
});

test("throws when param-name conflict also involves an optionality mismatch", () => {
  // A different name *and* different optionality — the name check fires first
  // and still produces a clear "conflicting param names" error.
  const r = new Router();
  r.addRoute("get", "/users/:id", () => new Response("mandatory id"));
  expect(() => r.addRoute("get", "/users/:name?", () => new Response("optional name"))).toThrow(
    /conflicting param names/i,
  );
});

test("does not throw when the same param name is reused across different methods", () => {
  // Identical structural routes under different HTTP methods are fine; they
  // share the trie node but each method gets its own handler entry.
  const r = new Router();
  r.addRoute("get", "/users/:id", () => new Response("GET"));
  expect(() => r.addRoute("post", "/users/:id", () => new Response("POST"))).not.toThrow();
});

test("does not throw when the same param name is reused at the same position in a longer path", () => {
  const r = new Router();
  r.addRoute("get", "/users/:id/posts", () => new Response("posts"));
  expect(() => r.addRoute("get", "/users/:id/comments", () => new Response("comments"))).not.toThrow();
});

test("addRoute accepts a RegisterableRoute object", async () => {
  const r = new Router();
  const route: RegisterableRoute = {
    method: "get",
    path: "/items/:id",
    handlerWrapped: () => new Response("item"),
  };
  r.addRoute(route);
  const found = r.getRoute("get", new URL("/items/42", url));
  expect(found).toBeDefined();
  expect(await (await found!.handler(new Request("https://example.com")))?.text()).toBe("item");
});

test("addRoute route-object overload throws on duplicate registration", () => {
  const r = new Router();
  const route: RegisterableRoute = {
    method: "post",
    path: "/orders",
    handlerWrapped: () => new Response("order"),
  };
  r.addRoute(route);
  expect(() => r.addRoute(route)).toThrow(/duplicate route/i);
});

test("addRoute route-object normalises paths without leading slash", async () => {
  const r = new Router();
  r.addRoute({ method: "get", path: "no-slash", handlerWrapped: () => new Response("ok") });
  const found = r.getRoute("get", new URL("/no-slash", url));
  expect(found).toBeDefined();
});

// ── prefixes ──────────────────────────────────────────────────────────────────

test("addRoute registers all prefix paths from a RegisterableRoute", async () => {
  const r = new Router();
  const route: RegisterableRoute = {
    method: "get",
    path: "/list/:itemId",
    prefixes: ["hu", "en"],
    handlerWrapped: () => new Response("item"),
  };
  r.addRoute(route);

  expect(r.getRoute("get", new URL("/list/42", url))).toBeDefined();
  expect(r.getRoute("get", new URL("/hu/list/42", url))).toBeDefined();
  expect(r.getRoute("get", new URL("/en/list/42", url))).toBeDefined();
});

test("canonical path dispatches with undefined prefix", async () => {
  const r = new Router();
  const receivedPrefixes: Array<string | undefined> = [];
  const route: RegisterableRoute = {
    method: "get",
    path: "/list/:itemId",
    prefixes: ["hu"],
    handlerWrapped: (_req, prefix) => {
      receivedPrefixes.push(prefix);
      return new Response("ok");
    },
  };
  r.addRoute(route);

  await (await r.handleRequest(new Request(`${url}/list/1`))).text();
  expect(receivedPrefixes).toEqual([undefined]);
});

test("prefix path dispatches with correct prefix string", async () => {
  const r = new Router();
  const receivedPrefixes: Array<string | undefined> = [];
  const route: RegisterableRoute = {
    method: "get",
    path: "/list/:itemId",
    prefixes: ["hu", "en"],
    handlerWrapped: (_req, prefix) => {
      receivedPrefixes.push(prefix);
      return new Response("ok");
    },
  };
  r.addRoute(route);

  await (await r.handleRequest(new Request(`${url}/hu/list/1`))).text();
  await (await r.handleRequest(new Request(`${url}/en/list/1`))).text();
  expect(receivedPrefixes).toEqual(["hu", "en"]);
});

test("prefix handler receives URL with prefix stripped (params extracted correctly)", async () => {
  const r = new Router();
  let receivedUrl = "";
  const route: RegisterableRoute = {
    method: "get",
    path: "/list/:itemId",
    prefixes: ["hu"],
    handlerWrapped: (req, _prefix) => {
      receivedUrl = new URL(req.url).pathname;
      return new Response("ok");
    },
  };
  r.addRoute(route);

  await (await r.handleRequest(new Request(`${url}/hu/list/123`))).text();
  expect(receivedUrl).toBe("/list/123");
});

test("throws when prefix path duplicates an already-registered path", () => {
  const r = new Router();
  r.addRoute("get", "/hu/list/:itemId", () => new Response("existing"));
  const route: RegisterableRoute = {
    method: "get",
    path: "/list/:itemId",
    prefixes: ["hu"],
    handlerWrapped: () => new Response("new"),
  };
  expect(() => r.addRoute(route)).toThrow(/duplicate route/i);
});

test("throws when two routes produce the same prefixed path", () => {
  const r = new Router();
  const route1: RegisterableRoute = {
    method: "get",
    path: "/list/:itemId",
    prefixes: ["hu"],
    handlerWrapped: () => new Response("first"),
  };
  const route2: RegisterableRoute = {
    method: "get",
    path: "/list/:itemId",
    prefixes: ["hu"],
    handlerWrapped: () => new Response("second"),
  };
  r.addRoute(route1);
  expect(() => r.addRoute(route2)).toThrow(/duplicate route/i);
});

// ── trie static-priority + fallback behaviour ─────────────────────────────────

test("trie: static 'me' node takes priority for /users/me, but /users/me/posts falls back to param branch", async () => {
  // When the trie walks /users/me/posts it first tries the static "me" child.
  // That node has no "posts" child, so the trie falls back and retries the
  // segment against the ":id" param child, successfully matching
  // /users/:id/posts with id="me". Static priority therefore does NOT prevent
  // a longer param-based route from being found.
  const r = new Router();
  r.addRoute("get", "/users/me", () => new Response("ME"));
  r.addRoute("get", "/users/:id/posts", () => new Response("POSTS"));

  // /users/me — static branch wins exactly
  const meRoute = r.getRoute("get", new URL("/users/me", url));
  expect(await (await meRoute?.handler(new Request("https://example.com")))?.text()).toBe("ME");

  // /users/42/posts — param branch, no ambiguity
  const paramRoute = r.getRoute("get", new URL("/users/42/posts", url));
  expect(await (await paramRoute?.handler(new Request("https://example.com")))?.text()).toBe("POSTS");

  // /users/me/posts — static "me" node has no /posts child; trie falls back
  // to the ":id" param branch and matches /users/:id/posts with id="me"
  const fallbackRoute = r.getRoute("get", new URL("/users/me/posts", url));
  expect(await (await fallbackRoute?.handler(new Request("https://example.com")))?.text()).toBe("POSTS");
});
