import init, { start } from "/generated/demo.js";

window.addEventListener("pageshow", (event) => {
  if (event.persisted) window.location.reload();
});
await init();
const page = await start();
window.addEventListener("pagehide", () => page?.free(), { once: true });
