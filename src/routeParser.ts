import { merge } from "./util.ts";
import { z, ZodArray, ZodBoolean, ZodNumber, ZodObject, ZodOptional, ZodReadonly, ZodType, ZodUnion } from "zod";

// TODO clean up the types in this file, remove the casts

type ZodUnwrap<T> = T extends ZodReadonly<infer K> ? ZodUnwrap<K> : T extends ZodOptional<infer K> ? ZodUnwrap<K> : T;

export const zodUnwrap = <T>(schema: T): ZodUnwrap<T> => {
  if (schema instanceof ZodReadonly) return zodUnwrap(schema.def.innerType) as any;
  if (schema instanceof ZodOptional) return zodUnwrap(schema.def.innerType) as any;
  return schema as ZodUnwrap<T>;
};

const callForSubObjects = <Schema extends z.ZodType>(
  schemaIn: Schema,
  input: unknown,
  callback: (schema: z.ZodType, obj: object) => Record<string, any> | undefined | unknown,
): Record<string, unknown> | undefined => {
  let out: Record<string, any> | undefined;
  if (schemaIn instanceof ZodUnion) {
    return unionHandler(schemaIn, input, (t, i) => callForSubObjects(t, i, callback));
  }
  if (!(schemaIn instanceof ZodObject)) return undefined;
  if (!input || typeof input !== "object") return undefined;

  const shape = schemaIn.shape;

  for (const key of Object.keys(input)) {
    const incoming = (input as Record<string, unknown>)[key];
    const part = zodUnwrap(shape[key]);

    if (part instanceof ZodObject && incoming && typeof incoming === "object") {
      const res = callback(part, incoming);
      if (!res) continue;
      if (!out) out = {};
      out[key] = typeof out[key] === "object" && res ? merge(out[key], res) : res;
    }

    if (part instanceof ZodUnion && incoming) {
      const res = callback(part, incoming);
      if (!res) continue;
      if (!out) out = {};
      out[key] = typeof out[key] === "object" && res ? merge(out[key], res) : res;
    }

    if (part instanceof ZodArray && incoming && Array.isArray(incoming)) {
      const res = callback(part, incoming);
      if (!res) continue;
      if (!Array.isArray(res)) throw new Error(`Expected array but got ${typeof res}`);
      if (!out) out = {};
      out[key] = res;
    }
  }

  return out;
};

const unionHandler = <RETURN>(
  schema: ZodUnion,
  input: unknown,
  parser: (z: ZodType, input: unknown) => RETURN,
): RETURN => {
  //This function is kinda fine,
  // but due to the design there is an edge case
  // when there is a union of a string and something that the parser can parse, and both would be valid, it will parse it, possibly making it it invalid.
  // TODO make a test case to demondtrate that
  return schema.options
    .map((x) => parser(x as z.ZodType, input))
    .filter((x) => x !== undefined)
    .reduce((p: any, c: RETURN) => {
      if (c) {
        if (typeof c === "object" && p && typeof p === "object") {
          return merge(p, c, "nonEmpty", "nonEmpty");
        }
        return c;
      }
      return p;
    }, undefined);
};

const arrayHandler = <RETURN>(
  schema: ZodArray,
  input: unknown,
  parser: (z: ZodType, input: unknown) => RETURN,
): RETURN[] | undefined => {
  if (!Array.isArray(input)) return;

  const arrayItems = zodUnwrap(schema.def.element);
  const result = input.map((item) => parser(arrayItems as ZodType, item)).filter((x) => x !== undefined);
  if (result.length === 0) return;

  return result;
};

