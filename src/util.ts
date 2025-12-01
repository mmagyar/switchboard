type ExcludeUndefined<T> = T extends undefined ? never : T;
export const forEach = <T extends Partial<Record<Keys, any>>, Keys extends keyof T = keyof T>(
  obj: T,
  callback: (value: T[keyof T], key: Keys, index: number) => void,
): void => {
  let i = 0;
  for (const key of Object.keys(obj)) {
    callback(obj[key as Keys], key as Keys, i);
    i += 1;
  }
};

export const forEachFilterUndefined = <
  T extends Partial<Record<Keys, any>>,
  Keys extends keyof T = keyof T,
>(
  obj: T,
  callback: (value: Exclude<T[keyof T], undefined>, key: Keys, index: number) => void,
): void => {
  let i = 0;
  for (const key of Object.keys(obj)) {
    const value = obj[key as Keys];
    if (value !== undefined) {
      callback(value as Exclude<T[keyof T], undefined>, key as Keys, i);
      i += 1;
    }
  }
};

export const count = <T>(obj: { [s: string]: T }): number => Object.keys(obj).length;

export const map = <
  T extends Record<Keys, any>,
  U,
  Keys extends keyof T = keyof T,
  FILTER extends boolean = false,
>(
  obj: T,
  fn: (value: T[keyof T], key: keyof T) => U,
  filterUndefinedOutput: FILTER = false as FILTER,
): FILTER extends false ? Record<Keys, U> : Partial<Record<Keys, ExcludeUndefined<U>>> => {
  const newObj = {} as Record<Keys, U>;
  for (const key in obj) {
    const newValue = fn(obj[key], key);
    if (!filterUndefinedOutput || newValue !== undefined) {
      newObj[key as unknown as Keys] = newValue;
    }
  }
  return newObj as FILTER extends false
    ? Record<Keys, U>
    : Partial<Record<Keys, ExcludeUndefined<U>>>;
};

export const filter = <T extends Record<Keys, any>, Keys extends keyof T = keyof T>(
  obj: T,
  fn: (value: T[keyof T], key: keyof T) => boolean,
): Partial<Record<Keys, T[keyof T]>> => {
  const newObj = {} as Record<Keys, T[keyof T]>;
  for (const key in obj) {
    if (fn(obj[key], key)) {
      newObj[key as unknown as Keys] = obj[key];
    }
  }
  return newObj;
};

export const values = <T extends Record<Keys, any>, Keys extends keyof T = keyof T>(obj: T) =>
  Object.values(obj) as T[keyof T][];

export const keys = <T extends Record<Keys, any>, Keys extends keyof T = keyof T>(obj: T) =>
  Object.keys(obj) as Keys[];

// export const entries = <T extends Record<Keys, any>, Keys extends keyof T = keyof T>(obj: T) =>
//   Object.entries(obj) as [Keys, T[Keys]][];
export const entriesWithUndefiend = <T extends object>(obj: T) => {
  return Object.entries(obj) as [keyof T, T[keyof T]][];
};

/**
 * Returns all entries typed correctly, without undefined values
 **/
export const entries = <T extends object>(obj: T) => {
  return Object.entries(obj).filter(([, value]) => value !== undefined) as [
    keyof T,
    Exclude<T[keyof T], undefined>,
  ][];
  // .map(([key, value]) => [key as keyof T, value as Exclude<T[keyof T], undefined>]) as [
  // keyof T,
  // Exclude<T[keyof T], undefined>,
  // ][];
};
export function toObject<T, K extends number | string | symbol>(
  array: T[],
  keyTransform: (input: T) => K = (input) => (input as { id: K })["id"],
): Record<K, T> {
  const response: Record<K, T> = {} as Record<K, T>;
  for (const item of array) {
    response[keyTransform(item)] = item;
  }

  return response;
}

export const objectToArray = <T extends Record<any, any>, OUT, FILTER extends boolean = true>(
  obj: T,
  callback: (value: T[keyof T], key: keyof T, index: number) => OUT,
  filterUndefinedOutput: FILTER = true as FILTER,
): FILTER extends true ? ExcludeUndefined<OUT>[] : OUT[] => {
  let i = 0;
  const result: OUT[] = [];

  for (const key of Object.keys(obj)) {
    const value = obj[key as keyof T];
    if (filterUndefinedOutput && value === undefined) continue;
    result.push(callback(value, key as keyof T, i));
    i += 1;
  }

  return result as FILTER extends true ? ExcludeUndefined<OUT>[] : OUT[];
};

export const deepFreeze = <T>(o: T | any): T => {
  Object.entries(o).forEach(([key, value]) => {
    o[key] = value && typeof value === "object" ? deepFreeze(value) : value;
  });
  return Object.freeze(o);
};

