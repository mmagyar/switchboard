import { expect, test } from "bun:test";
import { Router } from "./router.ts";
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
    const res = await route?.handler({} as any);
    expect(await res?.text()).toBe(expected);
  };

  genTester("/hello", "HELLO");
  genTester("/hello/3/item", "ITEM");
  genTester("/hello/3/item/4", "HELLOITEMID");
  genTester("/hello/3", "HELLOID");
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
    const res = await route?.handler({} as any);
    expect(await res?.text()).toBe(expected);
  };

  genTester("/hello", "HELLO");
  genTester("/hello/3/item", "ITEM");
  genTester("/hello/3/item/4", "HELLOITEMID");
  genTester("/hello/3", "HELLOID");
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
  expect(params).toStrictEqual({ id: "3", name: "ohai" });
});

test("throws if a non optional parameter follows an optional", () => {
  const r = new Router();
  //cast is needed since we are actually disallowing it on a type level
  expect(() =>
    r.addRoute("get", "/hello/:id?/:name" as any, () => new Response("HELLO")),
  ).toThrow();
});
