import { merge, filter, keys } from "./util.ts";
import { z, ZodArray, ZodBoolean, ZodNumber, ZodObject, ZodOptional, ZodReadonly, ZodType } from "zod";
//Unwrap all types that my wrap a value
type ZodUnwrap<T> = T extends ZodReadonly<infer K> ? ZodUnwrap<K> : T extends ZodOptional<infer K> ? ZodUnwrap<K> : T;
// : T extends ZodEffects<infer K>
// ? ZodUnwrap<K>
// : T;
export const zodUnwrap = <T>(schema: T): ZodUnwrap<T> => {
  // TODO as any is invalid but still need to fix it.
  if (schema instanceof ZodReadonly) return zodUnwrap(schema.def.innerType) as any;
  // if (schema instanceof ZodEffects) return zodUnwrap((schema as any)._def);
  if (schema instanceof ZodOptional) return zodUnwrap(schema.def.innerType) as any;
  return schema as ZodUnwrap<T>;
};
const isNestedNumber = (value: ZodType) => zodUnwrap(value) instanceof ZodNumber;

const isNestedBoolean = (value: ZodType) => zodUnwrap(value) instanceof ZodBoolean;
const isNestedObject = (value: ZodType) => zodUnwrap(value) instanceof ZodObject;
const isNestedArray = (value: ZodType) => zodUnwrap(value) instanceof ZodArray;

