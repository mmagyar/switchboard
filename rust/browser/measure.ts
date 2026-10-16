import { gzipSync } from "node:zlib";
import { resolve } from "node:path";

const generated = resolve(import.meta.dir, "examples/demo/static/generated");
const result = await Bun.build({
  entrypoints: [resolve(import.meta.dir, "reference.ts")],
  outdir: generated,
  target: "browser",
  minify: true,
  sourcemap: "none",
});
if (!result.success) throw new AggregateError(result.logs, "Reference bundle build failed");
const names = ["demo_bg.wasm", "demo.js", "reference.js"];
const files: { name: string; raw: number; gzip: number }[] = [];
for (const name of names) {
  const bytes = await Bun.file(resolve(generated, name)).bytes();
  files.push({ name, raw: bytes.length, gzip: gzipSync(bytes, { level: 9 }).length });
}
const bootstrap = await Bun.file(resolve(generated, "../bootstrap.js")).bytes();
const bootstrapSize = { name: "bootstrap.js", raw: bootstrap.length, gzip: gzipSync(bootstrap, { level: 9 }).length };
const rust = files.filter(({ name }) => name !== "reference.js");
const evidence = {
  bun: Bun.version,
  files: [...files, bootstrapSize],
  rustWasmAndGeneratedJs: {
    raw: rust.reduce((total, file) => total + file.raw, 0),
    gzip: rust.reduce((total, file) => total + file.gzip, 0),
  },
  rustIncludingBootstrap: {
    raw: rust.reduce((total, file) => total + file.raw, bootstrapSize.raw),
    gzip: rust.reduce((total, file) => total + file.gzip, bootstrapSize.gzip),
  },
  compression: "gzip level 9, each HTTP resource independently, no filename/timestamp header",
  scope: "Both apps: typed fetch, literal output rejection, unchecked JSON, DOM counter/disposal, WS logs/close and RELOAD. Rust includes linked schema/serde code; reference imports original src/clientExport.ts and Zod. Shared server HTML/CSS excluded from both.",
};
await Bun.write(resolve(import.meta.dir, "../../.tmp/browser/artifact-sizes.json"), `${JSON.stringify(evidence, null, 2)}\n`);
console.log(JSON.stringify(evidence, null, 2));
