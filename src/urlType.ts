type TrimFirstSlash<S extends string, D extends string = "/"> = S extends `${D}${infer Rest}` ? Rest : S;

type TrimLastSlash<S extends string, D extends string = "/"> = S extends `${infer Rest}${D}`
  ? Rest extends "" // Check if Rest is an empty string
    ? ""
    : Rest
  : S;

type TrimFirstAndLastSlash<S extends string, D extends string = "/"> = TrimFirstSlash<TrimLastSlash<S, D>, D>;

type SplitRaw<S extends string, D extends string = "/"> = S extends ""
  ? []
  : S extends `${infer T}${D}${infer U}`
    ? [T, ...Split<U>]
    : [S];

export type Split<S extends string, D extends string = "/"> = SplitRaw<TrimFirstAndLastSlash<S, D>, D>;

export type RemoveLast<T extends any[]> = T extends [...infer Rest, any] ? Rest : T;

export type Last<T extends any[]> = T extends [...infer _, infer L] ? L : never;

export type AppendIdExceptLast<T extends any[]> = T extends [...infer Rest, infer Last]
  ? [...{ [K in keyof Rest]: Rest[K] extends string ? `${Rest[K]}/:${Rest[K]}_id` : Rest[K] }, Last]
  : T;

export type AppendId<T extends string[]> = {
  [K in keyof T]: T[K] extends string ? `${T[K]}_id` : T[K];
};

export type Join<Segments extends string[], D extends string = "/"> = Segments extends []
  ? ""
  : Segments extends [infer First, ...infer Rest]
    ? First extends string
      ? Rest extends string[]
        ? `${First}${Rest["length"] extends 0 ? "" : `${D}${Join<Rest, D>}`}`
        : First
      : ""
    : "";

type RemoveSpecialChars<T extends string> = T extends `${":"}${infer Rest}`
  ? RemoveSpecialChars<Rest>
  : T extends `${infer Base}${"?"}`
    ? Base
    : T;
type HasColon<T extends string> = T extends `:${string}` ? true : false;
type HasQuestionmark<T extends string> = T extends `${string}?` ? true : false;

type FilterColonSegments<T extends string[]> = T extends []
  ? []
  : T extends [infer First, ...infer Rest]
    ? First extends string
      ? Rest extends string[]
        ? HasColon<First> extends true
          ? [RemoveSpecialChars<First>, ...FilterColonSegments<Rest>]
          : FilterColonSegments<Rest>
        : []
      : []
    : [];
type FilterOptionalSegments<T extends string[]> = T extends []
  ? []
  : T extends [infer First, ...infer Rest]
    ? First extends string
      ? Rest extends string[]
        ? HasQuestionmark<First> extends true
          ? [RemoveSpecialChars<First>, ...FilterOptionalSegments<Rest>]
          : FilterOptionalSegments<Rest>
        : []
      : []
    : [];

type FilterNonOptionalSegments<T extends string[]> = T extends []
  ? []
  : T extends [infer First, ...infer Rest]
    ? First extends string
      ? Rest extends string[]
        ? HasQuestionmark<First> extends true
          ? FilterNonOptionalSegments<Rest>
          : HasColon<First> extends true
            ? [RemoveSpecialChars<First>, ...FilterNonOptionalSegments<Rest>]
            : FilterNonOptionalSegments<Rest>
        : []
      : []
    : [];

/**
 * Returns the path portion of a route definition string, stripping any query string.
 * A `?` that is NOT at the end of the string and NOT immediately followed by `/`
 * is treated as the query-string separator. Optional path param markers (`/:param?`)
 * are preserved because their `?` IS followed by `/` or end-of-string.
 *
 * Uses `${infer H}?${infer Rest}` which splits on the FIRST `?`.
 * When `Rest` starts with `/` or is empty the `?` is an optional-param marker and
 * we keep accumulating. Otherwise, we have found the query-string separator.
 */
