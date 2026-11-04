import { readFile, writeFile, mkdir, cp, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { sanitizeDirectory, validDiscord, safeHttps } from "../site-core.js";
const root = fileURLToPath(new URL("../", import.meta.url));
const dest = path.join(root, "dist");
const config = JSON.parse(
  await readFile(path.join(root, "site.config.json"), "utf8"),
);
if (!/^[\w.-]+\/[\w.-]+$/.test(config.repository))
  throw Error("Invalid repository");
if (config.discordInvite && !validDiscord(config.discordInvite))
  throw Error("Invalid Discord invite");
if (config.trailerUrl && !safeHttps(config.trailerUrl))
  throw Error("Trailer must use HTTPS");
await rm(dest, { recursive: true, force: true });
await mkdir(dest, { recursive: true });
for (const name of [
  "index.html",
  "privacy.html",
  "style.css",
  "app.js",
  "site-core.js",
  "site.config.json",
  "assets",
  "data",
])
  await cp(path.join(root, name), path.join(dest, name), { recursive: true });
// Keep the beta signup implementation shared with web/beta.
for (const name of ["beta.css", "beta.js"])
  await cp(path.join(root, "../beta", name), path.join(dest, name));
let community;
try {
  community = JSON.parse(
    await readFile(path.join(root, "../../community/community.json"), "utf8"),
  );
} catch {
  community = JSON.parse(
    await readFile(path.join(root, "data/community.json"), "utf8"),
  );
}
const publicDirectory = sanitizeDirectory(community);
await writeFile(
  path.join(dest, "data/community.json"),
  JSON.stringify(publicDirectory, null, 2) + "\n",
);
// Also expose the canonical schema at the site's root for the game client.
await writeFile(
  path.join(dest, "community.json"),
  JSON.stringify(publicDirectory, null, 2) + "\n",
);
await mkdir(path.join(dest, "github"), { recursive: true });
await writeFile(
  path.join(dest, "github/index.html"),
  `<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width"><meta http-equiv="refresh" content="0;url=https://github.com/${config.repository}"><title>DeCombat on GitHub</title><a href="https://github.com/${config.repository}">Continue to DeCombat on GitHub</a>`,
);
await writeFile(
  path.join(dest, "robots.txt"),
  `User-agent: *\nAllow: /\nSitemap: ${config.siteUrl}/sitemap.xml\n`,
);
await writeFile(
  path.join(dest, "sitemap.xml"),
  `<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc>${config.siteUrl}/</loc></url><url><loc>${config.siteUrl}/privacy</loc></url></urlset>`,
);
await writeFile(
  path.join(dest, "404.html"),
  `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><link rel="stylesheet" href="/style.css"><title>Off the map — DeCombat</title><main class="section wrap"><p class="eyebrow">404 / OFF THE MAP</p><h1>LET’S GET YOU BACK.</h1><p>This location doesn’t exist.</p><a class="button" href="/">Return to base ↗</a></main></html>`,
);
console.log(
  `Built ${dest}\n${publicDirectory.known_mods.length} publishable verified mods · ${publicDirectory.servers.length} public server listings`,
);
