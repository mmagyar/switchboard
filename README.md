# Switchboard

A router that is simple to use, not overly complex but can do great things

## Features

- Type-safe route definitions (because we don't trust you)
- Zod validation (so you can't mess up as badly)
- Permission handling (to keep the riffraff out)
- Automatic parameter parsing (since you can't be bothered)
- Error handling (for when you inevitably screw something up)

## Usage

```typescript
import { define, RouteHandlerDefiner } from "@router";
import { z } from "zod";

export type PermissionsType = ("public" | "private")[];
export type User = { name: string };
export const def = define<PermissionsType>();

export const handle = RouteHandlerDefiner<User, PermissionsType>(
  async (_user, _permissionsNeeded, _req) => {
    return "ok"; // Check permissions
  },
  async () => {
    return { name: "John" }; //Set the user, maybe get from jwt / db up to you
  },
  (s, x) => { //How do you want to log output validation errors
    console.warn("** OUTPUT VALIDATION FAILED **", s, " ** DATA ** ", x);
  },
);


const orderDef = def.get(
  "/order",
  [],
  z.object({ fromSearchString: z.string().optional() }),
  z.object({ restaurents: z.array(z.string()) }),
);

export const showItems = handle(orderDef, async () => {
  return { restaurents: ["sdf"] };
});

const routes = [orderDef];

and than server the request, in you http request handler, however you want:
const r = new Router();
routes.map((x) => r.addRoute(x.method, x.path, x.handlerWrapped));
res = await r.handleRequest(req)

```

## TODO
 - Router - implement that if the path has params (sections starting with /: ) they MUST be in this validation
 - Router - optimize this lookup, okay for now - Or just use Bun's resolution of paths
 - Create a more flexible error handling for custom error responses
 - Output formatter for errors, for HTML error pages
