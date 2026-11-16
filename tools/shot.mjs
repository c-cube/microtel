// Screenshot a page with headless chromium.
//
//   node shot.mjs http://127.0.0.1:8096/spans out.png [--width=1440] [--height=900]
//                 [--scale=2] [--full] [--click=".chip"]
//
// Needs chromium: `playwright install chromium` (see README).

import { mkdirSync } from "node:fs";
import { dirname } from "node:path";
import pw from "playwright-core";

const args = process.argv.slice(2);
const flags = new Map();
const pos = [];
for (const a of args) {
	const m = a.match(/^--([^=]+)(?:=(.*))?$/);
	if (m) flags.set(m[1], m[2] ?? "true");
	else pos.push(a);
}

const [url, out] = pos;
if (!url || !out) {
	console.error(
		"usage: node shot.mjs <url> <out.png> [--width=1440] [--height=900] [--scale=2] [--full] [--click=sel]",
	);
	process.exit(2);
}

const browser = await pw.chromium.launch();
const page = await browser.newPage({
	viewport: {
		width: Number(flags.get("width") ?? 1440),
		height: Number(flags.get("height") ?? 900),
	},
	deviceScaleFactor: Number(flags.get("scale") ?? 2),
});

const problems = [];
page.on(
	"console",
	(m) => m.type() === "error" && problems.push(`console: ${m.text()}`),
);
page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));

await page.goto(url, { waitUntil: "networkidle" });
await page.waitForTimeout(600);
if (flags.has("click")) {
	await page.click(flags.get("click"));
	await page.waitForTimeout(400);
}

mkdirSync(dirname(out), { recursive: true });
await page.screenshot({ path: out, fullPage: flags.has("full") });
await browser.close();

console.log(problems.length ? problems.join("\n") : `${out} ok`);
process.exit(problems.length ? 1 : 0);
