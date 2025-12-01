import type { ValidateOptionalUrl } from "./urlType.ts";
import { parseHTTPMethod, type HTTPMethods } from "./staticDefs.ts";
import { checkRouteOptionalParameterOrder, decomposeUrl } from "./urlUtils.ts";

export const matchRoute = (route: string, path: URL): boolean => {
  const routeParts = decomposeUrl(route);
  const pathParts = decomposeUrl(path.pathname);

  //TODO precompute this
  const partsMin = routeParts.filter((x) => !x.endsWith("?")).length;
  const partsMax = routeParts.length;
  const pathPartCount = pathParts.length;
  if (partsMin > pathPartCount || partsMax < pathPartCount) return false;

  for (let i = 0; i < routeParts.length; i++) {
    if (routeParts[i] === pathParts[i]) continue;
    if (routeParts[i]?.startsWith(":")) continue;
    return false;
  }

  return true;
};

export class Router {
  routes: {
    method: HTTPMethods;
    route: string;
    handler: (req: Request) => Promise<Response> | Response;
  }[] = [];
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

    this.routes.push({
      method,
      route: route.startsWith("/") ? route : `/${route}`,
      handler,
    });
  }
  getRoute(method: HTTPMethods, path: URL) {
    //TODO optimize this lookup, okay for now
    return this.routes.find((r) => r.method === method && matchRoute(r.route, path));
  }

  handleRequest(req: Request): Promise<Response> | Response {
    const url = new URL(req.url);
    const route = this.getRoute(parseHTTPMethod(req.method), url);
    if (route) {
      return route.handler(req);
    }
    return this.defaultRoute(req);
  }
}
