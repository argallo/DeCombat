// The frame every screen sits in: the top bar (which lobby, the switch between lobbies), the side navigation and the
// open screen. On a phone the navigation becomes a drawer (the shell component does that itself).
// Provenance: ORIGINAL (DeCombat operator tooling)

import {useEffect, useState} from 'react';
import type {Api} from './api.ts';
import {lobbyLabel} from './boot.ts';
import {type Route, type ScreenId, CLASSIC, NAV, navFor, parseHash, toHash} from './routes.ts';
import {NotBuilt} from './screens/NotBuilt.tsx';
import {BugReports} from './screens/BugReports.tsx';
import {Health} from './screens/Health.tsx';
import {Matches} from './screens/Matches.tsx';
import {ClassicIcon, SCREEN_ICON} from './viz/icons.tsx';
import {Overview} from './screens/Overview.tsx';
import {Players} from './screens/Players.tsx';
import {
  AppShell,
  Banner,
  EmptyState,
  Selector,
  SideNav,
  SideNavItem,
  SideNavSection,
  TopNav,
  TopNavHeading,
} from './ui/index.ts';

/** The value of the switcher's entry for the lobby that serves the pages (instance names never start with "@"). */
const OWN = '@own';

function useRoute(): Route {
  const [route, setRoute] = useState(() => parseHash(location.hash));
  useEffect(() => {
    const on = () => setRoute(parseHash(location.hash));
    addEventListener('hashchange', on);
    return () => removeEventListener('hashchange', on);
  }, []);
  return route;
}

function screenLabel(id: ScreenId): string {
  return NAV.flatMap(g => g.items).find(i => i.id === id)?.label ?? id;
}

/** The dashboard page's own path without the instance prefix ("/ops"). */
function pagePath(base: string): string {
  const p = location.pathname.replace(/\/+$/, '');
  return (base && p.startsWith(base) ? p.slice(base.length) : p) || '/ops';
}

function LobbySwitch({api, route}: {api: Api; route: Route}) {
  const {boot} = api;
  if (boot.instances.length === 0) return null;
  const options = [
    {value: OWN, label: boot.label || 'This lobby'},
    ...boot.instances.map(i => ({value: i.name, label: i.label})),
  ];
  return (
    <Selector
      label="Lobby"
      isLabelHidden
      size="sm"
      variant="secondary"
      options={options}
      value={boot.current ?? OWN}
      onChange={value => {
        const target = value === OWN ? '' : `/i/${value}`;
        if (target === api.base) return;
        // the open screen is kept, not a match or a player: those belong to one lobby
        const hash = route.screen ? toHash(route.screen) : '';
        location.assign(target + pagePath(api.base) + location.search + hash);
      }}
    />
  );
}

function Screen({api, route}: {api: Api; route: Route}) {
  if (route.screen === null) {
    return (
      <EmptyState
        title="There is no such screen"
        description="The address names a screen this dashboard does not have."
        headingLevel={1}
      />
    );
  }
  if (route.screen === 'overview') return <Overview api={api} />;
  if (route.screen === 'health') return <Health api={api} />;
  if (route.screen === 'bugs') return <BugReports api={api} />;
  if (route.screen === 'matches') return <Matches api={api} id={route.arg} key={route.arg} />;
  if (route.screen === 'players') return <Players api={api} name={route.arg} key={route.arg} />;
  return <NotBuilt api={api} label={screenLabel(route.screen)} route={route} />;
}

export function App({api}: {api: Api}) {
  const route = useRoute();
  const {boot} = api;
  const lobby = lobbyLabel(boot);
  const onInstance = boot.current !== null;
  const hasSwitch = boot.instances.length > 0;

  useEffect(() => {
    const parts = [route.screen ? screenLabel(route.screen) : 'Not found', lobby, 'DeCombat ops'];
    document.title = parts.filter(Boolean).join(' | ');
  }, [route.screen, lobby]);

  return (
    <AppShell
      contentPadding={4}
      banner={
        // a further instance's pages say so across the top, so a ban or a bot change is not made on the wrong lobby
        onInstance ? (
          <Banner
            status="warning"
            container="section"
            collapsible={false}
            title={`You are on ${lobby}, not on ${boot.label || 'the lobby that serves this page'}.`}
          />
        ) : undefined
      }
      topNav={
        <TopNav
          label="Lobby"
          heading={
            // with a switcher beside it, the switcher names the lobby
            <TopNavHeading heading="DeCombat" subheading={(hasSwitch ? '' : lobby) || 'operations'} />
          }
          endContent={<LobbySwitch api={api} route={route} />}
        />
      }
      sideNav={
        <SideNav>
          {navFor(onInstance, boot.web).map(group => (
            <SideNavSection key={group.title} title={group.title}>
              {group.items.map(item => (
                <SideNavItem
                  key={item.id}
                  label={item.label}
                  href={toHash(item.id)}
                  icon={SCREEN_ICON[item.id]}
                  isSelected={route.screen === item.id}
                />
              ))}
            </SideNavSection>
          ))}
          {/* LG-444: the earlier page stays one tap away until every screen has moved */}
          <SideNavSection title="Earlier version">
            <SideNavItem label="Classic dashboard" href={api.page(CLASSIC)} icon={ClassicIcon} />
          </SideNavSection>
        </SideNav>
      }>
      <Screen api={api} route={route} />
    </AppShell>
  );
}