type GetPathPart<T extends string, Acc extends string = ""> = T extends `${infer H}?${infer Rest}`
  ? Rest extends "" | `/${string}`
    ? GetPathPart<Rest, `${Acc}${H}?`>
    : `${Acc}${H}`
  : `${Acc}${T}`;

/**
 * Returns the raw query string declared in a route definition (everything after
 * the query-string separator `?`), or `""` if there is none.
 *
 * Uses `${string}?${infer Rest}` which is GREEDY — `${string}` matches up to the
 * LAST `?` in the string. If that last `?` is at end-of-string or followed by `/`
 * (i.e. it is an optional-param marker), there is no declared query string.
 */
type GetQueryPart<T extends string> = T extends `${string}?${infer Rest}`
  ? Rest extends "" | `/${string}`
    ? ""
    : Rest
  : "";

/** Splits `"key1&key2&key3"` into `["key1", "key2", "key3"]`. */
type ExtractQueryKeys<Q extends string> = Q extends ""
  ? []
  : Q extends `${infer K}&${infer Rest}`
    ? [K, ...ExtractQueryKeys<Rest>]
    : [Q];

/** The query-param keys declared in a route definition URL (after the `?` separator). */
export type QueryParamKeys<T extends string> = ExtractQueryKeys<GetQueryPart<T>>;

export type PathToMandatoryKeys<T extends string> = FilterNonOptionalSegments<Split<GetPathPart<T>>>;
export type PathToOptionalKeys<T extends string> = FilterOptionalSegments<Split<GetPathPart<T>>>;
export type PathToAllKeys<T extends string> = FilterColonSegments<Split<GetPathPart<T>>>;

// So we can handle ids with number type
export type FilterByIdEnding<T extends string[], K extends boolean = true> = T extends [
  infer F extends string,
  ...infer R extends string[],
]
  ? (F extends `${string}_id` | `${string}Id` ? K : K extends true ? false : true) extends true
    ? [F, ...FilterByIdEnding<R, K>]
    : FilterByIdEnding<R, K>
  : [];

/**
 * Extracts the schema key (the URL query-param name) from a raw declaration.
 * `"page=:page"` → `"page"`;  `"limit=:limitId"` → `"limit"`;  `"page"` → `"page"`.
 */
export type QueryKeyOf<S extends string> = S extends `${infer K}=:${string}` ? K : S;

/**
 * Extracts the placeholder name used for type derivation from a raw declaration.
 * `"page=:page"` → `"page"`;  `"limit=:limitId"` → `"limitId"`;  `"page"` → `"page"`.
 */
export type QueryPlaceholderOf<S extends string> = S extends `${string}=:${infer P}` ? P : S;

/**
 * Filters a list of raw query-param declarations by whether the placeholder name has
 * an `Id` / `_id` suffix (which signals a numeric type).
 *
 * K=true  → keep the Id-suffixed ones (map each to its schema key)  → ZodNumber
 * K=false → keep the non-Id-suffixed ones (map each to its schema key) → ZodString
 *
 * Works with both `"key=:placeholder"` and bare `"key"` declarations.
 */
export type FilterQueryById<Decls extends string[], K extends boolean = true> = Decls extends [
  infer F extends string,
  ...infer R extends string[],
]
  ? (QueryPlaceholderOf<F> extends `${string}Id` | `${string}_id` ? K : K extends true ? false : true) extends true
    ? [QueryKeyOf<F>, ...FilterQueryById<R, K>]
    : FilterQueryById<R, K>
  : [];

type IsValidOptionalSegments<S extends string[]> = S extends []
  ? true
  : S extends [infer First extends string, ...infer Rest extends string[]]
    ? First extends `${string}?`
      ? Rest extends `${string}?`[]
        ? true
        : Rest["length"] extends 0
          ? true
          : false
      : IsValidOptionalSegments<Rest>
    : true;

export type ValidateOptionalUrl<T extends string> =
  IsValidOptionalSegments<Split<GetPathPart<T>>> extends true ? T : never;
