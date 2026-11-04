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
export function catalogMods(reference, known) {
  const refs = Array.isArray(reference) ? reference : [];
  const reviewed = sanitizeDirectory({ known_mods: known }).known_mods;
  return [
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
