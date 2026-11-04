import {
  safeHttps,
  validDiscord,
  sanitizeDirectory,
  catalogMods,
  filterMods,
} from "./site-core.js";
const $ = (s) => document.querySelector(s);
const $$ = (s) => [...document.querySelectorAll(s)];
const node = (tag, className, text) => {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined) e.textContent = text;
  return e;
};
const external = (href, label, className = "text-link") => {
  const a = node("a", className, label);
  a.href = safeHttps(href) || "#";
  a.rel = "noopener noreferrer";
  return a;
};
async function getJSON(url) {
  const r = await fetch(url, {
    signal: AbortSignal.timeout(8000),
    cache: "no-cache",
  });
  if (!r.ok) throw new Error(`HTTP ${r.status}`);
  return r.json();
}
let config = {
    repository: "argallo/DeCombat",
    branch: "master",
  },
  mods = [],
  category = "all";
let repo = "https://github.com/argallo/DeCombat";
const menu = $(".menu-toggle");
menu.addEventListener("click", () => {
  const open = menu.getAttribute("aria-expanded") !== "true";
  menu.setAttribute("aria-expanded", String(open));
  menu.setAttribute(
    "aria-label",
    open ? "Close navigation" : "Open navigation",
  );
  $("#nav").classList.toggle("open", open);
});
$$("#nav a").forEach((a) =>
  a.addEventListener("click", () => {
    menu.setAttribute("aria-expanded", "false");
    menu.setAttribute("aria-label", "Open navigation");
    $("#nav").classList.remove("open");
  }),
);
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    menu.setAttribute("aria-expanded", "false");
    $("#nav").classList.remove("open");
  }
});
const dialog = $("#mod-dialog");
$("#close-mod").addEventListener("click", () => dialog.close());
dialog.addEventListener("click", (e) => {
  if (e.target === dialog) {
    const r = dialog.getBoundingClientRect();
    if (
      e.clientX < r.left ||
      e.clientX > r.right ||
      e.clientY < r.top ||
      e.clientY > r.bottom
    )
      dialog.close();
  }
});
dialog.addEventListener("close", () =>
  document.body.classList.remove("dialog-open"),
);
function showMod(m) {
  const detail = $("#mod-detail");
  detail.replaceChildren();
  detail.append(
    node(
      "span",
      m.status === "verified" ? "tag" : "tag amber",
      m.status === "verified"
        ? "VERIFIED DIRECTORY ENTRY"
        : "INCLUDED REFERENCE DEMO",
    ),
  );
  const title = node("h2", "dialog-title", m.name);
  title.id = "mod-dialog-title";
  detail.append(
    title,
    node("p", "mono", `v${m.version} · ${m.author || "Community contributor"}`),
    node("p", "", m.details || m.description),
  );
  detail.append(node("h4", "", "DECLARED CAPABILITIES"));
  const ul = node("ul");
  for (const c of m.capabilities || []) ul.append(node("li", "", c));
  detail.append(ul);
  if (m.sha256) {
    detail.append(
      node("h4", "", "PACKAGE SHA-256"),
      node("code", "hash", m.sha256),
    );
  }
  detail.append(
    node(
      "p",
      "fine",
      m.status === "verified"
        ? "This specific version and hash are listed in the community directory. Review its capabilities before joining a server that uses it."
        : "This is a built reference example, not a published approval or a claim that a live server is running it. The card uses an illustrated preview.",
    ),
  );
  const src =
    safeHttps(m.source_repo) ||
    `${repo}/tree/${config.branch}/${m.sourcePath || "examples/mods"}`;
  detail.append(external(src, "View source on GitHub ↗"));
  document.body.classList.add("dialog-open");
  dialog.showModal();
}
function renderMods() {
  const list = $("#mod-list");
  list.replaceChildren();
  const filtered = filterMods(mods, category, $("#mod-search").value);
  if (!filtered.length) {
    list.append(
      node(
        "p",
        "empty-state",
        category === "verified"
          ? "No publishable verified mod entries yet. Reference demos are available under All mods."
          : "No mods match this search. Try another name or category.",
      ),
    );
    return;
  }
  for (const m of filtered) {
    const card = node("article", "mod-card"),
      visual = node("div", "mod-visual"),
      img = node("img");
    img.src = /^assets\/[a-z\d-]+\.webp$/.test(m.image || "")
      ? m.image
      : "assets/junk-flea.webp";
    img.alt = "";
    img.loading = "lazy";
    img.width = 1280;
    img.height = 720;
    visual.append(
      img,
      node(
        "span",
        m.status === "verified" ? "tag" : "tag amber",
        m.status === "verified" ? "✓ VERIFIED" : "REFERENCE DEMO",
      ),
    );
    if (["blue_crosshair", "drawn_crosshair"].includes(m.id)) {
      const cross = node("span", "crosshair");
      cross.setAttribute("aria-hidden", "true");
      visual.append(cross);
    }
    if (["xray_walls", "hacking_mod"].includes(m.id)) {
      const outlines = node("span", "xray-outlines");
      outlines.setAttribute("aria-hidden", "true");
      outlines.append(node("i"), node("i"));
      visual.append(outlines);
    }
    visual.append(
      node(
        "span",
        "mod-number",
        "ILLUSTRATED PREVIEW / " + m.category.toUpperCase(),
      ),
    );
    const info = node("div", "mod-info"),
      top = node("div", "mod-info-top");
    top.append(node("h3", "", m.name), node("span", "mono", `v${m.version}`));
    info.append(top, node("p", "", m.description));
    const bottom = node("div", "mod-info-footer"),
      button = node("button", "mod-details", "Inspect mod ↗");
    button.type = "button";
    button.setAttribute("aria-label", `Inspect ${m.name}`);
    button.addEventListener("click", () => showMod(m));
    bottom.append(
      node("span", "", `${m.category} / ${m.author || "Community"}`),
      button,
    );
    info.append(bottom);
    card.append(visual, info);
    list.append(card);
  }
}
$$("[data-filter]").forEach((b) =>
  b.addEventListener("click", () => {
    category = b.dataset.filter;
    $$("[data-filter]").forEach((x) => {
      const active = x === b;
      x.classList.toggle("active", active);
      x.setAttribute("aria-pressed", String(active));
    });
    renderMods();
  }),
);
$("#mod-search").addEventListener("input", renderMods);
async function init() {
  try {
    const c = await getJSON("site.config.json");
    if (/^[\w.-]+\/[\w.-]+$/.test(c.repository) && /^[\w./-]+$/.test(c.branch))
      config = { ...config, ...c };
  } catch {}
  repo = `https://github.com/${config.repository}`;
  $$("[data-repo]").forEach((a) => {
    a.href = repo + (a.dataset.repo ? "/" + a.dataset.repo : "");
    a.rel = "noopener noreferrer";
  });
  const invite = validDiscord(config.discordInvite);
  if (invite) {
    $("#discord-link").href = invite;
    $("#discord-link").rel = "noopener noreferrer";
    $("#discord-link").firstChild.textContent = "Join the Discord ";
    $("#discord-status").textContent = "The community is open. You’re invited.";
  }
  const trailer = safeHttps(config.trailerUrl);
  if (trailer) {
    $("#trailer-link").href = trailer;
    $("#trailer-link").textContent = "Watch the launch trailer ↗";
    $(".trailer-panel .tag").textContent = "TRANSMISSION RECEIVED";
    $(".trailer-panel h3").textContent = "THE NEXT CHAPTER IS HERE.";
    $(".trailer-panel>p").textContent =
      "Watch the official DeCombat launch trailer.";
    $(".trailer-footer").textContent = "OFFICIAL LAUNCH TRAILER";
  }
  const [catalog, raw] = await Promise.allSettled([
    getJSON("data/catalog.json"),
    getJSON("data/community.json"),
  ]);
  const directory = sanitizeDirectory(
    raw.status === "fulfilled" ? raw.value : {},
  );
  mods = catalogMods(
    catalog.status === "fulfilled" ? catalog.value.mods : [],
    directory.known_mods,
  );
  renderMods();
  if (catalog.status === "rejected" && raw.status === "rejected")
    $("#catalog-note").textContent =
      "The workshop feed could not be loaded. Please refresh or visit the project on GitHub.";
}
init();

