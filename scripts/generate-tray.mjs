import { readFile, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import process from "node:process";
import { URL } from "node:url";

// Rasterize the checked-in monochrome master. SHARP_MODULE may point to a
// host-provided Sharp installation; this is not a runtime app dependency.
const require = createRequire(import.meta.url);
const sharp = require(process.env.SHARP_MODULE || "sharp");
const svg = await readFile(
  new URL(
    "../docs/design/skill-man-logo/menubar-template.svg",
    import.meta.url,
  ),
);
const rgba = await sharp(svg).ensureAlpha().raw().toBuffer();
if (rgba.length !== 18 * 18 * 4)
  throw new Error("Expected an 18x18 RGBA tray icon");
for (let i = 0; i < rgba.length; i += 4) {
  if (rgba[i] || rgba[i + 1] || rgba[i + 2])
    throw new Error("Template must contain only black and alpha");
}
await writeFile(new URL("../src-tauri/icons/tray.rgba", import.meta.url), rgba);
await writeFile(new URL("../src-tauri/icons/tray.svg", import.meta.url), svg);
