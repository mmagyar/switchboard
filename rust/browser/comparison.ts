import { strict as assert } from "node:assert";
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const pageUrl = new URL("../comparison.html", import.meta.url);
const artifactDir = fileURLToPath(new URL("../../.tmp/comparison/", import.meta.url));
assert.equal(await Bun.file(pageUrl).exists(), true, "The comparison guide must exist");
await mkdir(artifactDir, { recursive: true });

const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch: (request: Request): Response => new URL(request.url).pathname === "/"
    ? new Response(Bun.file(pageUrl), { headers: { "Content-Type": "text/html; charset=utf-8" } })
    : new Response("Not found", { status: 404 }),
});

try {
  const browser = await chromium.launch({ headless: true });
  try {
    const context = await browser.newContext({ javaScriptEnabled: false, reducedMotion: "reduce" });
    const page = await context.newPage();
    page.setDefaultTimeout(3000);
    const errors: string[] = [];
    const requests: string[] = [];
    page.on("pageerror", error => errors.push(error.message));
    page.on("console", message => { if (message.type() === "error") errors.push(message.text()); });
    page.on("request", request => requests.push(request.url()));
    const response = await page.goto(server.url.href, { waitUntil: "load" });
    assert.equal(response?.status(), 200);
    assert.equal(await page.title(), "Switchboard | TypeScript and Rust declaration guide");
    await page.keyboard.press("Tab");
    assert.equal(await page.getByRole("link", { name: "Skip to declarations" }).evaluate(node => node === document.activeElement), true);
    await page.keyboard.press("Enter");
    assert.equal(new URL(page.url()).hash, "#declarations");
    await page.getByRole("navigation", { name: "Guide chapters" }).getByRole("link", { name: "Forms" }).click();
    assert.equal(new URL(page.url()).hash, "#forms");
    assert.equal(await page.getByRole("heading", { name: "Search with a normal HTML form" }).isVisible(), true);
    await page.getByRole("navigation", { name: "Guide chapters" }).getByRole("link", { name: "Browser behavior" }).click();
    assert.equal(new URL(page.url()).hash, "#browser");
    assert.equal(await page.getByRole("heading", { name: "Add browser behavior only when you need it" }).isVisible(), true);
    await page.getByText("See optional browser client declarations", { exact: true }).click();
    assert.equal(await page.getByRole("region", { name: "Rust browser client" }).isVisible(), true);
    await page.getByText("See optional browser client declarations", { exact: true }).click();

    for (const width of [320, 768, 1024, 1440]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.goto(server.url.href, { waitUntil: "load" });
      const layout = await page.getByRole("main").evaluate(() => ({
        viewport: document.documentElement.clientWidth,
        content: document.documentElement.scrollWidth,
        scripts: document.scripts.length,
        brokenAnchors: Array.from(document.links)
          .filter(link => link.getAttribute("href")?.startsWith("#") && !document.getElementById(link.hash.slice(1)))
          .map(link => link.textContent),
      }));
      assert.ok(layout.content <= layout.viewport, `Page overflow at ${width}px: ${JSON.stringify(layout)}`);
      assert.equal(layout.scripts, 0);
      assert.deepEqual(layout.brokenAnchors, []);
      await page.screenshot({ path: `${artifactDir}/guide-${width}.png`, fullPage: true });
      if (width === 320 || width === 1440) {
        await page.screenshot({ path: `${artifactDir}/hero-${width}.png` });
      }
      console.log(`PASS ${width}px, JavaScript disabled, no page overflow, valid chapter anchors`);
    }

    await page.setViewportSize({ width: 320, height: 800 });
    const code = page.getByRole("region", { name: "Rust route declarations" });
    await code.focus();
    const before = await code.evaluate(node => node.scrollLeft);
    await page.keyboard.press("ArrowRight");
    let after = await code.evaluate(node => node.scrollLeft);
    const deadline = Date.now() + 2000;
    // Main-world RAF polling is disabled with page JavaScript, so observe native scrolling from the runner.
    while (after <= before && Date.now() < deadline) {
      await Bun.sleep(20);
      after = await code.evaluate(node => node.scrollLeft);
    }
    assert.ok(after > before, "Overflowing code must be keyboard-scrollable");

    const links = await page.getByRole("contentinfo").getByRole("link").evaluateAll(nodes => nodes.map(node => node.getAttribute("href")));
    for (const href of links) {
      assert.ok(href);
      assert.equal(await Bun.file(new URL(href, pageUrl)).exists(), true, `Source link ${href} must exist`);
    }
    await page.goto(pageUrl.href, { waitUntil: "load" });
    assert.equal(await page.getByRole("heading", { name: "Same cocktail. Different declarations." }).isVisible(), true);
    assert.deepEqual(errors, []);
    assert.deepEqual([...new Set(requests.filter(url => !url.startsWith(server.url.href) && url !== pageUrl.href))], []);
    console.log("PASS keyboard navigation, native disclosure, local-file opening, source links and no external requests");
    await context.close();
  } finally {
    await browser.close();
  }
} finally {
  await server.stop(true);
}
