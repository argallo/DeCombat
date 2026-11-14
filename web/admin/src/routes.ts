// The screens and their place in the address. Routes live in the hash, so they work below /i/<name> and with the
// loopback port's ?token= and need no route on the server. The hashes of the page before this one keep working.
// Provenance: ORIGINAL (DeCombat operator tooling)

export type ScreenId =
  | 'overview'
  | 'matches'
  | 'health'
  | 'bugs'
  | 'players'
  | 'beta'
  | 'bots'
  | 'deploy'
  | 'packs'
  | 'release'
  | 'keys';

export interface Route {
  /** null: the address names no screen. */
  screen: ScreenId | null;
  /** What the screen shows (a match id, a player name), "" for its front page. */
  arg: string;
}

export interface NavItem {
  id: ScreenId;
  label: string;
  /** For the whole box: not offered on a further lobby instance's pages. */
  boxWide?: boolean;
  /** Needs the admin web's session (the loopback port has no keys to manage). */
  webOnly?: boolean;
}

export interface NavGroup {
  title: string;
  items: NavItem[];
}

export const NAV: NavGroup[] = [
  {
    title: 'Monitor',
    items: [
      {id: 'overview', label: 'Overview'},
      {id: 'matches', label: 'Matches'},
      {id: 'health', label: 'Health'},
      {id: 'bugs', label: 'Bug reports', boxWide: true},
    ],
  },
  {
    title: 'Players',
    items: [
      {id: 'players', label: 'Players'},
      {id: 'beta', label: 'Beta list'},
    ],
  },
  {
    title: 'Operate',
    items: [
      {id: 'bots', label: 'Bots'},
      {id: 'deploy', label: 'Deploy', boxWide: true},
      {id: 'packs', label: 'Content packs', boxWide: true},
      {id: 'release', label: 'Public release', boxWide: true},
    ],
  },
  {
    title: 'Access',
    items: [{id: 'keys', label: 'Security keys', boxWide: true, webOnly: true}],
  },
];

const SCREENS = new Set<string>(NAV.flatMap(g => g.items.map(i => i.id)));

/** The hashes of the earlier dashboard page: view -> screen. */
const LEGACY: Record<string, ScreenId> = {
  live: 'overview',
  problems: 'health',
  alerts: 'health',
  matches: 'matches',
  match: 'matches',
  timeline: 'players',
  bots: 'bots',
  updates: 'deploy',
};

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

/** `#/matches/12`, `#/players/Some%20Name`, and the earlier `#match=12`, `#timeline=Name`, `#live`. */
export function parseHash(hash: string): Route {
  const h = hash.replace(/^#/, '');
  if (h === '' || h === '/') return {screen: 'overview', arg: ''};
  if (h.startsWith('/')) {
    const [screen = '', ...rest] = h.slice(1).split('/');
    if (!SCREENS.has(screen)) return {screen: null, arg: ''};
    return {screen: screen as ScreenId, arg: decode(rest.join('/'))};
  }
  const eq = h.indexOf('=');
  const view = eq < 0 ? h : h.slice(0, eq);
  const screen = LEGACY[decode(view)];
  if (!screen) return {screen: null, arg: ''};
  return {screen, arg: eq < 0 ? '' : decode(h.slice(eq + 1))};
}

export function toHash(screen: ScreenId, arg = ''): string {
  return arg ? `#/${screen}/${encodeURIComponent(arg)}` : `#/${screen}`;
}

/** The nav of a page: a further instance's pages leave out what is for the whole box, the loopback port the keys. */
export function navFor(onInstance: boolean, web: boolean): NavGroup[] {
  return NAV.map(g => ({
    title: g.title,
    items: g.items.filter(i => !(onInstance && i.boxWide) && !(i.webOnly && !web)),
  })).filter(g => g.items.length > 0);
}

/** The address of the earlier dashboard page (LG-444), below the instance prefix like every page. */
export const CLASSIC = '/ops/classic';

/**
 * Where the earlier pages show what `route` names: a page below /ops and its hash. Screens that have not moved into
 * this app yet send the operator there.
 */
export function classicFor(route: Route): {path: string; hash: string} {
  const arg = route.arg ? '=' + encodeURIComponent(route.arg) : '';
  switch (route.screen) {
    case 'matches':
      return {path: CLASSIC, hash: route.arg ? '#match' + arg : '#matches'};
    case 'players':
      return {path: CLASSIC, hash: '#timeline' + arg};
    case 'health':
      return {path: CLASSIC, hash: '#problems'};
    case 'bots':
      return {path: CLASSIC, hash: '#bots'};
    case 'bugs':
    case 'deploy':
    case 'packs':
    case 'release':
      return {path: CLASSIC, hash: '#updates'};
    case 'beta':
      return {path: '/ops/beta', hash: ''};
    default:
      return {path: CLASSIC, hash: '#live'};
  }
}
