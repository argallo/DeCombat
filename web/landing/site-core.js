// Shared validation for browser rendering, build preparation, and content tests.
export const SHA256 = /^[a-f0-9]{64}$/i;
export function safeHttps(value) {
  if (typeof value !== "string" || /[<>\s]/.test(value)) return null;
  try {
    const u = new URL(value);
    if (
      u.protocol !== "https:" ||
      u.username ||
      u.password ||
      /(?:^|\.)(invalid|localhost)$/.test(u.hostname) ||
      /^(?:example\.com|example\.org|example\.net)$/.test(u.hostname)
    )
      return null;
    return u.href;
  } catch {
    return null;
  }
}
export function validDiscord(value) {
  const url = safeHttps(value);
  if (!url) return null;
  const u = new URL(url);
  return (u.hostname === "discord.gg" && /^\/[\w-]+$/.test(u.pathname)) ||
    (u.hostname === "discord.com" && /^\/invite\/[\w-]+$/.test(u.pathname))
    ? url
    : null;
}
// Accept only canonical YouTube video URLs; never embed arbitrary config URLs.
export function youtubeVideoId(value) {
  const safe = safeHttps(value);
  if (!safe) return null;
  const u = new URL(safe);
  let id = null;
  if (u.hostname === "youtu.be") id = u.pathname.slice(1);
  if (
    ["youtube.com", "www.youtube.com"].includes(u.hostname) &&
    u.pathname === "/watch"
  )
    id = u.searchParams.get("v");
  return /^[A-Za-z0-9_-]{11}$/.test(id || "") ? id : null;
}
export function badgeFor(mod, known) {
  const info = known.find(
    (k) =>
      k.id === mod.id &&
      k.version === mod.version &&
      k.sha256?.toLowerCase() === mod.sha256?.toLowerCase() &&
      SHA256.test(k.sha256) &&
      safeHttps(k.source_repo),
  );
  return { status: info ? "verified" : "unverified", info: info || null };
}
export function publicServer(server) {
  if (
    !server ||
    typeof server.address !== "string" ||
    typeof server.name !== "string" ||
    !["vanilla", "modded"].includes(server.policy)
  )
    return false;
  const h = server.address.split(":")[0].toLowerCase();
  return (
    !/(placeholder|fictional|example)/i.test(server.name) &&
    h.length > 0 &&
    !/(?:^|\.)(localhost|invalid|local)$/.test(h) &&
    !/^(?:0\.|10\.|127\.|192\.168\.|192\.0\.2\.|198\.51\.100\.|203\.0\.113\.|169\.254\.|172\.(?:1[6-9]|2\d|3[01])\.)/.test(
      h,
    ) &&
    !/(?:^|\.)example\.(?:com|net|org)$/.test(h)
  );
}
export function sanitizeDirectory(doc) {
  return {
    schema_version: 1,
    updated: typeof doc.updated === "string" ? doc.updated : null,
    servers: (Array.isArray(doc.servers) ? doc.servers : [])
      .filter(publicServer)
      .map((s) => ({
        id: s.id,
        name: s.name,
        description: s.description || "",
        region: s.region,
        address: s.address,
        policy: s.policy,
        fingerprint: s.fingerprint,
        client_visual_allowed: s.client_visual_allowed,
        website: safeHttps(s.website) || undefined,
        mods: Array.isArray(s.mods) ? s.mods : [],
      })),
    known_mods: (Array.isArray(doc.known_mods) ? doc.known_mods : []).filter(
      (m) =>
        typeof m.id === "string" &&
        typeof m.name === "string" &&
        typeof m.version === "string" &&
        SHA256.test(m.sha256) &&
        safeHttps(m.source_repo),
    ),
  };
}
// Release packs (docs/PACKS_FROM_REPOSITORIES.md section 5): the packs.json a pack repository's tagged release
// publishes, as the site lists them. Every field is a stranger's text and is kept only when it is a plain word, a
// hash, or a GitHub URL of that very repository; a pack listed here is NOT verified (that badge is the directory's).
const PLAIN = /^[A-Za-z0-9._-]{1,100}$/;
const BRANCHES = ["main", "master", "latest", "HEAD", "develop", "dev", "trunk"];
export function releaseTag(tag) {
  return (
    typeof tag === "string" &&
    tag.length <= 64 &&
    !BRANCHES.includes(tag) &&
    /^(?:v\d|\d)[A-Za-z0-9._-]*$/.test(tag)
  );
}
const word = (v, max, fallback = "") =>
  typeof v === "string" && v.trim() ? v.trim().slice(0, max) : fallback;
const words = (v, max) =>
  (Array.isArray(v) ? v : [])
    .filter((t) => typeof t === "string" && PLAIN.test(t))
    .slice(0, max);
