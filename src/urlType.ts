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

//TODO add possibility to parse query string definitions as well
export type PathToMandatoryKeys<T extends string> = FilterNonOptionalSegments<Split<T>>;
export type PathToOptionalKeys<T extends string> = FilterOptionalSegments<Split<T>>;
export type PathToAllKeys<T extends string> = FilterColonSegments<Split<T>>;

// So we can handle ids with number type
export type FilterByIdEnding<T extends string[], K extends boolean = true> = T extends [
  infer F extends string,
  ...infer R extends string[],
]
  ? (F extends `${string}_id` | `${string}Id` ? K : K extends true ? false : true) extends true
    ? [F, ...FilterByIdEnding<R, K>]
    : FilterByIdEnding<R, K>
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

export type ValidateOptionalUrl<T extends string> = IsValidOptionalSegments<Split<T>> extends true ? T : never;