export const deepCopy = <T>(obj: T): T => {
  let copy: any = null;

  // Handle the 3 simple types, and null or undefined
  // eslint-disable-next-line eqeqeq
  if (obj == null || typeof obj !== "object") return obj;

  // Handle Date
  if (obj instanceof Date) {
    copy = new Date();
    copy.setTime(obj.getTime());
    return copy;
  }

  if (obj instanceof Array) {
    copy = [];
    for (let i = 0, len = obj.length; i < len; i += 1) {
      copy[i] = deepCopy(obj[i]);
    }

    return copy;
  }

  if (obj instanceof RegExp) return obj;

  if (obj instanceof Object) {
    copy = {};
    for (const attr in obj) {
      if (Object.prototype.hasOwnProperty.call(obj, attr)) {
        copy[attr] = deepCopy(obj[attr]);
      }
    }

    return copy;
  }

  throw new Error("Unable to copy obj! Its type isn't supported.");
};

/** Is defined object, and not an array **/
function isObject(item: any): item is object {
  return item && typeof item === "object" && !Array.isArray(item);
}

function isObjectOrArray(item: any): item is object | any[] {
  return item && (typeof item === "object" || Array.isArray(item));
}

type DeepMerge<T, U> = T extends object
  ? U extends object
    ? {
        [K in keyof T | keyof U]: K extends keyof T
          ? K extends keyof U
            ? T[K] extends any[]
              ? U[K] extends any[]
                ? (T[K][number] | U[K][number])[]
                : DeepMerge<T[K], U[K]>
              : DeepMerge<T[K], U[K]>
            : T[K]
          : K extends keyof U
            ? U[K]
            : never;
      }
    : T
  : U;

export function merge<T extends object, U extends object>(
  base: T | undefined,
  override: U | undefined,
  //If nonEmpty is set, undefined will not override set object
  strategy: "merge" | "nonEmpty" = "merge",
  //merge will merge arrays in place, ie, merge index 0 from base to index 0 of override, as if it was an object, but keep array type
  arrayStrategy: "concat" | "nonEmpty" | "merge" | "override" = "merge",
): DeepMerge<T, U> {
  if (base === undefined || override === undefined) {
    return (override ?? base) as DeepMerge<T, U>;
  }

  if (Array.isArray(base) && Array.isArray(override)) {
    if (arrayStrategy === "nonEmpty") {
      return override.length > 0 ? ([...override] as any) : ([...base] as any);
    }
    if (arrayStrategy === "merge") {
      const longest = Math.max(base.length, override.length);
      const merged = Array.from({ length: longest }, (_, i) => {
        const baseItem = base[i];
        const overrideItem = override[i];
        if (isObjectOrArray(baseItem) && isObjectOrArray(overrideItem)) {
          return merge(baseItem, overrideItem, strategy, arrayStrategy);
        }
        return overrideItem ?? baseItem;
      });
      return merged as any;
    }
    if (arrayStrategy === "concat") {
      return [...base, ...override] as any;
    }
    if (arrayStrategy === "override") {
      return [...override] as any;
    }
  }

  const output: any = { ...base } as T & U;
  if (isObject(base) && isObject(override)) {
    Object.keys(override).forEach((key) => {
      const overrideKey = key as keyof U;
      const baseKey = key as keyof T;

      const baseHasKey = Object.hasOwn(base, baseKey);
      const overrideHasKey = Object.hasOwn(override, overrideKey);

      if ((isObject(override[overrideKey]) || Array.isArray(override[overrideKey])) && baseHasKey) {
        output[overrideKey] = merge(
          base[baseKey] as object,
          override[overrideKey] as object,
          strategy,
          arrayStrategy,
        );
      } else {
        if (strategy === "nonEmpty") {
          output[overrideKey] =
            override[overrideKey] === undefined ? base[baseKey] : override[overrideKey];
        }
        //Need to check if the object has the key, so we only override with undefined if it is actaully defined as undefined
        else if (strategy === "merge") {
          output[overrideKey] = overrideHasKey ? override[overrideKey] : base[baseKey];
        } else {
          output[overrideKey] = baseHasKey ? base[baseKey] : override[overrideKey];
        }
      }
    });
  } else {
    throw new Error(
      "NOT MERGING ANYTHING, this should not happen: " +
        JSON.stringify({ base, override }, null, 2),
    );
  }
  return output;
}

export const promiseTimeout = <T>(time: number): Promise<T> =>
  new Promise<T>((resolve) => setTimeout(resolve, time));

export const promiseDelay =
  <T>(time: number): ((result: T) => Promise<T>) =>
  (result: T) =>
    new Promise<T>((resolve) => setTimeout(resolve, time, result));

export const stop = (err?: string) => {
  throw new Error(err ?? "No error message provided");
};
