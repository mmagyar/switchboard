import init, { start } from "/generated/demo.js";

await init();
const page = await start();
window.addEventListener("pagehide", () => page.free(), { once: true });
