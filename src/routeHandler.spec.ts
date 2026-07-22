import { describe, test, expect } from "bun:test";
import { RouteHandlerDefiner } from "./routeHandler.ts";
import { define } from "./routeDef.ts";
import { z } from "zod";
import { NotFoundError, RequestError, Unauthorized } from "./staticDefs.ts";

export const def = define<"">();

const getHandler = (
  options: { optionalQuery?: boolean; addNumber?: boolean; schema?: z.ZodTypeAny } = {
    optionalQuery: true,
  },
) => {
  const handle = RouteHandlerDefiner(
    async () => "ok",
    async () => ({}),
  );
  return handle(
    def.get(
      "/:id",
      "",
      z.unknown(),
      options.schema ??
        z.object({
          id: z.number(),
          name: options.optionalQuery ? z.string().min(2).max(100).optional() : z.string().min(2).max(100),
          "dotty.potty": z.string().min(2).max(100).optional(),
          "obj.different": z.number().optional(),
          obj: z
            .object({
              booboo: z.boolean().optional(),
              key: z.string(),
              num: options.addNumber ? z.number().min(0).max(100) : z.number().min(0).max(100).optional(),
              value: z
                .object({
                  name: z.string(),
                  age: z.number(),
                })
                .optional(),
            })
            .optional(),
          arr: z.array(z.number()).optional(),
          arrObj: z
            .array(
              z.object({
                name: z.string(),
                age: z.number(),
              }),
            )
            .optional(),
        }),
    ),
    async (p) => {
      return p;
    },
  );
};

