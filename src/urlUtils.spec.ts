import { describe, expect, test } from "bun:test";
import {
  defToUrl,
  extractMandatoryParamNames,
  extractOptionalParamNames,
  extractParamNames,
  getIdNames,
  getNonIdNames,
} from "./urlUtils.ts";
import { z } from "zod";

describe("decompose url string", () => {
  test("path to manadtory keys", () => {
    expect(extractParamNames("/hey/")).toStrictEqual([]);
    expect(extractParamNames("/hey/:you")).toStrictEqual(["you"]);
    expect(extractParamNames("/hey/:opt?")).toStrictEqual(["opt"]);
    expect(extractParamNames("/hey/:you/:opt?")).toStrictEqual(["you", "opt"]);
    expect(extractParamNames("/hey/:you/:opt?/:opt2?/")).toStrictEqual(["you", "opt", "opt2"]);
  });

  test("path to optionalKeys", () => {
    expect(extractOptionalParamNames("/hey/")).toStrictEqual([]);
    expect(extractOptionalParamNames("/hey/:you")).toStrictEqual([]);
    expect(extractOptionalParamNames("/hey/:opt?")).toStrictEqual(["opt"]);
    expect(extractOptionalParamNames("/hey/:you/:opt?")).toStrictEqual(["opt"]);
  });

  test("path to mandatory keys", () => {
    expect(extractMandatoryParamNames("/hey/")).toStrictEqual([]);
    expect(extractMandatoryParamNames("/hey/:you")).toStrictEqual(["you"]);
    expect(extractMandatoryParamNames("/hey/:opt?")).toStrictEqual([]);
    expect(extractMandatoryParamNames("/hey/:you/:opt?")).toStrictEqual(["you"]);
    expect(extractMandatoryParamNames("/hey/:you/:opt?/:opt2?/")).toStrictEqual(["you"]);
    expect(extractMandatoryParamNames("/hey/:you/:opt/:opt2?/")).toStrictEqual(["you", "opt"]);
  });

  test("get id containing keys", () => {
    expect(getIdNames(["hello"] as const)).toStrictEqual([]);
    expect(getIdNames(["hello", "guest_id"] as const)).toStrictEqual(["guest_id"]);
    expect(getIdNames(["hello", "guest_id", "item_id"] as const)).toStrictEqual([
      "guest_id",
      "item_id",
    ]);
    expect(getNonIdNames(["hello"] as const)).toStrictEqual(["hello"]);
    expect(getNonIdNames(["hello", "guest_id"] as const)).toStrictEqual(["hello"]);
    expect(getNonIdNames(["hello", "guest_id", "item"] as const)).toStrictEqual(["hello", "item"]);
  });
});