const callForSubObjects = <Schema extends z.ZodType, T extends Record<Keys, V>, V, Keys extends keyof T = keyof T>(
  schemaIn: Schema,
  input: T,
  callback: (schema: z.ZodType, obj: object) => Record<string, any> | undefined,
): Record<string, V> | undefined => {
  let out: Record<string, any> | undefined;
  if (!("shape" in schemaIn)) return out;

  const shape = schemaIn.shape as Record<string, z.ZodType>;
  const objectValidations = keys(filter(shape, (value) => isNestedObject(value)));
  for (const key of objectValidations) {
    const incoming = input[key as Keys];
    const part = zodUnwrap(shape[key]);
    if (part && part instanceof ZodObject && incoming && typeof incoming === "object") {
      const res = callback(part, incoming);
      if (!out) out = {};
      out[key] = typeof out[key] === "object" && res ? merge(out[key], res) : res;
    }
  }
  const arrayValidations = keys(filter(shape, (value) => isNestedArray(value)));
  for (const key of arrayValidations) {
    const incoming = input[key as Keys];
    const part = zodUnwrap(shape[key]);
    if (part && part instanceof ZodArray && incoming && Array.isArray(incoming)) {
      const res = callback(part, incoming);
      if (res) {
        if (!Array.isArray(res)) throw new Error(`Expected array but got ${typeof res}`);
        if (!out) out = {};
        out[key] = res;
      }
    }
  }
  return out;
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
export const parseNumberFromForm = <
  Schema extends z.ZodType,
  T extends Record<Keys, V>,
  V,
  Keys extends keyof T = keyof T,
>(
  schemaIn: Schema,
  input: T,
): Record<string, V | number | number[]> | number[] | undefined => {
  let convertedValues: Record<string, V | number | number[]> | undefined = undefined;
  const schema = zodUnwrap(schemaIn);

  if (schema instanceof ZodArray) {
    const arrayItems = zodUnwrap(schema.def.element);
    if (!Array.isArray(input)) {
      return undefined;
    }
    if (arrayItems instanceof ZodNumber) {
      const values = input.map(parseNumber).filter((value) => value !== undefined);
      if (values.length === 0) {
        return undefined;
      }

      return values;
    } else if (arrayItems instanceof ZodObject || arrayItems instanceof ZodArray) {
      //TODO cast to any on return, the actual return type is not the best, deal with it later, does not really matter
      const values = input
        .map((item) => parseNumberFromForm(arrayItems, item))
        .filter((value) => value !== undefined) as any;
      if (values.length === 0) {
        return undefined;
      }
      return values;
    }
    return [];
  }

  if (schema instanceof ZodType && "shape" in schema) {
    const shape = schema.shape as Record<string, z.ZodType>;
    const numberValidations = keys(filter(shape, (value) => isNestedNumber(value)));

    for (const key of numberValidations) {
      const currentValue = input[key as Keys];
      const value = typeof currentValue === "string" ? parseNumber(currentValue) : undefined;
      if (value) {
        if (!convertedValues) convertedValues = {};
        convertedValues[key] = value;
      }
    }
    const subObject = callForSubObjects(schema, input, parseNumberFromForm) as Record<string, any>;
    return convertedValues ? (subObject ? merge(convertedValues, subObject) : convertedValues) : subObject;
  }
  return convertedValues;
};

const parseBoolean = (value: string | undefined): boolean | undefined => {
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
export const parseBooleanFromForm = <
  Schema extends z.ZodType,
  T extends Record<Keys, any>,
  Keys extends keyof T = keyof T,
>(
  schemaIn: Schema,
  input: T,
): Record<string, any> | boolean[] | undefined => {
  const convertedValues: Record<string, boolean | object> = {};
  const schema = zodUnwrap(schemaIn);

  if (schema instanceof ZodArray) {
    const arrayItems = schema.def.element; //zodUnwrap(schema._def.type);
    if (!Array.isArray(input)) {
      return [];
    }
    if (arrayItems instanceof ZodBoolean) {
      return input.map(parseBoolean).filter((value) => value !== undefined);
    } else if (arrayItems instanceof ZodObject || arrayItems instanceof ZodArray) {
      //TODO cast to any on return, the actual return type is not the best, deal with it later, does not really matter
      return input.map((item) => parseBooleanFromForm(arrayItems, item)) as any;
    }
    return [];
  }

  //TODO this is probably not valid

  if ("def" in schema && "shape" in schema.def) {
    const shape = schema.def.shape as Record<string, z.ZodType>;
    const booleanValidations = keys(filter(shape, (value) => isNestedBoolean(value)));

    for (const key of booleanValidations) {
      const incoming = parseBoolean(input[key as Keys]);
      if (typeof incoming === "boolean") convertedValues[key] = incoming;
    }

    const subValues = callForSubObjects(schema, input, parseBooleanFromForm);
    return convertedValues ? (subValues ? merge(convertedValues, subValues) : convertedValues) : subValues;
  }
  return convertedValues;
};

export const parseObjectFromForm = <
  Schema extends z.ZodType,
  T extends Record<Keys, any>,
  Keys extends keyof T = keyof T,
>(
  schemaIn: Schema,
  input: T,
): Record<string, object | number | string> => {
  let results: Record<string, object> = {};
  const schema = zodUnwrap(schemaIn);
  const normalResults: Record<string, any> = {};

  if ("shape" in schema) {
    const shape = schema.shape as Record<string, z.ZodType>;
    const uw = zodUnwrap(shape);
    const deepKeys = keys(filter(shape, (value) => isNestedObject(value) || isNestedArray(value)));
    const normalKeys = keys(filter(shape, (value) => !isNestedObject(value) && !isNestedArray(value)));

    const processKey = (key: string) => {
      const matching = keys(input).filter((keyInput) => String(keyInput).startsWith(key + "."));
      const reduced = matching.reduce(
        (p, c) => {
          const parts = String(c).split(".");
          let nested = p as Record<string, any>;
          let nextValidation: z.ZodType | undefined;
          for (let i = 0; i < parts.length; i++) {
            const part = parts[i]!;
            if (nextValidation instanceof ZodArray) {
              nextValidation = zodUnwrap(nextValidation?.def?.element) as ZodType; //TODO validate cast
            } else if (nextValidation && "shape" in nextValidation) {
              const nextPart = (nextValidation as ZodObject).shape[part];
              if (!nextPart) {
                nextValidation = z.any();
                console.warn(`Missing validation for ${part}`);
              }
              nextValidation = zodUnwrap(nextPart);
            } else {
              nextValidation = zodUnwrap(uw[part])!;
            }

            if (!nested[part]) nested[part] = nextValidation instanceof ZodArray ? [] : {};

            if (parts.length - 1 === i) nested[part] = input[c];
            else nested = nested[part];
          }
          return p;
        },
        {} as Record<string, any>,
      );

      results = merge(results, reduced);
    };

    for (const key of normalKeys) {
      normalResults[key] = (input as any)[key as any] as any;
    }
    for (const key of deepKeys) {
      processKey(key);
    }
  }
  const mregeResult = merge(normalResults, results);
  return mregeResult;
};
