import test from "node:test";
import assert from "node:assert/strict";
import { readFile, mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { versionAssets } from "./version-assets.mjs";
import {
  safeHttps,
  validDiscord,
  badgeFor,
  sanitizeDirectory,
  catalogMods,
  filterMods,
  publicReleases,
  youtubeVideoId,
  publicPacks,
  releasePacks,
  releaseTag,
} from "../site-core.js";
const mod = {
  id: "demo",
  name: "Demo",
  version: "1",
  sha256: "a".repeat(64),
  source_repo: "https://github.com/argallo/DeCombat",
  category: "visual",
};
test("unpublished source placeholders never earn verification", () => {
  const placeholder = {
    ...mod,
    source_repo: "https://github.com/<owner>/<repo>",
  };
  assert.equal(badgeFor(mod, [placeholder]).status, "unverified");
  assert.equal(
    sanitizeDirectory({ known_mods: [placeholder] }).known_mods.length,
    0,
  );
  assert.equal(
    badgeFor({ ...mod, sha256: "b".repeat(64) }, [mod]).status,
    "unverified",
  );
  assert.equal(badgeFor(mod, [mod]).status, "verified");
});

test("trailer embeds accept only YouTube video IDs, not lookalike hosts or arbitrary URLs", () => {
  for (const url of [
    "https://youtu.be/tocDTVUPeOM",
    "https://www.youtube.com/watch?v=tocDTVUPeOM&t=32s",
  ])
    assert.equal(youtubeVideoId(url), "tocDTVUPeOM");
  for (const url of [
    "https://youtube.com.evil.test/watch?v=tocDTVUPeOM",
    "https://youtu.be/tocDTVUPeOM/extra",
    "https://www.youtube.com/watch?v=invalid",
    "https://user:pass@youtu.be/tocDTVUPeOM",
    "https://example.org/embed/tocDTVUPeOM",
    "javascript:alert(1)",
  ])
    assert.equal(youtubeVideoId(url), null);
});

test("a deploy changes CSS and module URLs together, including imported modules", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "decombat-cache-"));
  const files = ["style.css", "app.js", "site-core.js"];
  const html = '<link href="style.css"><script src="app.js"></script>';
  try {
    await writeFile(path.join(dir, "index.html"), html);
    await writeFile(path.join(dir, "style.css"), "body {color: white}");
    await writeFile(
      path.join(dir, "app.js"),
      'import {a} from "./site-core.js";',
    );
    await writeFile(path.join(dir, "site-core.js"), "export const a = 1;");
    const first = await versionAssets(dir, files, ["index.html"]);
    assert.ok(
      (await readFile(path.join(dir, first["app.js"]), "utf8")).includes(
        first["site-core.js"],
      ),
    );
    assert.ok(
      !(await readFile(path.join(dir, "index.html"), "utf8")).includes(
        'href="style.css"',
      ),
    );
    await writeFile(path.join(dir, "style.css"), "body {color: yellow}");
    await writeFile(path.join(dir, "index.html"), html);
    const next = await versionAssets(dir, files, ["index.html"]);
    for (const name of files) assert.notEqual(first[name], next[name]);
    assert.ok(
      (await readFile(path.join(dir, next["app.js"]), "utf8")).includes(
        next["site-core.js"],
      ),
    );
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("showcased packs match repository manifests and new captures exist", async () => {
  const root = new URL("../", import.meta.url);
  const { mods } = JSON.parse(
    await readFile(new URL("data/catalog.json", root)),
  );
  for (const entry of mods) {
    const manifest = JSON.parse(
      await readFile(new URL(`../../${entry.sourcePath}/manifest.json`, root)),
    );
    assert.equal(entry.version, manifest.version, entry.id);
    assert.equal(entry.id, manifest.id);
    for (const asset of [entry.image, entry.loop].filter(Boolean)) {
      assert.match(asset, /^assets\/[a-z\d-]+\.(?:webp|gif)$/);
      assert.ok(
        (await readFile(new URL(asset, root))).byteLength < 10 * 1024 * 1024,
      );
    }
  }
  const html = await readFile(new URL("index.html", root), "utf8");
  assert.doesNotMatch(html, /LAUNCH TRAILER.*COMING SOON|WORKFLOW CONCEPT/);
  assert.match(html, /id="beta-form"/);
  assert.match(html, /id="beta-consent"/);
});
test("private and documentation server addresses do not leak into the public catalog", () => {
  const base = { name: "Community server", policy: "vanilla" };
  const addresses = [
    "192.0.2.10:10000",
    "10.0.0.1:10000",
    "127.0.0.1:10000",
    "192.168.1.10:10000",
    "172.20.0.1:10000",
    "203.0.113.9:10000",
    "vanilla.example.invalid:10000",
  ];
  assert.equal(
    sanitizeDirectory({
      servers: addresses.map((address) => ({ ...base, address })),
    }).servers.length,
    0,
  );
  assert.equal(
    sanitizeDirectory({
      servers: [{ ...base, address: "play.decombat.com:10000" }],
    }).servers.length,
    1,
  );
});
test("a reviewed mod version does not promote every version of a reference mod", () => {
  const entries = catalogMods(
    [{ ...mod, version: "2", status: "verified" }],
    [mod],
  );
  assert.deepEqual(
    entries.map((m) => [m.version, m.status]),
    [
      ["1", "verified"],
      ["2", "reference"],
    ],
  );
  assert.equal(filterMods(entries, "verified").length, 1);
  assert.equal(filterMods(entries, "all", "not present").length, 0);
});
test("untrusted URLs and invalid invites cannot become clickable scripts", () => {
  for (const s of [
    "javascript:alert(1)",
    "data:text/html,test",
    "http://insecure.example",
    "https://u:p@github.com",
    "https://example.invalid",
    "https://github.com/<owner>",
  ])
    assert.equal(safeHttps(s), null);
  assert.equal(validDiscord("https://evil.example/invite/decombat"), null);
  assert.equal(
    validDiscord("https://discord.gg/decombat"),
    "https://discord.gg/decombat",
  );
});
test("release feed excludes drafts and unsafe links but labels prereleases downstream", () => {
  assert.equal(
    publicReleases([
      { draft: true, html_url: mod.source_repo },
      { html_url: "javascript:alert(1)" },
      { html_url: "https://evil.example/releases" },
    ]).length,
    0,
  );
  assert.equal(
    publicReleases([
      {
        html_url: mod.source_repo,
        prerelease: true,
        published_at: "2026-11-01",
      },
    ]).length,
    1,
  );
});

test("release packs keep only plain names, their own release page and hashes, and never earn verification", () => {
  const release = {
    tag_name: "v1",
    html_url: "https://github.com/argallo/DeCombat-mods/releases/tag/v1",
    published_at: "2026-11-08T20:51:58Z",
  };
  const row = {
    id: "case_reel",
    name: "Case Reel",
    version: "0.1.1",
    sha256: "d6c491bbb26180afed35f6b6a046c92ccff2da97f4116122c48767ddd5bc8a99",
    size: 15774,
    kind: "shared",
    tags: ["cosmetic", "menu"],
    capabilities: ["content.hud"],
    author: "DeCombat",
    description: "Case openings spin.",
    source_repo: "https://github.com/argallo/DeCombat-mods",
    zip: "case_reel-0.1.1.zip",
  };
  const good = publicPacks("argallo/DeCombat-mods", release, { packs: [row] });
  assert.equal(good.tag, "v1");
  assert.equal(good.packs.length, 1);
  assert.equal(good.packs[0].zip, "case_reel-0.1.1.zip");
  // the release page must be this repository's own tagged page, and the tag a release tag
  assert.equal(publicPacks("argallo/DeCombat-mods", { ...release, html_url: "https://github.com/evil/x/releases/tag/v1" }, { packs: [row] }), null);
  assert.equal(publicPacks("argallo/DeCombat-mods", { ...release, tag_name: "main", html_url: "https://github.com/argallo/DeCombat-mods/releases/tag/main" }, { packs: [row] }), null);
  assert.equal(publicPacks("argallo/DeCombat-mods", { ...release, draft: true }, { packs: [row] }), null);
  assert.equal(publicPacks("../etc", release, { packs: [row] }), null);
  assert.equal(releaseTag("v0.5.0-beta6"), true);
  assert.equal(releaseTag("latest"), false);
  assert.equal(releaseTag("-v1"), false);
  // rows with a path in the zip name, a bad hash, an id with spaces, or a duplicate are dropped; defaults fill in
  const rows = publicPacks("argallo/DeCombat-mods", release, {
    packs: [
      { ...row, zip: "../../evil.zip" },
      { ...row, id: "bad hash", sha256: "nothex" },
      { ...row, id: "reel b" },
      { ...row, id: "plain", zip: undefined, name: "", tags: ["ok", "../x"], source_repo: "javascript:alert(1)", description: "x".repeat(700) },
      { ...row, id: "plain" },
    ],
  }).packs;
  assert.deepEqual(rows.map((p) => [p.id, p.zip, p.name, p.tags, p.source_repo, p.description.length]), [
    ["plain", "plain-0.1.1.zip", "plain", ["ok"], null, 600],
  ]);
  // as catalog entries: status pack, a category the filters know, the release and the zip link
  const data = { sources: [{ ...good, packs: [good.packs[0], { ...good.packs[0], id: "variant_prism_scar", name: "Prism SCAR", tags: ["variant", "camo"] }, { ...good.packs[0], id: "freight_hall", tags: ["map"] }] }] };
  const packs = releasePacks(data);
  assert.deepEqual(packs.map((p) => [p.id, p.status, p.category]), [
    ["case_reel", "pack", "visual"],
    ["variant_prism_scar", "pack", "visual"],
    ["freight_hall", "pack", "maps"],
  ]);
  assert.equal(packs[0].release.zip, "https://github.com/argallo/DeCombat-mods/releases/download/v1/case_reel-0.1.1.zip");
  assert.equal(packs[0].release.url, release.html_url);
  // in the catalog: a reviewed version keeps Verified and gains the release; an unknown pack is a pack of its own
  const reviewed = { ...mod, id: "case_reel", version: "0.1.1", sha256: row.sha256 };
  const entries = catalogMods([], [reviewed], packs);
  assert.deepEqual(entries.map((m) => [m.id, m.status]), [
    ["case_reel", "verified"],
    ["variant_prism_scar", "pack"],
    ["freight_hall", "pack"],
  ]);
  assert.equal(entries[0].release.tag, "v1");
  assert.equal(filterMods(entries, "verified").length, 1);
  // the directory row keeps its own category (community); the release packs carry theirs
  assert.equal(filterMods(entries, "visual").length, 1);
  assert.equal(filterMods(entries, "all", "prism").length, 1);
  // a pack of a reference version lends it the release, no duplicate row
  const withRef = catalogMods([{ ...mod, id: "freight_hall", version: "0.1.1", category: "maps" }], [], packs);
  assert.deepEqual(withRef.map((m) => [m.id, m.status, m.release?.tag]), [
    ["freight_hall", "reference", "v1"],
    ["case_reel", "pack", "v1"],
    ["variant_prism_scar", "pack", "v1"],
  ]);
  // no packs: the catalog is what it was
  assert.deepEqual(catalogMods([mod], [], []).map((m) => m.status), ["reference"]);
});

test("the saved pack snapshot is what publicPacks would keep", async () => {
  const root = new URL("../", import.meta.url);
  const data = JSON.parse(await readFile(new URL("data/packs.json", root), "utf8"));
  assert.ok(Array.isArray(data.sources));
  for (const s of data.sources) {
    const again = publicPacks(s.repo, { tag_name: s.tag, html_url: s.url, published_at: s.published_at }, s);
    assert.ok(again, s.repo);
    assert.equal(again.packs.length, s.packs.length, s.repo);
    assert.deepEqual(again.packs, s.packs, s.repo);
  }
});

test("published server listings retain TLS identity and visual-mod policy", () => {
  const server = {
    id: "community",
    name: "Community server",
    address: "play.decombat.com:10000",
    policy: "vanilla",
    fingerprint: Array(32).fill("AB").join(":"),
    client_visual_allowed: false,
    website: "https://decombat.com",
    mods: [],
  };
  const [published] = sanitizeDirectory({ servers: [server] }).servers;
  assert.equal(published.fingerprint, server.fingerprint);
  assert.equal(published.client_visual_allowed, false);
  assert.equal(published.website, "https://decombat.com/");
});