const objectHandler = <RETURN>(
  schema: ZodObject,
  input: unknown,
  parser: (z: ZodType, input: unknown) => RETURN,
): RETURN | undefined => {
  let convertedValues: Record<string, RETURN | object> | undefined = undefined;
  const shape = schema.shape;

  for (const key of Object.keys(shape)) {
    if (!(shape[key] instanceof z.ZodObject) && input && typeof input === "object") {
      const output = parser(shape[key], (input as Record<string, any>)[key]);
      if (typeof output !== "undefined") {
        if (!convertedValues) convertedValues = {};
        convertedValues[key] = output;
      }
    }
  }

  const subObject = callForSubObjects(schema, input, parser);
  return (convertedValues ? (subObject ? merge(convertedValues, subObject) : convertedValues) : subObject) as RETURN;
};

const parseNumber = (value: string) => {
  if (typeof value !== "string") return;
  const cast = Number(value);
  const isNumber = !(isNaN(cast) || value.length === 0);
  if (isNumber) {
    return cast;
  }
  return;
};

/**
 * Convert string params to numbers, if schema requires a number
 * This avoids having to modify the schema to use corece in zod number
 * And that is a good thing because that is a bit too liberal with coercing, eg "" == 0
 *
 * This function does not run validation of any kind
 */
export const parseNumberFromForm = (
  schemaIn: z.ZodType,
  input: unknown,
): Record<string, any> | number[] | number | undefined => {
  if (typeof input === "undefined") return undefined;
  const schema = zodUnwrap(schemaIn);

  if (schema instanceof ZodUnion) {
    return unionHandler(schema, input, parseNumberFromForm);
  }

  if (typeof input === "string") {
    if (schema instanceof ZodNumber) return parseNumber(input);
    return undefined;
  }

  if (schema instanceof ZodArray) {
    return arrayHandler(schema, input, parseNumberFromForm);
  }

  if (schema instanceof ZodObject) {
    return objectHandler(schema, input, parseNumberFromForm);
  }

  return undefined;
};

const parseBoolean = (value: string | undefined | unknown): boolean | undefined => {
  if (value === "true") return true;
  if (value === "false") return false;
  return undefined;
};

/*
 * Convert string params to booleans, if schema requires a boolean
 * It only converts if a straight conversion is possible:
 * if the string is "true" or "false"
 * all other values will not be converted.
 * This function does NOT run validation of any kind
 */
export const parseBooleanFromForm = (
  schemaIn: z.ZodType,
  input: unknown,
): Record<string, any> | boolean[] | boolean | undefined => {
  const schema = zodUnwrap(schemaIn);

  if (schema instanceof ZodUnion) {
    return unionHandler(schema, input, parseBooleanFromForm);
  }

  if (typeof input === "string") {
    if (schema instanceof ZodBoolean) {
      return parseBoolean(input);
    }
    return undefined;
  }

  if (schema instanceof ZodArray) {
    return arrayHandler(schema, input, parseBooleanFromForm);
  }

  if (schema instanceof ZodObject) {
    return objectHandler(schema, input, parseBooleanFromForm);
  }
  return undefined;
};

export const parseObjectFromForm = (
  input: Record<string, string | string[]>,
): Record<string, object | number | string> => {
  let result = {};

  for (const inputKey of Object.keys(input)) {
    result = merge(result, parseFieldName(inputKey, input[inputKey]));
  }
  return result;
};

export const parseFieldName = (fieldName: string, fieldValue?: string | string[]): object => {
  const parts = fieldName.split(".");
  const result: any = {};
  let current = result;
  for (let i = 0; i < parts.length; i++) {
    current[parts[i]!] = isNumeric(parts[i + 1]) ? [] : {};

    if (!parts[i + 1]) {
      current[parts[i]!] = fieldValue;
      return result;
    }
    current = current[parts[i]!];
  }

  return result;
};

function isNumeric(str?: string): boolean {
  if (!str) return false;
  for (let i = 0; i < str.length; i++) {
    const code = str.charCodeAt(i);
    // Checks if ascii code is below 48 ('0') or above 57 ('9')
    if (code < 48 || code > 57) {
      return false;
    }
  }
  return true;
}