describe("RouteHandler", () => {
  describe("url param handling", () => {
    test("should return 200 status if url param is present and valid", async () => {
      const req = new Request("https://example.com/2");
      const res = await getHandler().handlerWrapped(req);
      expect(res).toBeInstanceOf(Response);
      expect(res.status).toBe(200);
    });
    test("should return 400 status if url param is missing", async () => {
      const req = new Request("https://example.com/");
      const res = await getHandler().handlerWrapped(req);
      expect(res.status).toBe(400);
    });

    test("should return 400 status if url param is invalid", async () => {
      const req = new Request("https://example.com/invalid");
      const res = await getHandler().handlerWrapped(req);
      expect(res.status).toBe(400);
    });
    test("should return 400 status if url param is passed as query param", async () => {
      const req = new Request("https://example.com/?id=2");
      const res = await getHandler().handlerWrapped(req);
      expect(res.status).toBe(400);
    });
  });

  describe("query param handling", () => {
    test("should return 200 status if query param is present and valid", async () => {
      const req = new Request("https://example.com/1/?name=Joe");
      const res = await getHandler({ optionalQuery: false }).handlerWrapped(req);
      expect(res).toBeInstanceOf(Response);
      expect(res.status).toBe(200);
    });
    test("should return 400 status if query param is missing", async () => {
      const req = new Request("https://example.com/1/");
      const res = await getHandler({ optionalQuery: false }).handlerWrapped(req);
      expect(res.status).toBe(400);
    });

    test("for string params, it cannot differentiate between number, string or boolean, because they are all strings on url level", async () => {
      const req = new Request("https://example.com/1/?name=23");
      const res = await getHandler({ optionalQuery: false }).handlerWrapped(req);
      expect(res.status, await res.text()).toBe(200);

      const req2 = new Request("https://example.com/1/?name=true");
      const res2 = await getHandler({ optionalQuery: false }).handlerWrapped(req2);
      expect(res2.status, await res2.text()).toBe(200);
    });

    test("can handle object in the query params", async () => {
      const req = new Request("https://example.com/1/?name=Joe&obj.key=value");
      const res = await getHandler({ optionalQuery: false }).handlerWrapped(req);
      const json = await res.json();
      expect(res.status).toBe(200);
      expect(json).toEqual({ id: 1, name: "Joe", obj: { key: "value" } });
    });

    test("can handle numbers in objects in the query params", async () => {
      const req = new Request("https://example.com/1/?name=Joe&obj.key=value&obj.num=35");
      const res = await getHandler({ optionalQuery: false }).handlerWrapped(req);
      const json = await res.json();
      expect(res.status).toBe(200);
      expect(json).toEqual({ id: 1, name: "Joe", obj: { key: "value", num: 35 } });
    });

    test("can handle deep objects in the query params", async () => {
      const req = new Request("https://example.com/1/?name=Joe&obj.key=value&obj.value.name=deep&obj.value.age=35");
      const res = await getHandler({ optionalQuery: false }).handlerWrapped(req);
      const json = await res.json();
      expect(res.status).toBe(200);
      expect(json).toEqual({
        id: 1,
        name: "Joe",
        obj: { key: "value", value: { name: "deep", age: 35 } },
      });
    });

    test("can handle boolean values in objects in the query params", async () => {
      const req = new Request("https://example.com/1/?name=Joe&obj.key=value&obj.booboo=true");
      const res = await getHandler({ optionalQuery: false }).handlerWrapped(req);
      const json = await res.json();
      expect(res.status).toBe(200);
      expect(json).toEqual({
        id: 1,
        name: "Joe",
        obj: { key: "value", booboo: true },
      });
    });

    test("you can still have dots in the object names, as long as it's not interpretable as a nested object", async () => {
      const req = new Request("https://example.com/1/?dotty.potty=helloYou");
      const res = await getHandler({ optionalQuery: true }).handlerWrapped(req);
      expect(res.status).toBe(200);
      expect(await res.json()).toEqual({
        id: 1,
        "dotty.potty": "helloYou",
      });

      const invalidReq = new Request("https://example.com/1/?dotty.potty=helloYou&obj.different=32");
      const { status } = await getHandler({ optionalQuery: true }).handlerWrapped(invalidReq);
      expect(status).toBe(400);
    });

    test("can handle arrays", async () => {
      const req = new Request("https://example.com/1/?name=Joe&arr=1&arr=2");
      const res = await getHandler({ optionalQuery: false }).handlerWrapped(req);
      const json = await res.text();
      expect(res.status, json).toBe(200);
      expect(JSON.parse(json)).toEqual({
        id: 1,
        arr: [1, 2],
        name: "Joe",
      });
    });

    test("alternative array syntax with item index", async () => {
      const req = new Request("https://example.com/1/?name=Joe&arr.0=1&arr.1=2");
      const res = await getHandler({ optionalQuery: false }).handlerWrapped(req);
      const json = await res.text();
      expect(res.status, json).toBe(200);
      expect(JSON.parse(json)).toEqual({
        id: 1,
        arr: [1, 2],
        name: "Joe",
      });
    });

    test("can handle arrays of objects simple", async () => {
      const req = new Request("https://example.com/1/?arr.0.name=joe&arr.1.name=jane");
      const res = await getHandler({
        schema: z.object({ id: z.number(), arr: z.array(z.object({ name: z.string() })) }),
      }).handlerWrapped(req);
      const json = await res.text();
      expect(res.status, json).toBe(200);
      expect(JSON.parse(json)).toEqual({
        id: 1,
        arr: [{ name: "joe" }, { name: "jane" }],
      });
    });

    test("can handle arrays of objects", async () => {
      //SO deep down the number parsing does not work
      const req = new Request(
        "https://example.com/1/?name=Joe&arrObj.0.name=joe&arrObj.0.age=30&arrObj.1.name=jane&arrObj.1.age=25",
      );
      const res = await getHandler({ optionalQuery: false }).handlerWrapped(req);
      const json = await res.text();
      expect(res.status, json).toBe(200);
      expect(JSON.parse(json)).toEqual({
        id: 1,
        arrObj: [
          { name: "joe", age: 30 },
          { name: "jane", age: 25 },
        ],
        name: "Joe",
      });
    });

    describe("promisable properties", () => {
      const outputSchema = z.object({ title: z.string(), content: z.string() });
      const handle = RouteHandlerDefiner(
        async () => "ok",
        async () => ({}),
        { errorLogger: () => {} },
      );

      test("sync handler still returns JSON as before", async () => {
        const route = handle(def.get("/:id", "", outputSchema), () => ({ title: "Hello", content: "World" }));
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        expect(await res.json()).toEqual({ title: "Hello", content: "World" });
      });

      test("handler with promise properties resolves all for JSON response", async () => {
        const route = handle(def.get("/:id", "", outputSchema), () => ({
          title: "Hello",
          content: Promise.resolve("Async World"),
        }));
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        expect(await res.json()).toEqual({ title: "Hello", content: "Async World" });
      });

      test("handler with all promise properties resolves for JSON response", async () => {
        const route = handle(def.get("/:id", "", outputSchema), () => ({
          title: Promise.resolve("Async Hello"),
          content: Promise.resolve("Async World"),
        }));
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        expect(await res.json()).toEqual({ title: "Async Hello", content: "Async World" });
      });

      test("async handler returning promise properties resolves for JSON response", async () => {
        const route = handle(def.get("/:id", "", outputSchema), async () => ({
          title: "Hello",
          content: Promise.resolve("Async World"),
        }));
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        expect(await res.json()).toEqual({ title: "Hello", content: "Async World" });
      });

      test("formatOutput receives raw promises when handler returns promise properties", async () => {
        let receivedData!: { title: string; content: Promise<string> };
        const route = handle(
          def.get("/:id", "", outputSchema),
          () => ({
            title: "Hello",
            content: Promise.resolve("Async World"),
          }),
          async (data, _user, _req, _params) => {
            receivedData = data;
            // TypeScript knows data.title is `string` — no await needed
            // TypeScript knows data.content is `Promise<string>` — must await
            return {
              data: JSON.stringify({ title: data.title, content: await data.content }),
              headers: new Headers({ "Content-Type": "application/json" }),
            };
          },
        );
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        // formatOutput received the exact types — title is a plain string, content is a Promise
        expect(receivedData.title).toBe("Hello");
        expect(receivedData.content).toBeInstanceOf(Promise);
        expect(await res.json()).toEqual({ title: "Hello", content: "Async World" });
      });

      test("formatOutput with sync data — data properties are plain values, no await needed", async () => {
        const route = handle(
          def.get("/:id", "", outputSchema),
          () => ({ title: "Hello", content: "World" }),
          (data, _user, _req, _params) => ({
            // data.title and data.content are string — no await needed
            data: `<h1>${data.title}</h1><p>${data.content}</p>`,
            headers: new Headers({ "Content-Type": "text/html" }),
          }),
        );
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        expect(await res.text()).toBe("<h1>Hello</h1><p>World</p>");
      });

      test("formatOutput knows exact types — plain string properties need no await", async () => {
        const route = handle(
          def.get("/:id", "", outputSchema),
          () => ({
            title: "Hello",
            content: Promise.resolve("Async World"),
          }),
          (data) => {
            // data.title is exactly `string` — calling string methods directly is valid
            const shout: string = data.title.toUpperCase();
            // data.content is exactly `Promise<string>` — it cannot be used as a plain string
            const contentIsPromise: Promise<string> = data.content;
            const stream = new ReadableStream({
              async start(controller) {
                controller.enqueue(shout);
                controller.enqueue(await contentIsPromise);
                controller.close();
              },
            });
            return { data: stream, headers: new Headers({ "Content-Type": "text/plain" }) };
          },
        );
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(await res.text()).toBe("HELLOAsync World");
      });

      test("formatOutput can produce a ReadableStream for streaming SSR", async () => {
        const route = handle(
          def.get("/:id", "", outputSchema),
          () => ({
            title: "Hello",
            content: new Promise<string>((resolve) => setTimeout(() => resolve("Streamed!"), 10)),
          }),
          (data, _user, _req, _params) => {
            const stream = new ReadableStream({
              async start(controller) {
                controller.enqueue(`<h1>${data.title}</h1>`);
                const content = await data.content;
                controller.enqueue(`<p>${content}</p>`);
                controller.close();
              },
            });
            return {
              data: stream,
              headers: new Headers({ "Content-Type": "text/html" }),
            };
          },
        );
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        expect(await res.text()).toBe("<h1>Hello</h1><p>Streamed!</p>");
      });

      test("per-property validation fires outputErrorWarning for invalid async values", async () => {
        const warnings: { error: z.ZodError<unknown>; data: unknown; method: string; url: string }[] = [];
        const handleWithWarning = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
          { outputErrorWarning: (error, data, method, url) => warnings.push({ error, data, method, url }) },
        );
        const route = handleWithWarning(
          def.get("/:id", "", outputSchema),
          () => ({
            title: "Valid",
            content: Promise.resolve(12345 as unknown as string), // intentionally wrong type to test validation
          }),
          async (data, _user, _req, _params) => ({
            data: JSON.stringify({ title: data.title, content: await data.content }),
            headers: new Headers({ "Content-Type": "application/json" }),
          }),
        );
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        // value.then(warn) is attached before formatOutput runs, so warn fires before the response resolves
        expect(warnings.length).toBeGreaterThan(0);
        expect(warnings.some((w) => w.url.endsWith("#content"))).toBe(true);
      });

      test("JSON path validates resolved values and calls outputErrorWarning on mismatch", async () => {
        const warnings: { error: z.ZodError<unknown>; data: unknown; method: string; url: string }[] = [];
        const handleWithWarning = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
          { outputErrorWarning: (error, data, method, url) => warnings.push({ error, data, method, url }) },
        );
        const route = handleWithWarning(def.get("/:id", "", outputSchema), () => ({
          title: "Valid",
          content: Promise.resolve(999 as unknown as string), // intentionally wrong type to test validation
        }));
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        // Full-object validation fires on the resolved result
        expect(warnings.length).toBeGreaterThan(0);
      });

      test("JSON path strips extra properties after zod validation", async () => {
        const syncRoute = handle(def.get("/:id", "", outputSchema), () => ({
          title: "Hello",
          content: "World",
          extra: "should be stripped",
        }));
        const syncJson = await syncRoute.handlerWrapped(new Request("https://example.com/1")).then((r) => r.json());
        expect(syncJson).toEqual({ title: "Hello", content: "World" });
        expect(syncJson.extra).toBeUndefined();

        const asyncRoute = handle(def.get("/:id", "", outputSchema), () => ({
          title: Promise.resolve("Hello"),
          content: Promise.resolve("World"),
          extra: Promise.resolve("should be stripped"),
        }));
        const asyncJson = await asyncRoute.handlerWrapped(new Request("https://example.com/1")).then((r) => r.json());
        expect(asyncJson).toEqual({ title: "Hello", content: "World" });
        expect(asyncJson.extra).toBeUndefined();
      });

      // The typed handler surface (PromisableProperties) only admits Promises as top-level
      // object properties, so these shapes are only reachable from dynamically-typed handler
      // data. z.unknown() is used so the handlers below stay honestly typed without casts.
      describe("nested promise resolution (JSON path)", () => {
        const handle = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
        );

        const jsonOf = (data: unknown) =>
          handle(def.get("/:id", "", z.unknown()), () => data)
            .handlerWrapped(new Request("https://example.com/1"))
            .then((r) => r.json());

        test("promises inside array elements are resolved", async () => {
          const result = await jsonOf([{ name: Promise.resolve("a") }, { name: Promise.resolve("b") }]);
          expect(result).toEqual([{ name: "a" }, { name: "b" }]);
        });

        test("promises inside nested arrays are resolved", async () => {
          const result = await jsonOf([[Promise.resolve("a"), Promise.resolve("b")], [Promise.resolve("c")]]);
          expect(result).toEqual([["a", "b"], ["c"]]);
        });

        test("promises nested several levels deep are resolved", async () => {
          const result = await jsonOf({ rows: [{ tags: [Promise.resolve("x")] }] });
          expect(result).toEqual({ rows: [{ tags: ["x"] }] });
        });

        test("a top-level array of promises is still resolved", async () => {
          expect(await jsonOf([Promise.resolve("a"), Promise.resolve("b")])).toEqual(["a", "b"]);
        });

        test("a fully materialised result is passed through unchanged", async () => {
          const plain = [{ a: 1 }, { a: 2 }];
          expect(await jsonOf(plain)).toEqual(plain);
        });

        test("non-plain objects are values, not containers to traverse", async () => {
          const result = await jsonOf({ when: new Date("2020-01-01T00:00:00.000Z") });
          expect(result).toEqual({ when: "2020-01-01T00:00:00.000Z" });
        });

        test("non-plain objects stay untouched even when a sibling holds a promise", async () => {
          const result = await jsonOf({ v: Promise.resolve(1), when: new Date("2020-01-01T00:00:00.000Z") });
          expect(result).toEqual({ v: 1, when: "2020-01-01T00:00:00.000Z" });
        });

        // 0.1.x resolved direct Promise properties on any object, including class
        // instances — without this they reach JSON.stringify unresolved and serialise as {}.
        test("direct Promise properties on class instances are resolved", async () => {
          class Dto {
            name = Promise.resolve("a");
            plain = 1;
          }
          expect(await jsonOf(new Dto())).toEqual({ name: "a", plain: 1 });
        });

        test("class instances nested in plain structures get their promises resolved", async () => {
          class Dto {
            name = Promise.resolve("x");
          }
          expect(await jsonOf({ rows: [new Dto()] })).toEqual({ rows: [{ name: "x" }] });
        });
      });

      describe("primitive output schemas", () => {
        const handle = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
        );

        test("z.string() output — sync handler returns string as JSON", async () => {
          const route = handle(def.get("/:id", "", z.string()), async () => "hello");
          const res = await route.handlerWrapped(new Request("https://example.com/1"));
          expect(res.status).toBe(200);
          expect(await res.json()).toBe("hello");
        });

        test("z.string() output — async handler returning Promise<string>", async () => {
          const route = handle(def.get("/:id", "", z.string()), async () => Promise.resolve("hello async"));
          const res = await route.handlerWrapped(new Request("https://example.com/1"));
          expect(res.status).toBe(200);
          expect(await res.json()).toBe("hello async");
        });

        test("z.number() output — sync handler returns number as JSON", async () => {
          const route = handle(def.get("/:id", "", z.number()), async () => 42);
          const res = await route.handlerWrapped(new Request("https://example.com/1"));
          expect(res.status).toBe(200);
          expect(await res.json()).toBe(42);
        });

        test("z.boolean() output — sync handler", async () => {
          const route = handle(def.get("/:id", "", z.boolean()), async () => true);
          const res = await route.handlerWrapped(new Request("https://example.com/1"));
          expect(res.status).toBe(200);
          expect(await res.json()).toBe(true);
        });

        test("z.string() output — formatOutput receives the plain string directly", async () => {
          let receivedData: unknown;
          const route = handle(
            def.get("/:id", "", z.string()),
            async () => "hello",
            async (data) => {
              receivedData = data;
              return { data: `<p>${data}</p>`, headers: new Headers({ "Content-Type": "text/html" }) };
            },
          );
          const res = await route.handlerWrapped(new Request("https://example.com/1"));
          expect(res.status).toBe(200);
          expect(await res.text()).toBe("<p>hello</p>");
          expect(receivedData).toBe("hello");
          expect(typeof receivedData).toBe("string");
        });
      });

      test("formatOutput with redirect:true returns 303", async () => {
        const route = handle(
          def.get("/:id", "", outputSchema),
          () => ({ title: "Hello", content: "World" }),
          () => ({
            headers: new Headers({ Location: "/new-url" }),
            redirect: true,
          }),
        );
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(303);
        expect(res.headers.get("Location")).toBe("/new-url");
      });

      test("formatOutput with explicit status overrides default success code", async () => {
        const route = handle(
          def.get("/:id", "", outputSchema),
          () => ({ title: "Hello", content: "World" }),
          (data) => ({
            data: JSON.stringify({ title: data.title, content: data.content }),
            headers: new Headers({ "Content-Type": "application/json" }),
            status: 202,
          }),
        );
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(202);
      });

      test("promise rejection in JSON path returns 500", async () => {
        const route = handle(def.get("/:id", "", outputSchema), () => ({
          title: "Hello",
          content: new Promise<string>((_, reject) => reject(new Error("async failure"))),
        }));
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(500);
      });

      describe("array output with promise elements", () => {
        test("array of promises resolves to array, not a numeric-keyed object", async () => {
          const arraySchema = z.array(z.string());
          const route = handle(def.get("/:id", "", arraySchema), () => [
            Promise.resolve("a"),
            Promise.resolve("b"),
            Promise.resolve("c"),
          ]);
          const res = await route.handlerWrapped(new Request("https://example.com/1"));
          expect(res.status).toBe(200);
          const body = await res.json();
          expect(Array.isArray(body)).toBe(true);
          expect(body).toEqual(["a", "b", "c"]);
        });

        test("mixed array (plain values and promises) preserves order and shape", async () => {
          const arraySchema = z.array(z.string());
          const route = handle(def.get("/:id", "", arraySchema), () => ["first", Promise.resolve("second"), "third"]);
          const res = await route.handlerWrapped(new Request("https://example.com/1"));
          expect(res.status).toBe(200);
          const body = await res.json();
          expect(Array.isArray(body)).toBe(true);
          expect(body).toEqual(["first", "second", "third"]);
        });

        test("array without any promises is not affected by array resolution path", async () => {
          const arraySchema = z.array(z.string());
          const route = handle(def.get("/:id", "", arraySchema), () => ["x", "y", "z"]);
          const res = await route.handlerWrapped(new Request("https://example.com/1"));
          expect(res.status).toBe(200);
          const body = await res.json();
          expect(Array.isArray(body)).toBe(true);
          expect(body).toEqual(["x", "y", "z"]);
        });

        test("promise rejection inside array returns 500", async () => {
          const arraySchema = z.array(z.string());
          const route = handle(def.get("/:id", "", arraySchema), () => [
            "ok",
            new Promise<string>((_, reject) => reject(new Error("boom"))),
          ]);
          const res = await route.handlerWrapped(new Request("https://example.com/1"));
          expect(res.status).toBe(500);
        });
      });

      test("outputErrorWarning fires for sync handler returning wrong type", async () => {
        const warnings: { error: z.ZodError<unknown>; data: unknown }[] = [];
        const handleWithWarning = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
          { outputErrorWarning: (error, data) => warnings.push({ error, data }) },
        );
        const route = handleWithWarning(def.get("/:id", "", outputSchema), () => ({
          title: "Hello",
          content: 999 as unknown as string,
        }));
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        expect(warnings.length).toBeGreaterThan(0);
      });
    });
  });

  describe("auth and permissions", () => {
    test("returns 403 when authorizer returns 'forbidden'", async () => {
      const handle = RouteHandlerDefiner(
        async () => "forbidden",
        async () => ({}),
      );
      const route = handle(def.get("/:id", "", z.object({ id: z.string() })), (params) => ({ id: params.id }));
      const res = await route.handlerWrapped(new Request("https://example.com/1"));
      expect(res.status).toBe(403);
    });

    test("returns 401 when authorizer returns 'unauthenticated'", async () => {
      const handle = RouteHandlerDefiner(
        async () => "unauthenticated",
        async () => ({}),
      );
      const route = handle(def.get("/:id", "", z.object({ id: z.string() })), (params) => ({ id: params.id }));
      const res = await route.handlerWrapped(new Request("https://example.com/1"));
      expect(res.status).toBe(401);
    });
  });

  describe("body handling (POST)", () => {
    const bodySchema = z.object({ name: z.string(), age: z.number() });
    const handle = RouteHandlerDefiner(
      async () => "ok",
      async () => ({}),
    );
    const route = handle(
      def.post("/item", "", bodySchema, z.object({ name: z.string(), age: z.number() })),
      (_params, body) => ({ name: body.name, age: body.age }),
    );

    test("POST with valid JSON body returns 201", async () => {
      const res = await route.handlerWrapped(
        new Request("https://example.com/item", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ name: "Joe", age: 30 }),
        }),
      );
      expect(res.status).toBe(201);
      expect(await res.json()).toEqual({ name: "Joe", age: 30 });
    });

    test("POST with invalid JSON returns 400", async () => {
      const res = await route.handlerWrapped(
        new Request("https://example.com/item", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: "not-json{",
        }),
      );
      expect(res.status).toBe(400);
    });

    test("POST body failing schema validation returns 400", async () => {
      const res = await route.handlerWrapped(
        new Request("https://example.com/item", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ name: "Joe", age: "not-a-number" }),
        }),
      );
      expect(res.status).toBe(400);
    });

    test("POST with unsupported content type returns 415", async () => {
      const res = await route.handlerWrapped(
        new Request("https://example.com/item", {
          method: "POST",
          headers: { "Content-Type": "text/plain" },
          body: "hello",
        }),
      );
      expect(res.status).toBe(415);
    });

    test("POST with no Content-Type and empty body passes when schema is nullish", async () => {
      const routeNullish = handle(
        def.post(
          "/item-nullish",
          "",
          z.object({ name: z.string(), age: z.number() }).nullish(),
          z.object({}).nullish(),
        ),
        () => null,
      );
      const res = await routeNullish.handlerWrapped(
        new Request("https://example.com/item-nullish", { method: "POST" }),
      );
      expect(res.status).toBe(201);
    });

    test("POST with no Content-Type and empty body returns 400 when schema requires an object", async () => {
      const res = await route.handlerWrapped(new Request("https://example.com/item", { method: "POST" }));
      expect(res.status).toBe(400);
      const text = await res.text();
      expect(text).not.toBe("Body is not valid JSON");
      expect(text).toContain("Body does not match defined schema");
    });

    test("POST with form data coerces numbers and returns 201", async () => {
      const formData = new FormData();
      formData.append("name", "Joe");
      formData.append("age", "30");
      const res = await route.handlerWrapped(
        new Request("https://example.com/item", { method: "POST", body: formData }),
      );
      expect(res.status).toBe(201);
      expect(await res.json()).toEqual({ name: "Joe", age: 30 });
    });
  });

  describe("error handling", () => {
    const handle = RouteHandlerDefiner(
      async () => "ok",
      async () => ({}),
      { errorLogger: () => {} },
    );

    test("handler throwing NotFoundError returns 404", async () => {
      const route = handle(def.get("/:id", "", z.object({})), () => {
        throw new NotFoundError();
      });
      const res = await route.handlerWrapped(new Request("https://example.com/1"));
      expect(res.status).toBe(404);
    });

    test("handler throwing RequestError returns its status code", async () => {
      const route = handle(def.get("/:id", "", z.object({})), () => {
        throw new RequestError(422, "Unprocessable Entity");
      });
      const res = await route.handlerWrapped(new Request("https://example.com/1"));
      expect(res.status).toBe(422);
    });

    test("handler throwing a generic Error returns 500", async () => {
      const route = handle(def.get("/:id", "", z.object({})), () => {
        throw new Error("something went wrong");
      });
      const res = await route.handlerWrapped(new Request("https://example.com/1"));
      expect(res.status).toBe(500);
    });

    describe("errorParser", () => {
      test("uses status and message from errorParser when it returns a value", async () => {
        const handle = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
          {
            errorLogger: () => {},
            errorParser: async (error) => {
              if (error instanceof RangeError) {
                return { status: 400, message: "range error caught by parser" };
              }
              return undefined;
            },
          },
        );
        const route = handle(def.get("/:id", "", z.object({})), () => {
          throw new RangeError("out of range");
        });
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(400);
        expect(await res.text()).toContain("range error caught by parser");
      });

      test("falls through to built-in NotFoundError handler when errorParser returns undefined", async () => {
        const handle = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
          {
            errorLogger: () => {},
            errorParser: async (_error) => undefined,
          },
        );
        const route = handle(def.get("/:id", "", z.object({})), () => {
          throw new NotFoundError();
        });
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(404);
      });

      test("errorParser overrides built-in NotFoundError handler when it returns a value", async () => {
        const handle = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
          {
            errorLogger: () => {},
            errorParser: async (error) => {
              if (error instanceof NotFoundError) {
                return { status: 410, message: "gone, overridden by parser" };
              }
              return undefined;
            },
          },
        );
        const route = handle(def.get("/:id", "", z.object({})), () => {
          throw new NotFoundError();
        });
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(410);
        expect(await res.text()).toContain("gone, overridden by parser");
      });

      test("errorParser overrides built-in Unauthorized handler when it returns a value", async () => {
        const handle = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
          {
            errorLogger: () => {},
            errorParser: async (error) => {
              if (error instanceof Unauthorized) {
                return { status: 418, message: "unauthorized, overridden by parser" };
              }
              return undefined;
            },
          },
        );
        const route = handle(def.get("/:id", "", z.object({})), () => {
          throw new Unauthorized(401);
        });
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(418);
        expect(await res.text()).toContain("unauthorized, overridden by parser");
      });
    });

    describe("errorHtmlFormatter", () => {
      test("500 with HTML Accept → formatter output is the body, Content-Type is text/html", async () => {
        const handle = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
          {
            errorLogger: () => {},
            errorHtmlFormatter: async (_status, _message) => "<h1>Custom 500</h1>",
          },
        );
        const route = handle(def.get("/:id", "", z.object({})), () => {
          throw new Error("something went wrong");
        });
        const res = await route.handlerWrapped(
          new Request("https://example.com/1", { headers: { Accept: "text/html" } }),
        );
        expect(res.status).toBe(500);
        expect(await res.text()).toBe("<h1>Custom 500</h1>");
        expect(res.headers.get("Content-Type")).toBe("text/html");
      });

      test("404 (NotFoundError) with HTML Accept → formatter is called", async () => {
        const handle = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
          {
            errorHtmlFormatter: async (_status, _message) => "<h1>Not Found</h1>",
          },
        );
        const route = handle(def.get("/:id", "", z.object({})), () => {
          throw new NotFoundError();
        });
        const res = await route.handlerWrapped(
          new Request("https://example.com/1", { headers: { Accept: "text/html" } }),
        );
        expect(res.status).toBe(404);
        expect(await res.text()).toBe("<h1>Not Found</h1>");
      });

      test("403 (Unauthorized) with HTML Accept → formatter is called", async () => {
        const handle = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
          {
            errorHtmlFormatter: async (_status, _message) => "<h1>Forbidden</h1>",
          },
        );
        const route = handle(def.get("/:id", "", z.object({})), () => {
          throw new Unauthorized(403, "No access");
        });
        const res = await route.handlerWrapped(
          new Request("https://example.com/1", { headers: { Accept: "text/html" } }),
        );
        expect(res.status).toBe(403);
        expect(await res.text()).toBe("<h1>Forbidden</h1>");
      });

      test("400 param validation error with HTML Accept → formatter is called", async () => {
        const handle = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
          {
            errorHtmlFormatter: async (_status, _message) => "<h1>Bad Request</h1>",
          },
        );
        const route = handle(def.get("/:id", "", z.object({}), z.object({ id: z.number() })), async () => ({}));
        const res = await route.handlerWrapped(
          new Request("https://example.com/not-a-number", { headers: { Accept: "text/html" } }),
        );
        expect(res.status).toBe(400);
        expect(await res.text()).toBe("<h1>Bad Request</h1>");
      });

      test("JSON Accept → formatter is NOT called, error returned as JSON", async () => {
        let formatterCalled = false;
        const handle = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
          {
            errorLogger: () => {},
            errorHtmlFormatter: async (_status, _message) => {
              formatterCalled = true;
              return "<h1>Error</h1>";
            },
          },
        );
        const route = handle(def.get("/:id", "", z.object({})), () => {
          throw new Error("something went wrong");
        });
        const res = await route.handlerWrapped(
          new Request("https://example.com/1", { headers: { Accept: "application/json" } }),
        );
        expect(formatterCalled).toBe(false);
        expect(res.status).toBe(500);
        expect(await res.json()).toBeDefined();
      });

      test("formatter receives the correct user argument", async () => {
        let capturedUser: { name: string } | undefined;
        const handle = RouteHandlerDefiner(
          async () => "ok",
          async () => ({ name: "Alice" }),
          {
            errorHtmlFormatter: async (_status, _message, _request, user) => {
              capturedUser = user;
              return "<h1>Not Found</h1>";
            },
          },
        );
        const route = handle(def.get("/:id", "", z.object({})), () => {
          throw new NotFoundError();
        });
        const res = await route.handlerWrapped(
          new Request("https://example.com/1", { headers: { Accept: "text/html" } }),
        );
        expect(res.status).toBe(404);
        expect(capturedUser?.name).toBe("Alice");
      });
    });
  });
});
