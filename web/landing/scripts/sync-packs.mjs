// Public API only: the packs.json each configured pack repository published with its latest release
// (docs/PACKS_FROM_REPOSITORIES.md section 5). Every field goes through publicPacks before it is saved; a repository
// that cannot be read keeps its saved snapshot, and nothing here is an instruction to the site.
import { readFile, writeFile } from "node:fs/promises";
import { publicPacks } from "../site-core.js";
const root = new URL("../", import.meta.url);
const config = JSON.parse(
  await readFile(new URL("site.config.json", root), "utf8"),
);
const repos = (Array.isArray(config.packRepositories) ? config.packRepositories : []).filter((r) =>
  /^[\w.-]+\/[\w.-]+$/.test(r),
);
let saved = { updated: null, sources: [] };
try {
  saved = JSON.parse(await readFile(new URL("data/packs.json", root), "utf8"));
} catch {}
const sources = [];
let fetched = 0;
for (const repo of repos) {
  const keep = (saved.sources || []).find((s) => s.repo === repo);
  try {
    const r = await fetch(`https://api.github.com/repos/${repo}/releases/latest`, {
      headers: { Accept: "application/vnd.github+json" },
      signal: AbortSignal.timeout(10000),
    });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    const release = await r.json();
    const asset = (release.assets || []).find((a) => a.name === "packs.json");
    const url = asset?.browser_download_url;
    if (!url || !url.startsWith(`https://github.com/${repo}/releases/download/`))
      throw new Error(`release ${release.tag_name} has no packs.json asset`);
    const d = await fetch(url, { signal: AbortSignal.timeout(10000) });
    if (!d.ok) throw new Error(`packs.json HTTP ${d.status}`);
    const source = publicPacks(repo, release, await d.json());
    if (!source) throw new Error(`release ${release.tag_name} is not a tagged release page of ${repo}`);
    sources.push(source);
    fetched += 1;
    console.log(`${repo} ${source.tag}: ${source.packs.length} packs.`);
  } catch (e) {
    console.log(`${repo}: ${e.message}; ${keep ? "keeping the saved snapshot" : "no snapshot to keep"}.`);
    if (keep) sources.push(keep);
  }
}
if (fetched) {
  await writeFile(
    new URL("data/packs.json", root),
    JSON.stringify({ updated: new Date().toISOString(), sources }, null, 2) + "\n",
  );
  console.log(`Saved ${sources.length} pack source(s).`);
} else {
  console.log("No pack repository answered; existing snapshot preserved.");
}
