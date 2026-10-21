import { z } from "zod";

export const workloadName = z.enum(["static", "numeric-path", "nested-query", "validated-post", "html"]);
export type WorkloadName = z.infer<typeof workloadName>;
export const workloads: readonly WorkloadName[] = workloadName.options;

export const fixtures: Readonly<Record<WorkloadName, {
  readonly path: string;
  readonly method: "GET" | "POST";
  readonly requestBody?: string;
  readonly status: number;
  readonly contentType: string;
  readonly responseBody: string;
}>> = {
  static: { path: "/static", method: "GET", status: 200, contentType: "application/json", responseBody: '{"ok":true}' },
  "numeric-path": { path: "/items/42", method: "GET", status: 200, contentType: "application/json", responseBody: '{"id":42}' },
  "nested-query": {
    path: "/search?filter.active=true&values.0=1&values.1=2", method: "GET", status: 200,
    contentType: "application/json", responseBody: '{"filter":{"active":true},"values":[1,2]}',
  },
  "validated-post": {
    path: "/items", method: "POST", requestBody: '{"name":"sample","tags":["a","b"]}', status: 201,
    contentType: "application/json", responseBody: '{"name":"sample","count":2}',
  },
  html: {
    path: "/html/42", method: "GET", status: 200, contentType: "text/html; charset=utf-8",
    responseBody: "<main><h1>Item 42</h1><p>Switchboard</p></main>",
  },
};

const nonnegative = z.number().finite().nonnegative();
export const loadResultSchema = z.object({
  workload: workloadName,
  durationMs: z.number().int().positive(),
  concurrency: z.number().int().positive(),
  timeoutMs: z.number().int().positive(),
  elapsedSeconds: z.number().finite().positive(),
  completed: z.number().int().nonnegative(),
  transportErrors: z.number().int().nonnegative(),
  mismatches: z.number().int().nonnegative(),
  requestsPerSecond: nonnegative,
  latency: z.object({
    p50Ms: nonnegative, p95Ms: nonnegative, p99Ms: nonnegative,
    histogramRelativeError: z.number().finite().positive().max(0.011),
  }).strict(),
  errors: z.array(z.string()),
}).strict();

export type LoadResult = z.infer<typeof loadResultSchema>;

export const parseLoadResult = (text: string): LoadResult => {
  const parsed = loadResultSchema.safeParse(JSON.parse(text));
  if (!parsed.success) throw new Error(`Invalid loadgen output: ${parsed.error.message}`);
  return parsed.data;
};
