// The build of the operator dashboard: one script and one stylesheet with fixed names, written into the lobby crate,
// which embeds them (the servers build with cargo alone, so what this writes is committed). No source map, no code
// splitting, nothing loaded from another site. Beside them the licence texts of every package in the bundle.
// Provenance: ORIGINAL (DeCombat operator tooling)

import {existsSync, readdirSync, readFileSync} from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {type Plugin, defineConfig} from 'vite';

const here = path.dirname(fileURLToPath(import.meta.url));
const OUT = path.resolve(here, '../../crates/dc-lobby-server/src/admin_app');
const NOTICES = 'THIRD_PARTY_NOTICES.txt';

/** The package folder a bundled module comes from (`.../node_modules/<name>` or `.../node_modules/@scope/<name>`). */
function packageDir(id: string): string | null {
  const clean = id.replace(/\\/g, '/').replace(/^\0+/, '').split('?')[0] ?? '';
  const at = clean.lastIndexOf('/node_modules/');
  if (at < 0) return null;
  const parts = clean.slice(at + '/node_modules/'.length).split('/');
  const name = parts[0]?.startsWith('@') ? parts.slice(0, 2).join('/') : parts[0];
  return name ? `${clean.slice(0, at)}/node_modules/${name}` : null;
}

const lf = (s: string) => s.replace(/\r\n?/g, '\n').replace(/[ \t]+$/gm, '').trim();

/** Writes the name, version, licence and licence text of every package whose code is in the bundle. */
function thirdPartyNotices(): Plugin {
  return {
    name: 'third-party-notices',
    generateBundle(_options, bundle) {
      const dirs = new Set<string>();
      // DC_ADMIN_REPORT=1: what the script is made of, by package (the design library by component), before minifying
      const sizes = new Map<string, number>();
      for (const chunk of Object.values(bundle)) {
        if (chunk.type !== 'chunk') continue;
        for (const id of [...Object.keys(chunk.modules ?? {}), ...(chunk.moduleIds ?? [])]) {
          const dir = packageDir(id);
          if (dir) dirs.add(dir);
        }
        for (const [id, module] of Object.entries(chunk.modules ?? {})) {
          const dir = packageDir(id);
          let key = dir ? dir.slice(dir.lastIndexOf('/node_modules/') + '/node_modules/'.length) : '(the app)';
          if (dir && key === '@astryxdesign/core') {
            key += `/${id.replace(/\\/g, '/').slice(dir.length).split('/')[2] ?? ''}`;
          }
          sizes.set(key, (sizes.get(key) ?? 0) + module.renderedLength);
        }
      }
      if (process.env.DC_ADMIN_REPORT) {
        const rows = [...sizes].sort((a, b) => b[1] - a[1]);
        const total = rows.reduce((sum, row) => sum + row[1], 0);
        for (const [key, bytes] of rows.slice(0, 50)) {
          this.info(`${String(bytes).padStart(8)} ${((100 * bytes) / total).toFixed(1).padStart(5)}%  ${key}`);
        }
        this.info(`${String(total).padStart(8)} 100.0%  in all`);
      }
      const entries: {name: string; text: string}[] = [];
      const missing: string[] = [];
      for (const dir of dirs) {
        const pkg = JSON.parse(readFileSync(`${dir}/package.json`, 'utf8')) as {
          name: string;
          version: string;
          license?: string;
        };
        // a package that ships no licence file has its text in licenses/<name with "/" as "__">.txt (taken from
        // the package's repository by hand)
        const shipped = readdirSync(dir)
          .filter(f => /^(licen[cs]e|copying)(\.|$)/i.test(f))
          .sort()[0];
        const byHand = path.join(here, 'licenses', `${pkg.name.replace('/', '__')}.txt`);
        const file = shipped ? `${dir}/${shipped}` : existsSync(byHand) ? byHand : null;
        if (!file) {
          missing.push(`${pkg.name} (licenses/${path.basename(byHand)})`);
          continue;
        }
        entries.push({
          name: pkg.name,
          text: `${pkg.name} ${pkg.version}\nLicence: ${pkg.license ?? 'see below'}\n\n${lf(readFileSync(file, 'utf8'))}\n`,
        });
      }
      if (missing.length > 0) {
        this.error(`no licence file shipped and none kept by hand for: ${missing.sort().join(', ')}`);
      }
      entries.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
      const rule = '-'.repeat(78);
      const head =
        'The operator dashboard of dc-lobby-server (app.js, app.css) contains code of the packages below.\n' +
        'This file is written by the build in web/admin; do not edit it by hand.\n';
      this.emitFile({
        type: 'asset',
        fileName: NOTICES,
        source: `${head}\n${entries.map(e => `${rule}\n${e.text}`).join('\n')}`,
      });
    },
  };
}

export default defineConfig({
  plugins: [thirdPartyNotices()],
  build: {
    outDir: OUT,
    // the folder also holds the hand-written shell.html
    emptyOutDir: false,
    sourcemap: false,
    cssCodeSplit: false,
    modulePreload: false,
    copyPublicDir: false,
    target: 'es2022',
    // one file each by design; the size budget is checked by tools/admin_web_check.py
    chunkSizeWarningLimit: 4096,
    rollupOptions: {
      input: path.resolve(here, 'src/main.tsx'),
      output: {
        format: 'es',
        entryFileNames: 'app.js',
        assetFileNames: 'app[extname]',
        codeSplitting: false,
      },
    },
  },
});
