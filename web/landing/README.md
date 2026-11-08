# DeCombat website

The Firebase-hosted landing page for **decombat.com**. Static HTML, CSS and JavaScript; no framework, package install, or analytics. The beta signup form uses the existing lobby endpoint at `https://beta.decombat.com/beta/signup`.

Live custom domain: https://decombat.com

Firebase URL: https://decombat.web.app

## Work locally

Requires Node.js 22+ and the Firebase CLI for publishing.

```sh
cd web/landing
npm test
npm run build
npm run preview
```

Open http://127.0.0.1:4173. `dist/` is generated and ignored. The preview server supports video range requests. The rest of the game is not built by this website workflow.

## Publish

```sh
firebase login
npm run deploy
```

The project and Hosting site are both `decombat`. `.firebaserc` is scoped to this folder. `firebase.json` serves only the generated `dist/` folder; source, tooling and the ROM never enter the deployment.

Firebase has custom domains registered for `decombat.com` and `www.decombat.com`; the latter redirects to the apex. Cloudflare DNS records have been configured with DNS-only A/CNAME records and Firebase ownership/certificate TXT records. The custom domain now serves successfully over HTTPS. The Firebase URL remains available.

## Beta signup

The homepage includes the beta signup form from `../beta/section.html`. Its JavaScript and styles remain in `../beta/` and are copied into `dist/` by the build. Keep both website directories when copying this site. Firebase CSP explicitly permits the beta origin for fetch and the non-JavaScript form fallback. The existing email, platform, explicit consent and confirmation flow are preserved; see `../../docs/BETA.md` for server configuration.

## Privacy page

`privacy.html` is the plain-language privacy page (served at `/privacy`); the build copies it, the footer and the beta form's consent line link to it, and it needs no script or inline style, so the Firebase CSP serves it as it is. Its facts come from `docs/legal/PRIVACY.md` and DC_DEFAULTS (LG-360..362, LG-370..379, MON-2); when a retention number or a provider changes there, change the page and its "Last updated" date.

## Community content

- **Discord:** https://discord.gg/hNWcXpTST is configured in `site.config.json` and deployed. It never expires, has unlimited uses, and lands in `#welcome`. The DeCombat server uses the existing logo and includes welcome, announcements, release-notes, general, clips-and-highlights, find-a-squad, support, mods, mapkit, camo-showcase, community-vote, and two voice channels. To change the invite, edit `discordInvite` and redeploy.
- **Launch trailer:** `site.config.json` points to `https://youtu.be/tocDTVUPeOM`. The main player keeps a fixed 16:9 frame, with compact chapter links below it, and uses the official locally hosted thumbnail and creates a privacy-enhanced YouTube iframe only after a click. Chapter shortcuts start at 00:32 (modes), 00:51 (maps), and 01:04 (mods). An always-visible YouTube link and generated `/trailer/` redirect provide fallbacks. Playback controls initialize from the build-validated video ID before the asynchronous site-config request, so an early click stays on the site. Only validated YouTube IDs can be embedded; Firebase CSP permits the privacy-enhanced frame origin.
- **Mod showcase:** edit `data/catalog.json`. Four media cards: Galaxy G36E + inspect, Toxic L96A1 + inspect, Custom Crosshair settings, and a real Hacking Server gameplay excerpt. The two camo cards use the exact supplied GIF files; the inspect pack is linked from their details instead of repeated as a separate card. There are no synthetic crosshair or wallhack overlays. Catalog versions match repository manifests. Verified entries still come from the general community directory; other source packs are labeled “Built pack,” without implying directory verification or universal server availability. Inspect 0.1.0 and both featured maps have owner-approved rows in `community/vanilla_plus.json`; camo packs are not currently in that allowlist.
- **Verified mods:** the build reads `../../community/community.json`, the game's maintained directory. An entry needs an ID, version, a 64-character SHA-256 and a real HTTPS source repository. Placeholder source URLs do not earn verified status. Verification applies to that exact version/hash only. Multiple versions can coexist.
- **Server directory:** placeholder names, documentation IP ranges, private IPv4 addresses and example domains are filtered out of the published client directory. The landing page no longer has a server-list panel.
- **Client directory:** the sanitized directory is published at both `/community.json` and `/data/community.json`. The legacy tracked `web/landing/community.json` and `community.sample.js` remain unchanged for existing repository parity tests; they are not copied into the production bundle.
- **Field Intel:** removed at the owner’s request, including the release feed, development dispatch and server-list panels. The landing page no longer calls the GitHub Releases API. The release snapshot utility and pure parsing helpers are retained for a possible future updates page.

Source links use the configured GitHub repository. Access follows its visibility settings. This website does not publish game binaries.

## Continuous deployment

`.github/workflows/website.yml` runs tests and creates the static artifact when website files or the community directory change, and on release events. To enable its deployment job, configure a Firebase Hosting service-account credential as the repository secret `FIREBASE_SERVICE_ACCOUNT_DECOMBAT`, then set the repository variable `DC_WEBSITE_DEPLOY_ENABLED=true`. Configure the `website` GitHub Environment to match your preferred deployment approval policy. Deployment remains opt-in; no service-account credential is included in the source.

After enabling it, approved directory changes are reflected when the source is pushed. There is no recurring job or subscription.

## Sources and media

Product claims were checked against the project's README, HOSTING, MODDING_GUIDE, MACOS, IPAD, MAPKIT_PLAN, RELEASES, examples/mods, current community directory and the project's platform notes. MapKit is identified as in development. Apple Silicon is playable; Intel hardware validation is pending; iPadOS is experimental; Linux is listed as dedicated-server support.

