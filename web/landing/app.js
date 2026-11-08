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
      m.status === "verified" ? "VERIFIED DIRECTORY ENTRY" : "BUILT MOD PACK",
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
        : "This pack is built and included in the project. Availability depends on your server; it is not listed in the general verified-mod directory.",
    ),
  );
  const sourceLink = (p) => (/^https:\/\/github\.com\/[\w.-]+\/[\w.-]+\/tree\/[\w.-]+\/[\w./-]+$/.test(p) ? p : `${repo}/tree/${config.branch}/${p}`);
  const src = m.sourcePath
    ? sourceLink(m.sourcePath)
    : safeHttps(m.source_repo) || `${repo}/tree/${config.branch}/examples/mods`;
  detail.append(external(src, "View source on GitHub ↗"));
  if (/^(examples\/mods\/[a-z\d_]+|https:\/\/github\.com\/[\w.-]+\/[\w.-]+\/tree\/[\w.-]+\/mods\/[a-z\d_]+)$/.test(m.relatedSourcePath || "")) {
    detail.append(
      external(
        sourceLink(m.relatedSourcePath),
        `${m.relatedName} source ↗`,
        "text-link related-source",
      ),
    );
  }
  document.body.classList.add("dialog-open");
  dialog.showModal();
}
function renderMods() {
  const list = $("#mod-list");
  list
    .querySelectorAll(".gameplay-loop")
    .forEach((img) => loopObserver.unobserve(img));
  list.replaceChildren();
  const filtered = filterMods(mods, category, $("#mod-search").value).sort(
    (a, b) => (a.order ?? 99) - (b.order ?? 99),
  );
  if (!filtered.length) {
    list.append(
      node(
        "p",
        "empty-state",
        category === "verified"
          ? "No publishable verified entries yet. Built packs are available under All mods."
          : "No mods match this search. Try another name or category.",
      ),
    );
    return;
  }
  for (const m of filtered) {
    const hasImage = /^assets\/[a-z\d-]+\.webp$/.test(m.image || "");
    const card = node("article", "mod-card");
    if (hasImage) {
      const visual = node("div", "mod-visual"),
        img = node("img");
      img.src = m.image;
      img.alt = m.imageAlt || m.name;
      img.loading = "lazy";
      img.width = 1280;
      img.height = 720;
      if (/^assets\/[a-z\d-]+\.(?:webp|gif)$/.test(m.loop || "")) {
        img.className = "gameplay-loop";
        img.dataset.still = m.image;
        img.dataset.loop = m.loop;
        visual.append(img);
        loopObserver.observe(img);
      } else {
        const link = node("a", "capture-link");
        link.href = m.image;
        link.target = "_blank";
        link.rel = "noopener";
        link.setAttribute("aria-label", `View full ${m.name} screenshot`);
        link.append(img);
        visual.append(link);
      }
      visual.append(
        node("span", "mod-number", m.captureLabel || "CAPTURED IN DECOMBAT"),
      );
      card.append(visual);
    }
    const info = node("div", "mod-info"),
      top = node("div", "mod-info-top");
    top.append(node("h3", "", m.name), node("span", "mono", `v${m.version}`));
    info.append(
      node(
        "span",
        m.status === "verified" ? "tag" : "tag amber",
        m.status === "verified" ? "✓ VERIFIED" : "BUILT PACK",
      ),
      top,
      node("p", "", m.shortDescription || m.description),
    );
    const bottom = node("div", "mod-info-footer"),
      button = node("button", "mod-details", "Mod details ↗");
    button.type = "button";
    button.setAttribute("aria-label", `Inspect ${m.name}`);
    button.addEventListener("click", () => showMod(m));
    bottom.append(
      node("span", "", `${m.category} / ${m.author || "Community"}`),
      button,
    );
    info.append(bottom);
    card.append(info);
    list.append(card);
  }
  updateLoops();
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
$("[data-watch-trailer]").addEventListener("click", (event) => {
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  event.preventDefault();
  const player = $("#trailer-player");
  const rect = player.getBoundingClientRect();
  const headerBottom = $(".header").getBoundingClientRect().bottom;
  const inset = Math.max(16, (innerHeight - headerBottom - rect.height) / 2);
  if (location.hash !== "#trailer-player")
    history.pushState(null, "", "#trailer-player");
  player.focus({ preventScroll: true });
  window.scrollTo({
    top: Math.max(0, scrollY + rect.top - headerBottom - inset),
    behavior: motionPreference.matches ? "instant" : "smooth",
  });
});
// Bind playback immediately; a slow config request must not navigate away.
function setupTrailer() {
  const videoId = $("#trailer-player").dataset.videoId;
  if (/^[A-Za-z0-9_-]{11}$/.test(videoId || "")) {
    const watch = `https://www.youtube.com/watch?v=${videoId}`;
    $("#trailer-link").href = watch;
    const playTrailer = (start = 0) => {
      const frame = node("iframe");
      frame.src = `https://www.youtube-nocookie.com/embed/${videoId}?autoplay=1&rel=0&start=${start}`;
      frame.title = "DeCombat official launch trailer";
      frame.allow = "autoplay; encrypted-media; picture-in-picture; fullscreen";
      frame.allowFullscreen = true;
      frame.referrerPolicy = "strict-origin-when-cross-origin";
      $("#trailer-player").replaceChildren(frame);
      frame.focus();
    };
    $("#trailer-play").disabled = false;
    $("#trailer-play").addEventListener("click", (e) => {
      e.preventDefault();
      playTrailer();
    });
    $$("[data-trailer-start]").forEach((a) => {
      const start = Number(a.dataset.trailerStart);
      a.disabled = false;
      a.addEventListener("click", (e) => {
        e.preventDefault();
        playTrailer(start);
      });
    });
  }
}
setupTrailer();

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
const motionToggles = $$("[data-motion-toggle]");
let motionEnabled = !motionPreference.matches;
function updateLoops() {
  for (const toggle of motionToggles) {
    toggle.textContent = motionEnabled ? "Pause loops Ⅱ" : "Play loops ▶";
    toggle.setAttribute("aria-pressed", String(motionEnabled));
  }
  for (const img of $$(".gameplay-loop")) {
    const play =
      motionEnabled && img.dataset.inView === "true" && !document.hidden;
    const src = play ? img.dataset.loop : img.dataset.still;
    if (img.getAttribute("src") !== src) img.src = src;
    img.closest("figure, .mod-card").classList.toggle("is-playing", play);
  }
}
motionToggles.forEach((toggle) =>
  toggle.addEventListener("click", () => {
    motionEnabled = !motionEnabled;
    updateLoops();
  }),
);
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
$$(".gameplay-loop").forEach((img) => loopObserver.observe(img));
updateLoops();
