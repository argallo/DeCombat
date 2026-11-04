// Public API only: never export a private repository's release notes to the website.
import { readFile, writeFile } from "node:fs/promises";
import { publicReleases } from "../site-core.js";
const root = new URL("../", import.meta.url);
const config = JSON.parse(
  await readFile(new URL("site.config.json", root), "utf8"),
);
try {
  const r = await fetch(
    `https://api.github.com/repos/${config.repository}/releases?per_page=6`,
    {
      headers: { Accept: "application/vnd.github+json" },
      signal: AbortSignal.timeout(10000),
    },
  );
  if (!r.ok) {
    console.log(
      `Public GitHub release feed unavailable (${r.status}); keeping saved release snapshot.`,
    );
    process.exit(0);
  }
  const releases = publicReleases(await r.json()).map((r) => ({
    name: r.name,
    tag_name: r.tag_name,
    body: r.body,
    html_url: r.html_url,
    published_at: r.published_at,
    prerelease: r.prerelease,
    assets: (r.assets || []).map((a) => ({
      name: a.name,
      browser_download_url: a.browser_download_url,
    })),
  }));
  await writeFile(
    new URL("data/releases.json", root),
    JSON.stringify({ updated: new Date().toISOString(), releases }, null, 2) +
      "\n",
  );
  console.log(`Saved ${releases.length} public releases.`);
} catch (e) {
  console.log(
    `Public release sync unavailable: ${e.message}. Existing snapshot preserved.`,
  );
}