- `assets/logo.webp` and `assets/mapkit.webp`: optimized copies of the existing project logos, kept unchanged in design.
- `assets/hero.webp`: original promotional concept artwork made with the built-in image generation tool. It is labeled concept art on the page, not presented as a game screenshot.
- `assets/social.jpg`: optimized social-sharing crop of the hero artwork.
- `assets/gameplay.mp4`: a silent 24-second excerpt from the supplied Snow Valley gameplay recording, starting at 00:30; H.264, 1280×720, 30 fps. It loads only when played.
- `assets/snow-valley.webp`, `junk-flea.webp`, `one-man-army.webp`, `close-quarters.webp`: original still frames from the user-supplied recordings. These earlier captures are retained as source media; the launch trailer replaces the Snow Valley video on the page.
- Three six-second gameplay loops replace the gallery stills. The page uses animated WebP at 640×360, 18 fps, totaling about 7.8 MiB instead of roughly 30 MiB of GIFs. The original GIF exports are supplied separately in `DeCombat-gameplay-GIFs.zip`. No new artwork is substituted for gameplay.
- Loop sources: Junk Flea sniper action: `2026-10-31 21-09-47.mp4`, 02:39–02:45; One Man Army streak: `2026-10-31 21-35-38.mp4`, 01:41–01:47; close-quarters automatic fire: `2026-10-31 22-00-58.mp4`, 00:21–00:27.
- Loops load when near the viewport, switch to static posters offscreen or when the tab is hidden, honor reduced-motion preferences, and provide a shared play/pause button.
- The mission and ready-up sections are merged into one three-step graphic. Game modes, lobby features, hosting, modding, and the MapKit workflow use inline SVG icons and compact diagrams. Body copy was reduced by about 34% (1,306 to 856 whitespace-delimited words in source HTML, excluding document head, SVG and scripts).
- Shipping Container and Freight Hall use uncropped screenshots at their native aspect ratios, with full screenshots linked from each. MapKit is a compact logo/text/link row; it does not repeat a map photo. The three Vanilla++ extras sit in a compact row within the existing classic-mode panel.
- Owner-supplied `gun1.gif` and `gun2.gif` are copied unchanged to `galaxy-g36e.gif` (2.94 seconds) and `toxic-l96a1.gif` (7.47 seconds), at 720×405. Static posters are used only when motion is paused, offscreen, or reduced motion is requested; they are not separate cards.
- `hacking-server-loop.webp`: real footage from the supplied `Hacking Server.mp4`, 00:28–00:32, showing a double kill with highlighted players. 720×405, 18 fps, around 2.5 MiB; `hacking-server.webp` is its static poster at 00:30.
- `custom-crosshair.webp`, `shipping-container-editor.webp`, and `freight-hall-editor.webp` are 1920px optimized copies of the owner’s attached screenshots; no scene elements were generated or replaced. These are observed screen captures, not files extracted from the ROM or mod-package assets.
- `launch-trailer.webp` is the official thumbnail from the owner’s YouTube trailer.
- Vanilla++ callouts follow DC_DEFAULTS LG-305, DSV-481 (Gun Game), LG-307/DSV-483 (Possession), and LG-306/DSV-482 (Hardcore, fivefold damage). Hardcore is described as a room setting. Map approval comes from `community/vanilla_plus.json`.
- Local fonts: Barlow Condensed under OFL and the repository's Roboto under its included license. No runtime Google Fonts request.

### Hero generation prompt

Built-in image generation, no API key or CLI fallback:

> Use case: stylized-concept. Asset type: ultra-wide website hero background for DeCombat, an independent emulator for a 2009 military multiplayer FPS. Create polished cinematic game key art with authentic late-2000s PC military shooter atmosphere, updated with premium art direction. Scene: abandoned snow-dusted industrial military outpost at dusk, cold overcast distant mountains, corrugated metal buildings, concrete barriers, overhead wires, small distant amber light and windblown dust. One anonymous helmeted soldier in olive tactical fatigues, face obscured, holding rifle low and facing away, positioned in the RIGHT third, cropped mid-thigh. Strong silhouette, credible period gear, no sci-fi. Leave the LEFT HALF dark, low-contrast and mostly open for large white website typography, but with atmospheric environmental depth. Mood: a familiar battlefield waiting for the squad to return, nostalgic, gritty, quiet confidence. Palette: near-black charcoal, muted olive, desaturated blue gray, warm amber edge lighting. Wide 16:9 composition, detailed painterly realistic 3D game promotional artwork, light analog grain, subtle haze, beautifully layered. No text, no logos, no UI, no letters, no watermark, no gore, no gratuitous explosions.

## Verification

`npm test` checks verification/version boundaries, placeholder filtering, safe link handling, Discord invites, release eligibility, trailer URL boundaries, showcase assets/manifest versions, release-pack sanitising (`data/packs.json`, written by `node scripts/sync-packs.mjs` from the latest release of each `packRepositories` entry of `site.config.json`; a listed pack is never Verified by itself), and coherent asset versioning. Browser checks cover 1440px desktop and 390px mobile layouts, search, category filters, mod dialogs and mobile navigation. The build validates configured public URLs. All rendered external data is created using text nodes; no release or mod data is inserted as HTML.

Firebase's custom-domain guide: https://firebase.google.com/docs/hosting/custom-domain

## Cache coherence

The build generates content-versioned names for the stylesheet, application module, shared module, and beta JS/CSS. Every page and the app's module import point to the same build version. This prevents an old cached renderer or stylesheet from being mixed with new HTML/catalog data. Unversioned compatibility files are still published, with revalidation headers for all JS/CSS. The regression test changes CSS and verifies that both the page references and transitive module references move to a new version.
