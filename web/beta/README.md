# Beta signup section for decombat.com

Provenance: ORIGINAL.

The homepage source now lives in `../landing/`. Its `index.html` includes the markup from `section.html`; the landing build copies `beta.css` and `beta.js` from this directory into `web/landing/dist/`.

Edit behavior/styles here, and update both `section.html` and the integrated markup if form fields change. Run `npm test` and `npm run build` from `web/landing`, then `npm run deploy` to publish the site to Firebase.

The endpoint (`https://beta.decombat.com/beta/signup`) and the operator side are described in `../../docs/BETA.md`. Firebase's CSP allows this origin for signup fetches and form submission. The site requests consent before posting; the lobby handles confirmation email and invite eligibility.
