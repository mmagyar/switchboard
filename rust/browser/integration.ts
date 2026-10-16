import { chromium } from "playwright-core";
import { strict as assert } from "node:assert";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dir, "../..");
const artifacts = resolve(root, ".tmp/browser");
await mkdir(artifacts, { recursive: true });
const logPath = resolve(artifacts, "live-server.log");
await Bun.write(logPath, "");
const browser = await chromium.launch({ headless: true });
try {
  const server = Bun.spawn(
    [resolve(import.meta.dir, "examples/demo/target/debug/browser-demo-server")],
    { cwd: root, stdout: Bun.file(logPath), stderr: "inherit" },
  );
  try {
    const deadline = Date.now() + 10_000;
    while (!(await Bun.file(logPath).text()).includes('{"ready":true,"port":3217}')) {
      if (server.exitCode !== null) throw new Error(`Server exited: ${server.exitCode}`);
      if (Date.now() >= deadline) throw new Error("Server readiness deadline exceeded");
      await Bun.sleep(50);
    }
    const response = await fetch("http://127.0.0.1:3217/health");
    assert.equal(response.status, 200);
    assert.equal(await response.text(), "ready");
    for (const variant of ["browser", "reference"]) {
      const page = await browser.newPage({ viewport: { width: 1100, height: 1000 } });
      page.setDefaultTimeout(5_000);
      page.setDefaultNavigationTimeout(10_000);
      const errors: string[] = [];
      const responses: { path: string; status: number }[] = [];
      const frames: string[] = [];
      const sockets: { closed: boolean }[] = [];
      page.on("pageerror", (error) => errors.push(error.message));
      page.on("console", (message) => {
        if (message.type() === "error") errors.push(message.text());
      });
      page.on("response", (response) => {
        responses.push({ path: new URL(response.url()).pathname, status: response.status() });
      });
      page.on("websocket", (socket) => {
        const state = { closed: false };
        sockets.push(state);
        socket.on("framereceived", ({ payload }) => frames.push(payload.toString()));
        socket.on("close", () => { state.closed = true; });
      });
      await page.goto(`http://127.0.0.1:3217${variant === "browser" ? "/" : "/reference"}`, { waitUntil: "load" });
      await page.getByRole("status").filter({ hasText: /^Ready$/ }).waitFor();
      assert.equal(await page.getByText("Item 42; query: a b&c", { exact: true }).textContent(), "Item 42; query: a b&c");
      assert.match(await page.getByText(/^Schema rejected: /).innerText(), /^Schema rejected: /);
      assert.equal(await page.getByText(/^Untrusted JSON: /).textContent(), 'Untrusted JSON: {"title":"wrong title","query":"a b&c"}');
      await page.getByText("Rust server log\nsecond line", { exact: true }).waitFor();
      assert.equal(await page.getByText("Open", { exact: true }).textContent(), "Open");
      await page.screenshot({ path: resolve(artifacts, `${variant}-desktop.png`), fullPage: true });
      await page.setViewportSize({ width: 360, height: 900 });
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
      await page.screenshot({ path: resolve(artifacts, `${variant}-mobile.png`), fullPage: true });
      await page.setViewportSize({ width: 1100, height: 1000 });
  
      const navigation = page.waitForNavigation({ waitUntil: "load" });
      const reload = await fetch("http://127.0.0.1:3217/reload", { method: "POST" });
      assert.equal(reload.status, 204);
      await reload.text();
      await navigation;
      await page.getByRole("status").filter({ hasText: /^Ready$/ }).waitFor();
      await page.getByText("Rust server log\nsecond line", { exact: true }).waitFor();
      assert.deepEqual(await page.evaluate(() => performance.getEntriesByType("navigation").map((entry) => entry instanceof PerformanceNavigationTiming ? entry.type : "unknown")), ["reload"]);
      assert.equal(frames.includes("RELOAD"), true);
      assert.equal(sockets.length, 2);
      assert.equal(sockets[0]?.closed, true);
  
      await page.getByRole("button", { name: "Increment", exact: true }).click();
      assert.equal(await page.getByRole("status").filter({ hasText: /^1$/ }).textContent(), "1");
      await page.getByRole("button", { name: "Dispose resources", exact: true }).click();
      await page.getByText("Closed", { exact: true }).waitFor();
      assert.equal(await page.getByText("Disposed", { exact: true }).textContent(), "Disposed");
      await page.getByRole("button", { name: "Increment", exact: true }).click();
      assert.equal(await page.getByRole("status").filter({ hasText: /^1$/ }).textContent(), "1");
      assert.equal(sockets[1]?.closed, true);
      assert.deepEqual(errors, []);
      assert.equal(responses.filter(({ path, status }) => path === "/api/items/42" && status === 200).length, 2);
      assert.equal(responses.filter(({ path, status }) => path === "/api/refinement/42" && status === 200).length, 4);
      assert.equal(responses.some(({ path, status }) => path === (variant === "browser" ? "/generated/demo_bg.wasm" : "/generated/reference.js") && status === 200), true);
      const evidence = {
        browser: browser.version(),
        typedFetch: "Item 42; query: a b&c",
        typedValidation: await page.getByText(/^Schema rejected: /).textContent(),
        unchecked: await page.getByText(/^Untrusted JSON: /).textContent(),
        countAfterDisposeAndClick: await page.getByRole("status").filter({ hasText: /^1$/ }).textContent(),
        websocketFrames: frames,
        sockets,
        reloadNavigation: true,
        responses,
        errors,
      };
      await Bun.write(resolve(artifacts, `${variant}-evidence.json`), `${JSON.stringify(evidence, null, 2)}\n`);
      console.log(JSON.stringify(evidence, null, 2));
      await page.close();
    }
  } finally {
    server.kill("SIGTERM");
    const exitCode = await server.exited;
    if (exitCode !== 0) {
      console.error(`Server cleanup failed: exit ${exitCode}`);
      process.exitCode = 1;
    }
  }
} finally {
  await browser.close();
}
