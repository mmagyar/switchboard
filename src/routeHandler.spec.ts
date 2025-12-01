import { describe, test, expect } from "bun:test";
import { RouteHandlerDefiner } from "./routeHandler.ts";
import { define } from "./routeDef.ts";
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
          name: options.optionalQuery
            ? z.string().min(2).max(100).optional()
            : z.string().min(2).max(100),
          "dotty.potty": z.string().min(2).max(100).optional(),
          "obj.different": z.number().optional(),
          obj: z
            .object({
              booboo: z.boolean().optional(),
              key: z.string(),
              num: options.addNumber
                ? z.number().min(0).max(100)
                : z.number().min(0).max(100).optional(),
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
      const req = new Request(
        "https://example.com/1/?name=Joe&obj.key=value&obj.value.name=deep&obj.value.age=35",
      );
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

      const invalidReq = new Request(
        "https://example.com/1/?dotty.potty=helloYou&obj.different=32",
      );
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
  });
});
