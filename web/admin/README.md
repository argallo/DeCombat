# web/admin: the operator dashboard's front end

The lobby's operator dashboard (docs/ADMIN_WEB.md, docs/OPS.md section 15) as a React app on the
[Astryx](https://github.com/meta/astryx) design library. Rules: DC_DEFAULTS.md LG-441, LG-442.

The servers build with cargo alone, so the **built** files are committed and the lobby embeds them:

```
web/admin/src/...                                  the sources (TypeScript, React)
crates/dc-lobby-server/src/admin_app/shell.html    the page (hand-written, no inline script)
crates/dc-lobby-server/src/admin_app/app.js        built: the script
crates/dc-lobby-server/src/admin_app/app.css       built: the stylesheet
crates/dc-lobby-server/src/admin_app/THIRD_PARTY_NOTICES.txt   built: the licences of what is bundled
```

## Changing it

You need Node.js of the version in `.nvmrc`. From this folder:

```
npm ci --ignore-scripts     # the exact packages of package-lock.json; no install script runs
npm run typecheck
npm test                    # node --test on src/**/*.test.ts
npm run build               # writes the three built files into the crate
```

Then commit the source change **together with** the rebuilt files. `python tools/admin_web_check.py` (from the
repository root; CI runs it as the job `admin-web`) does all of the above and fails when the committed files are not
what the sources build, when they are over the size budget, or when they break a rule below.
`DC_ADMIN_REPORT=1 npm run build` prints what the script is made of.

To look at it: start a lobby (`tools/server_start.ps1`, docs/SERVER.md) and open
`http://127.0.0.1:<admin port>/ops`. The page reads the lobby's own JSON; there is no separate dev server.

## Rules

- **Nothing from another site.** No CDN, no web font, no analytics. The theme's `fonts.css` imports a font from another
  site and is deliberately not imported (`src/main.tsx`); the system font is used. The content security policy would
  block it anyway.
- **No inline script, no `eval`.** The page loads `/ops/app.js` and `/ops/app.css` and nothing else.
- **The server is the authority.** The app only draws what the lobby's JSON says and sends the same requests the
  earlier page did (`src/api.ts`): the page token on every write, the admin token on the loopback port, every path
  below the instance prefix. A check in the app is a convenience, never the guard.
- **Server text is text.** Never build HTML from an answer; React escapes what is put in as children.
- **Exact versions.** `package.json` pins every package without a range, and all `@astryxdesign/*` packages move
  together (a theme pins the core's version). The library is a 0.x release whose names still change: the app takes its
  components from `src/ui/index.ts` only, so an upgrade is made there. An upgrade is its own change, with a look at
  every screen; take a release that is at least a week old.
- **Charts** are the app's own (`src/viz/`): plain elements and one stylesheet, the arithmetic in `scale.ts` with tests. No chart
  library is bundled: these few forms do not need one. A colour for a series is checked for both themes
  before it is added (the design library's data colours are the same in light and dark and two of them are too dim on dark).
- **Licences.** The build lists every bundled package with its licence text. A package that ships no licence file needs
  its text in `licenses/<name with "/" as "__">.txt`, taken from the package's repository, and a new package needs its
  line in the repository's `NOTICE.md`; the build and the check refuse otherwise.
- **The budget** is in `tools/admin_web_check.py`. React and the library's shared code are most of it; look at the
  report before adding a component that pulls in a large one.
