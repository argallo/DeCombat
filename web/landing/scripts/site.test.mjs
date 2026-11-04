import test from "node:test";
import assert from "node:assert/strict";
import {
  safeHttps,
  validDiscord,
  badgeFor,
  sanitizeDirectory,
  catalogMods,
  filterMods,
  publicReleases,
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
