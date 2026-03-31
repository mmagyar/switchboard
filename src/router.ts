import type { ValidateOptionalUrl } from "./urlType.ts";
import { parseHTTPMethod, RequestError, type HTTPMethods } from "./staticDefs.ts";
import { checkRouteOptionalParameterOrder, decomposeUrl } from "./urlUtils.ts";

type StoredRoute = {
  method: HTTPMethods;
  route: string;
  handler: (req: Request) => Promise<Response> | Response;
};

type ParamEdge = {
  node: TrieNode;
  name: string;
  optional: boolean;
};

type TrieNode = {
  staticChildren: Map<string, TrieNode>;
  paramChild: ParamEdge | null;
  handlers: Map<string, StoredRoute>;
};

const makeNode = (): TrieNode => ({
  staticChildren: new Map(),
  paramChild: null,
  handlers: new Map(),
});

const trieInsert = (
  root: TrieNode,
  method: HTTPMethods,
  normalized: string,
  handler: (req: Request) => Promise<Response> | Response,
): void => {
  const segments = decomposeUrl(normalized).filter((s) => s !== "");
  let node = root;

  for (const segment of segments) {
    if (segment.startsWith(":")) {
      const optional = segment.endsWith("?");
      const name = optional ? segment.slice(1, -1) : segment.slice(1);
      if (!node.paramChild) {
        node.paramChild = { node: makeNode(), name, optional };
      } else if (node.paramChild.optional !== optional) {
        const existing = node.paramChild.optional ? "optional" : "mandatory";
        const incoming = optional ? "optional" : "mandatory";
        throw new Error(
          `Conflicting param edges: cannot add ${incoming} param ":${name}" because an ${existing} param ":${node.paramChild.name}${node.paramChild.optional ? "?" : ""}" already exists at the same position. Route: ${normalized}`,
        );
      }
      node = node.paramChild.node;
    } else {
      if (!node.staticChildren.has(segment)) {
        node.staticChildren.set(segment, makeNode());
      }
      node = node.staticChildren.get(segment)!;
    }
  }

  const entry: StoredRoute = { method, route: normalized, handler };
  node.handlers.set(method, entry);
};

const trieLookup = (node: TrieNode, segments: string[], segIndex: number, method: string): StoredRoute | undefined => {
  if (segIndex >= segments.length) {
    const h = node.handlers.get(method);
    if (h) return h;
    // Path exhausted — follow optional param edges looking for a handler
    if (node.paramChild?.optional) {
      return trieLookup(node.paramChild.node, segments, segIndex, method);
    }
    return undefined;
  }

  const segment = segments[segIndex]!;

  // 1. Prefer exact static match
  const staticChild = node.staticChildren.get(segment);
  if (staticChild) {
    const r = trieLookup(staticChild, segments, segIndex + 1, method);
    if (r) return r;
  }

  // 2. Fall back to param (mandatory or optional, both consume the segment)
  if (node.paramChild) {
    return trieLookup(node.paramChild.node, segments, segIndex + 1, method);
  }

  return undefined;
};

export class Router {
  private readonly root: TrieNode = makeNode();
  private readonly registeredRoutes = new Set<string>();

  constructor(
    public readonly defaultRoute: (r: Request) => Promise<Response> | Response = () =>
      new Response("Not found - Route not defined", { status: 404 }),
  ) {}

  addRoute<T extends string>(
    method: HTTPMethods,
    route: ValidateOptionalUrl<T>,
    handler: (req: Request) => Promise<Response> | Response,
  ) {
    checkRouteOptionalParameterOrder(route);
    const normalized = route.startsWith("/") ? route : `/${route}`;
    const key = `${method}:${normalized}`;
    if (this.registeredRoutes.has(key)) {
      throw new Error(`Duplicate route: ${method.toUpperCase()} ${normalized}`);
    }
    this.registeredRoutes.add(key);
    trieInsert(this.root, method, normalized, handler);
  }

  getRoute(method: HTTPMethods, path: URL): StoredRoute | undefined {
    const segments = decomposeUrl(path.pathname).filter((s) => s !== "");
    return trieLookup(this.root, segments, 0, method);
  }

  async handleRequest(req: Request): Promise<Response> {
    let method: HTTPMethods;
    try {
      method = parseHTTPMethod(req.method);
    } catch (e) {
      if (e instanceof RequestError) {
        return new Response(e.message, { status: e.status });
      }
      throw e;
    }

    const url = new URL(req.url);
    const route = this.getRoute(method, url);
    if (route) {
      return route.handler(req);
    }

    if (method === "head") {
      const getRoute = this.getRoute("get", url);
      if (getRoute) {
        const res = await getRoute.handler(req);
        return new Response(null, { status: res.status, headers: res.headers });
      }
    }

    return this.defaultRoute(req);
  }
}