const motionPreference = matchMedia("(prefers-reduced-motion: reduce)");
const loops = $$(".gameplay-loop");
const motionToggle = $("#motion-toggle");
let motionEnabled = !motionPreference.matches;
function updateLoops() {
  motionToggle.textContent = motionEnabled ? "Pause loops Ⅱ" : "Play loops ▶";
  motionToggle.setAttribute("aria-pressed", String(motionEnabled));
  for (const img of loops) {
    const play =
      motionEnabled && img.dataset.inView === "true" && !document.hidden;
    const src = play ? img.dataset.loop : img.dataset.still;
    if (img.getAttribute("src") !== src) img.src = src;
    img.closest("figure").classList.toggle("is-playing", play);
  }
}
motionToggle.addEventListener("click", () => {
  motionEnabled = !motionEnabled;
  updateLoops();
});
motionPreference.addEventListener("change", (e) => {
  motionEnabled = !e.matches;
  updateLoops();
});
document.addEventListener("visibilitychange", updateLoops);
const loopObserver = new IntersectionObserver(
  (entries) => {
    for (const entry of entries)
      entry.target.dataset.inView = String(entry.isIntersecting);
    updateLoops();
  },
  { rootMargin: "100px", threshold: 0 },
);
loops.forEach((img) => loopObserver.observe(img));
updateLoops();
