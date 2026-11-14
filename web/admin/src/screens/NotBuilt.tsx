// A screen that is still served by the earlier dashboard page.
// Provenance: ORIGINAL (DeCombat operator tooling)

import type {Api} from '../api.ts';
import {type Route, classicFor} from '../routes.ts';
import {Button, EmptyState} from '../ui/index.ts';

export function NotBuilt({api, label, route}: {api: Api; label: string; route: Route}) {
  // the key page is the admin web's own, outside /ops and outside any instance
  const to = classicFor(route);
  const href = route.screen === 'keys' ? '/keys' : api.page(to.path) + to.hash;
  const onClassic = to.path.endsWith('/classic') && route.screen !== 'keys';
  return (
    <EmptyState
      title={`${label} has not moved here yet`}
      description="The new dashboard is arriving screen by screen. Until this one does, it is on the classic dashboard, where everything works as before."
      headingLevel={1}
      actions={
        <Button
          label={onClassic ? `Open ${label} on the classic dashboard` : `Open ${label}`}
          variant="primary"
          href={href}
        />
      }
    />
  );
}
