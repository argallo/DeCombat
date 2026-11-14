// The entry of the operator dashboard. The first three stylesheets are the design library's reset, its components and the
// theme's colours, in that order (the order is their cascade). The theme's own font file is deliberately not imported:
// it loads a web font from another site, which the dashboard's content security policy forbids; the system font is used.
// The fourth is the dashboard's own: its charts.
// Provenance: ORIGINAL (DeCombat operator tooling)

import '@astryxdesign/core/reset.css';
import '@astryxdesign/core/astryx.css';
import '@astryxdesign/theme-neutral/theme.css';
import './viz/viz.css';

import {StrictMode} from 'react';
import {createRoot} from 'react-dom/client';
import {createApi} from './api.ts';
import {App} from './App.tsx';
import {Theme, theme} from './ui/index.ts';

const root = document.getElementById('app');
if (root) {
  const api = createApi(document, location);
  createRoot(root).render(
    <StrictMode>
      <Theme theme={theme} mode="system">
        <App api={api} />
      </Theme>
    </StrictMode>,
  );
}
