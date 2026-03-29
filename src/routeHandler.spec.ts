import { describe, test, expect } from "bun:test";
import { RouteHandlerDefiner, type FormatOutput, type FormatStreamOutput, type HandlerWithoutBodyFn } from "./routeHandler.ts";
import { define, type UrlParamsSchema } from "./routeDef.ts";
import { z } from "zod";
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
      z.any(),
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
export const def = define<"">();
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
      const json = await res.text();
      expect(res.status).toBe(200);
      expect(JSON.parse(json)).toEqual({
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

    test("alterantive array syntax with item index", async () => {
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

      const makeHandler = <HANDLER extends HandlerWithoutBodyFn<{}, UrlParamsSchema<"/:id">, typeof outputSchema>>(
        handler: HANDLER,
        opts?: {
          formatOutput?: FormatOutput<UrlParamsSchema<"/:id">, typeof outputSchema, {}>;
          formatStreamOutput?: FormatStreamOutput<UrlParamsSchema<"/:id">, typeof outputSchema, {}, Awaited<ReturnType<HANDLER>>>;
          outputErrorWarning?: (error: z.ZodError<unknown>, data: unknown, method: string, url: string) => void;
        },
      ) => {
        const handle = RouteHandlerDefiner(
          async () => "ok",
          async () => ({}),
          opts?.outputErrorWarning,
        );
        return handle(def.get("/:id", "", outputSchema), handler, opts?.formatOutput, opts?.formatStreamOutput);
      };

      test("sync handler still returns JSON as before", async () => {
        const route = makeHandler(() => ({ title: "Hello", content: "World" }));
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        expect(await res.json()).toEqual({ title: "Hello", content: "World" });
      });

      test("handler with promise properties resolves all for JSON response", async () => {
        const route = makeHandler(() => ({
          title: "Hello",
          content: Promise.resolve("Async World"),
        }));
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        expect(await res.json()).toEqual({ title: "Hello", content: "Async World" });
      });

      test("handler with all promise properties resolves for JSON response", async () => {
        const route = makeHandler(() => ({
          title: Promise.resolve("Async Hello"),
          content: Promise.resolve("Async World"),
        }));
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        expect(await res.json()).toEqual({ title: "Async Hello", content: "Async World" });
      });

      test("async handler returning promise properties resolves for JSON response", async () => {
        const route = makeHandler(async () => ({
          title: "Hello",
          content: Promise.resolve("Async World"),
        }));
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        expect(await res.json()).toEqual({ title: "Hello", content: "Async World" });
      });

      test("formatOutput receives fully resolved data even when handler returned promises", async () => {
        let receivedData!: z.infer<typeof outputSchema>;
        const route = makeHandler(
          () => ({
            title: "Hello",
            content: Promise.resolve("Async World"),
          }),
          {
            formatOutput: async (data, _user, _req, _params) => {
              receivedData = data;
              return {
                data: JSON.stringify(data),
                headers: new Headers({ "Content-Type": "application/json" }),
              };
            },
          },
        );
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        // formatOutput should have received plain resolved values, not promises
        expect(receivedData.title).toBe("Hello");
        expect(receivedData.content).toBe("Async World");
        expect(receivedData.content).not.toBeInstanceOf(Promise);
        expect(await res.json()).toEqual({ title: "Hello", content: "Async World" });
      });

      test("formatOutput with sync data works normally — data properties are plain values", async () => {
        const route = makeHandler(() => ({ title: "Hello", content: "World" }), {
          formatOutput: (data, _user, _req, _params) => ({
            // data.title and data.content are string — no await needed
            data: `<h1>${data.title}</h1><p>${data.content}</p>`,
            headers: new Headers({ "Content-Type": "text/html" }),
          }),
        });
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        expect(await res.text()).toBe("<h1>Hello</h1><p>World</p>");
      });

      test("formatStreamOutput receives raw promises when handler returns promise properties", async () => {
        let receivedData!: { title: string; content: Promise<string> };
        const route = makeHandler(
          () => ({
            title: "Hello",
            content: Promise.resolve("Async World"),
          }),
          {
            formatStreamOutput: async (data, _user, _req, _params) => {
              receivedData = data;
              // TypeScript knows data.title is `string` — no await needed
              // TypeScript knows data.content is `Promise<string>` — must await
              return {
                data: JSON.stringify({ title: data.title, content: await data.content }),
                headers: new Headers({ "Content-Type": "application/json" }),
              };
            },
          },
        );
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        // formatStreamOutput received the exact types — title is a plain string, content is a Promise
        expect(receivedData.title).toBe("Hello");
        expect(receivedData.content).toBeInstanceOf(Promise);
        expect(await res.json()).toEqual({ title: "Hello", content: "Async World" });
      });

      test("formatStreamOutput knows exact types — plain string properties need no await", async () => {
        const route = makeHandler(
          () => ({
            title: "Hello",
            content: Promise.resolve("Async World"),
          }),
          {
            formatStreamOutput: (data) => {
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
          },
        );
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(await res.text()).toBe("HELLOAsync World");
      });

      test("formatStreamOutput can produce a ReadableStream for streaming SSR", async () => {
        const route = makeHandler(
          () => ({
            title: "Hello",
            content: new Promise<string>((resolve) => setTimeout(() => resolve("Streamed!"), 10)),
          }),
          {
            formatStreamOutput: (data, _user, _req, _params) => {
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
          },
        );
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        expect(await res.text()).toBe("<h1>Hello</h1><p>Streamed!</p>");
      });

      test("formatStreamOutput: per-property validation fires outputErrorWarning for invalid async values", async () => {
        const warnings: any[] = [];
        const route = makeHandler(
          () => ({
            title: "Valid",
            content: Promise.resolve(12345 as unknown as string), // intentionally wrong type to test validation
          }),
          {
            outputErrorWarning: (error, data, method, url) => {
              warnings.push({ error, data, method, url });
            },
            formatStreamOutput: async (data, _user, _req, _params) => ({
              data: JSON.stringify({ title: data.title, content: await data.content }),
              headers: new Headers({ "Content-Type": "application/json" }),
            }),
          },
        );
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        // Wait a tick for the background per-property validation to fire
        await new Promise((r) => setTimeout(r, 10));
        expect(warnings.length).toBeGreaterThan(0);
        expect(warnings.some((w) => w.url === "content")).toBe(true);
      });

      test("JSON path validates resolved values and calls outputErrorWarning on mismatch", async () => {
        const warnings: any[] = [];
        const route = makeHandler(
          () => ({
            title: "Valid",
            content: Promise.resolve(999 as unknown as string), // intentionally wrong type to test validation
          }),
          {
            outputErrorWarning: (error, data, method, url) => {
              warnings.push({ error, data, method, url });
            },
          },
        );
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        expect(res.status).toBe(200);
        // Full-object validation fires on the resolved result
        expect(warnings.length).toBeGreaterThan(0);
      });

      test("JSON path strips extra properties after validation when all sync", async () => {
        const route = makeHandler(() => ({
          title: "Hello",
          content: "World",
          extra: "should be stripped",
        }));
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        const json = await res.json();
        expect(json).toEqual({ title: "Hello", content: "World" });
        expect(json.extra).toBeUndefined();
      });

      test("JSON path strips extra properties after validation when promises resolved", async () => {
        const route = makeHandler(() => ({
          title: Promise.resolve("Hello"),
          content: Promise.resolve("World"),
          extra: Promise.resolve("should be stripped"),
        }));
        const res = await route.handlerWrapped(new Request("https://example.com/1"));
        const json = await res.json();
        expect(json).toEqual({ title: "Hello", content: "World" });
        expect(json.extra).toBeUndefined();
      });

      describe("primitive output schemas", () => {
        test("z.string() output — sync handler returns string as JSON", async () => {
          const handle = RouteHandlerDefiner(async () => "ok", async () => ({}));
          const route = handle(def.get("/:id", "", z.string()), async () => "hello");
          const res = await route.handlerWrapped(new Request("https://example.com/1"));
          expect(res.status).toBe(200);
          expect(await res.json()).toBe("hello");
        });

        test("z.string() output — async handler returning Promise<string>", async () => {
          const handle = RouteHandlerDefiner(async () => "ok", async () => ({}));
          const route = handle(def.get("/:id", "", z.string()), async () => Promise.resolve("hello async"));
          const res = await route.handlerWrapped(new Request("https://example.com/1"));
          expect(res.status).toBe(200);
          expect(await res.json()).toBe("hello async");
        });

        test("z.number() output — sync handler returns number as JSON", async () => {
          const handle = RouteHandlerDefiner(async () => "ok", async () => ({}));
          const route = handle(def.get("/:id", "", z.number()), async () => 42);
          const res = await route.handlerWrapped(new Request("https://example.com/1"));
          expect(res.status).toBe(200);
          expect(await res.json()).toBe(42);
        });

        test("z.string() output — formatOutput receives the plain string, not a mapped object", async () => {
          const handle = RouteHandlerDefiner(async () => "ok", async () => ({}));
          let receivedData: unknown;
          const route = handle(
            def.get("/:id", "", z.string()),
            async () => "hello",
            async (data) => {
              receivedData = data;
              return {
                data: `<p>${data}</p>`,
                headers: new Headers({ "Content-Type": "text/html" }),
              };
            },
          );
          const res = await route.handlerWrapped(new Request("https://example.com/1"));
          expect(res.status).toBe(200);
          expect(await res.text()).toBe("<p>hello</p>");
          expect(receivedData).toBe("hello");
          expect(typeof receivedData).toBe("string");
        });

        test("z.boolean() output — sync handler", async () => {
          const handle = RouteHandlerDefiner(async () => "ok", async () => ({}));
          const route = handle(def.get("/:id", "", z.boolean()), async () => true);
          const res = await route.handlerWrapped(new Request("https://example.com/1"));
          expect(res.status).toBe(200);
          expect(await res.json()).toBe(true);
        });
      });
    });
  });
});