describe("defToGetUrl", () => {
  test("simple use case", () => {
    const def = {
      method: "get",
      path: "/testpath",
      paramsValidation: z.object({}),
      outputValidation: z.unknown(),
      permissionsNeeded: "",
    } as const;
    expect(defToUrl(def, {})).toBe("/testpath");
    //Extra keys are stripped
    //@ts-expect-error
    expect(defToUrl(def, { extrakey: "isstripped" })).toBe("/testpath");

    expect(
      defToUrl(
        {
          ...def,
          paramsValidation: z.object({ hellyeah: z.number() }),
        },
        { hellyeah: 23 },
      ),
    ).toBe("/testpath?hellyeah=23");

    expect(
      defToUrl(
        {
          ...def,
          paramsValidation: z.object({ hellyeah: z.number(), aSecondString: z.string() }),
        },
        { hellyeah: 23, aSecondString: "HELLOYOU" },
      ),
    ).toBe("/testpath?hellyeah=23&aSecondString=HELLOYOU");
  });
  test("zod or", () => {
    const def = {
      method: "get",
      path: "/testpath",
      paramsValidation: z
        .object({
          name: z.string().min(2).max(10),
        })
        .or(z.object({ value: z.string().min(2).max(10) })),
      outputValidation: z.unknown(),
      permissionsNeeded: "",
    } as const;
    expect(defToUrl(def, { name: "John" })).toBe("/testpath?name=John");
    expect(defToUrl(def, { value: "John" })).toBe("/testpath?value=John");
  });

  describe("complex objects", () => {
    test("nested objects", () => {
      const def = {
        method: "get",
        path: "/testpath",
        paramsValidation: z.object({
          nested: z.object({
            name: z.string().min(2).max(10),
          }),
        }),
        outputValidation: z.unknown(),
        permissionsNeeded: "",
      } as const;
      expect(defToUrl(def, { nested: { name: "John" } })).toBe("/testpath?nested.name=John");
      expect(
        defToUrl(
          {
            ...def,
            paramsValidation: z.object({
              nested: z.object({
                name: z.string().min(2).max(10),
                age: z.number().min(0).max(100),
                again: z.object({
                  name: z.string().min(2).max(10),
                }),
              }),
            }),
          },
          { nested: { name: "John", age: 23, again: { name: "Jane" } } },
        ),
      ).toBe("/testpath?nested.name=John&nested.age=23&nested.again.name=Jane");
    });

    //test array
    test("array", () => {
      const def = {
        method: "get",
        path: "/testpath",
        paramsValidation: z.object({
          array: z.array(z.string().min(2).max(10)),
        }),
        outputValidation: z.unknown(),
        permissionsNeeded: "",
      } as const;
      expect(defToUrl(def, { array: ["John", "Jane"] })).toBe(
        "/testpath?array.0=John&array.1=Jane",
      );
    });
    //test nested array, object under array
    test("nested array", () => {
      const def = {
        method: "get",
        path: "/testpath",
        paramsValidation: z.object({
          array: z.array(z.string().min(2).max(10)),
          nested: z.object({
            array: z.array(z.string().min(2).max(10)),
          }),
        }),
        outputValidation: z.unknown(),
        permissionsNeeded: "",
      } as const;
      expect(defToUrl(def, { array: ["John", "Jane"], nested: { array: ["John", "Jane"] } })).toBe(
        "/testpath?array.0=John&array.1=Jane&nested.array.0=John&nested.array.1=Jane",
      );
    });
    //test nested array, object under array, object under object
    test("nested array", () => {
      const def = {
        method: "get",
        path: "/testpath",
        paramsValidation: z.object({
          nested: z.object({
            array: z.array(
              z.object({
                name: z.string().min(2).max(10),
                age: z.number().min(0).max(120),
              }),
            ),
          }),
        }),
        outputValidation: z.unknown(),
        permissionsNeeded: "",
      } as const;
      expect(
        defToUrl(def, {
          nested: {
            array: [
              { name: "John", age: 30 },
              { name: "Jane", age: 25 },
            ],
          },
        }),
      ).toBe(
        "/testpath?nested.array.0.name=John&nested.array.0.age=30&nested.array.1.name=Jane&nested.array.1.age=25",
      );
    });
  });
  test("with URL params and search params", () => {
    const def = {
      method: "get",
      path: "/testpath/:pathnum",
      paramsValidation: z.object({
        pathnum: z.number(),
        otherValue: z.string().min(2).max(10),
      }),
      outputValidation: z.unknown(),
      permissionsNeeded: "",
    } as const;
    expect(
      defToUrl(def, {
        pathnum: 1,
        otherValue: "test",
      }),
    ).toBe("/testpath/1?otherValue=test");
  });

  test("with optional URL params and search params", () => {
    const def = {
      method: "get",
      path: "/testpath/:pathnum/:optstr?",
      paramsValidation: z.object({
        pathnum: z.number(),
        optstr: z.string().optional(),
        otherValue: z.string().min(2).max(10).optional(),
      }),
      outputValidation: z.unknown(),
      permissionsNeeded: "",
    } as const;
    expect(
      defToUrl(def, {
        pathnum: 2,
        otherValue: "test",
      }),
    ).toBe("/testpath/2?otherValue=test");

    expect(
      defToUrl(def, {
        pathnum: 3,
        optstr: "hello",
        otherValue: "test",
      }),
    ).toBe("/testpath/3/hello?otherValue=test");
  });

  test("with url params and trailing slash", () => {
    const def = {
      method: "get",
      path: "/testpath/:pathnum/",
      paramsValidation: z.object({
        pathnum: z.number(),
        optstr: z.string().optional(),
        otherValue: z.string().min(2).max(10).optional(),
      }),
      outputValidation: z.unknown(),
      permissionsNeeded: "",
    } as const;
    expect(
      defToUrl(def, {
        pathnum: 4,
        otherValue: "test",
      }),
    ).toBe("/testpath/4?otherValue=test");
  });

  test("with url params and trailing slash and optional params", () => {
    const def = {
      method: "get",
      path: "/testpath/:pathnum/:optstr?/:optnum?/",
      paramsValidation: z.object({
        pathnum: z.number(),
        optstr: z.string().optional(),
        optnum: z.number().optional(),
        otherValue: z.string().min(2).max(10).optional(),
      }),
      outputValidation: z.unknown(),
      permissionsNeeded: "",
    } as const;
    expect(
      defToUrl(def, {
        pathnum: 5,
        optstr: "hello",
        optnum: 99,
        otherValue: "test",
      }),
    ).toBe("/testpath/5/hello/99?otherValue=test");

    expect(
      defToUrl(def, {
        pathnum: 6,
        otherValue: "test",
      }),
    ).toBe("/testpath/6?otherValue=test");

    expect(
      defToUrl(def, {
        pathnum: 6,
        //This will not be in the url, since preceeding optionals are missing
        optnum: 100,
        otherValue: "test",
      }),
    ).toBe("/testpath/6?otherValue=test");
  });

  test("funky characters in params", () => {
    const def = {
      method: "get",
      path: "/testpath/:pathnum/:optstr?/:optnum?/",
      paramsValidation: z.object({
        pathnum: z.number(),
        optstr: z.string().optional(),
        optnum: z.number().optional(),
        otherValue: z.string().min(2).max(10).optional(),
      }),
      outputValidation: z.unknown(),
      permissionsNeeded: "",
    } as const;
    expect(
      defToUrl(def, {
        pathnum: 7,
        optstr: "don't",
        otherValue: "t;'`%!est",
      }),
    ).toBe("/testpath/7/don%27t?otherValue=t%3B%27%60%25%21est");

    expect(
      defToUrl(def, {
        pathnum: 8000000000001,
        otherValue: "test",
      }),
    ).toBe("/testpath/8000000000001?otherValue=test");
  });

  //test serialization of object keys and values that contain characters that are not valid in a query string
  test("serialization of object keys and values that contain characters that are not valid in a query string", () => {
    const def = {
      method: "get",
      path: "/testpath",
      paramsValidation: z.object({
        "spaced out": z.object({
          "star+><=&array": z.array(
            z.object({
              name: z.string().min(2).max(10),
              age: z.number().min(0).max(120),
            }),
          ),
        }),
      }),
      outputValidation: z.unknown(),
      permissionsNeeded: "",
    } as const;
    expect(
      defToUrl(def, {
        "spaced out": {
          "star+><=&array": [
            { name: "Joh`n", age: 30 },
            { name: "Jane", age: 25 },
          ],
        },
      }),
    ).toBe(
      "/testpath?spaced%252520out.star%25252B%25253E%25253C%25253D%252526array.0.name=Joh%60n&spaced%252520out.star%25252B%25253E%25253C%25253D%252526array.0.age=30&spaced%252520out.star%25252B%25253E%25253C%25253D%252526array.1.name=Jane&spaced%252520out.star%25252B%25253E%25253C%25253D%252526array.1.age=25",
    );
  });
});