export function publicPacks(repo, release, doc) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repo || "")) return null;
  const tag = release?.tag_name;
  const page = `https://github.com/${repo}/releases/tag/${tag}`;
  if (!releaseTag(tag) || release.draft || safeHttps(release.html_url) !== page)
    return null;
  const rows = Array.isArray(doc?.packs) ? doc.packs : Array.isArray(doc) ? doc : [];
  const seen = new Set();
  const packs = [];
  for (const p of rows.slice(0, 500)) {
    if (!p || !PLAIN.test(p.id || "") || !PLAIN.test(p.version || "") || !SHA256.test(p.sha256 || ""))
      continue;
    const zip = typeof p.zip === "string" ? p.zip : `${p.id}-${p.version}.zip`;
    if (!/^[A-Za-z0-9._-]{1,200}\.zip$/.test(zip) || zip.startsWith("-")) continue;
    const key = `${p.id}@${p.version}`;
    if (seen.has(key)) continue;
    seen.add(key);
    packs.push({
      id: p.id,
      name: word(p.name, 120, p.id),
      version: p.version,
      sha256: p.sha256.toLowerCase(),
      size: Number.isInteger(p.size) && p.size >= 0 ? p.size : null,
      kind: PLAIN.test(p.kind || "") ? p.kind : "shared",
      tags: words(p.tags, 16),
      capabilities: words(p.capabilities, 32),
      author: word(p.author, 80),
      description: word(p.description, 600),
      source_repo: safeHttps(p.source_repo),
      zip,
    });
  }
  return { repo, tag, url: page, published_at: word(release.published_at, 40, null), packs };
}
// The rows of data/packs.json as catalog entries: status "pack", a category the filters know, the release and zip.
export function releasePacks(data) {
  const out = [];
  for (const s of Array.isArray(data?.sources) ? data.sources : []) {
    const src = publicPacks(s.repo, { tag_name: s.tag, html_url: s.url, published_at: s.published_at }, s);
    if (!src) continue;
    for (const p of src.packs) {
      const t = new Set(p.tags);
      const category =
        t.has("map") || /_map$|^map_|hall|container/.test(p.id) ? "maps"
        : t.has("camo") || t.has("variant") || t.has("cosmetic") || t.has("menu") || t.has("hud") ? "visual"
        : "gameplay";
      out.push({
        ...p,
        category,
        status: "pack",
        order: 50,
        release: {
          repo: src.repo,
          tag: src.tag,
          url: src.url,
          zip: `https://github.com/${src.repo}/releases/download/${src.tag}/${p.zip}`,
        },
      });
    }
  }
  return out;
}
export function catalogMods(reference, known, packs = []) {
  const refs = Array.isArray(reference) ? reference : [];
  const reviewed = sanitizeDirectory({ known_mods: known }).known_mods;
  const entries = [
    ...reviewed.map((m) => ({
      ...refs.find((r) => r.id === m.id),
      ...m,
      category: refs.find((r) => r.id === m.id)?.category || "community",
      status: "verified",
    })),
    ...refs
      .filter(
        (r) => !reviewed.some((m) => m.id === r.id && m.version === r.version),
      )
      .map((r) => ({ ...r, status: "reference" })),
  ];
  // a release pack of a listed version lends that row its release; any other appears as a pack of its own
  for (const p of Array.isArray(packs) ? packs : []) {
    const same = entries.find((e) => e.id === p.id && e.version === p.version);
    if (same) {
      if (!same.release) same.release = p.release;
      if (!same.sha256) same.sha256 = p.sha256;
    } else entries.push(p);
  }
  return entries;
}
export function filterMods(mods, category = "all", text = "") {
  const query = text.trim().toLowerCase();
  return mods.filter(
    (m) =>
      (category === "all" ||
        (category === "verified"
          ? m.status === "verified"
          : m.category === category)) &&
      `${m.name} ${m.author} ${m.description}`.toLowerCase().includes(query),
  );
}
export function publicReleases(data) {
  return (Array.isArray(data) ? data : [])
    .filter(
      (r) =>
        !r.draft &&
        safeHttps(r.html_url) &&
        new URL(r.html_url).hostname === "github.com",
    )
    .sort((a, b) => new Date(b.published_at) - new Date(a.published_at))
    .slice(0, 3);
}
// Preserve the previous directory helper interface for consumers.
export function filterServers(doc, f) {
  return (doc.servers || []).filter(
    (s) =>
      (!f.policy || f.policy === "all" || s.policy === f.policy) &&
      (!f.region || s.region === f.region) &&
      (!f.text ||
        `${s.name} ${s.description || ""} ${(s.mods || []).map((m) => m.id).join(" ")}`
          .toLowerCase()
          .includes(f.text.toLowerCase())) &&
      (!f.players ||
        (s.players &&
          (f.players === "some"
            ? s.players.current > 0
            : f.players === "empty"
              ? s.players.current === 0
              : s.players.current < s.players.max))),
  );
}
