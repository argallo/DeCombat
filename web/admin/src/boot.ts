// What the server tells the page about itself: the shell's <body data-boot="..."> (JSON), its <meta name="csrf"> and,
// on the loopback admin port, the ?token= of the address. Nothing here is trusted for more than drawing: every
// request is checked by the server again.
// Provenance: ORIGINAL (DeCombat operator tooling)

/** A further lobby instance the admin web serves under /i/<name>. */
export interface Instance {
  name: string;
  label: string;
}

export interface Boot {
  /** True on the admin web (a signed-in session); false on the loopback admin port. */
  web: boolean;
  /** The name of the lobby that serves the page ("" when it has none). */
  label: string;
  /** The further instance these pages are for; null for the lobby that serves them. */
  current: string | null;
  instances: Instance[];
}

const NAME = /^[a-z0-9][a-z0-9-]{0,31}$/;

export const EMPTY_BOOT: Boot = {web: false, label: '', current: null, instances: []};

/** The boot data of the shell; anything malformed falls back to the plain single-lobby page. */
export function parseBoot(text: string | null | undefined): Boot {
  if (!text) return EMPTY_BOOT;
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return EMPTY_BOOT;
  }
  if (typeof raw !== 'object' || raw === null) return EMPTY_BOOT;
  const o = raw as Record<string, unknown>;
  const instances: Instance[] = [];
  if (Array.isArray(o.instances)) {
    for (const i of o.instances) {
      if (typeof i !== 'object' || i === null) continue;
      const {name, label} = i as Record<string, unknown>;
      if (typeof name === 'string' && NAME.test(name) && typeof label === 'string') {
        instances.push({name, label});
      }
    }
  }
  const current =
    typeof o.current === 'string' && instances.some(i => i.name === o.current) ? o.current : null;
  return {
    web: o.web === true,
    label: typeof o.label === 'string' ? o.label : '',
    current,
    instances,
  };
}

/** The path every request and link of the page goes below: "/i/<name>" on a further instance, else "". */
export function basePath(boot: Boot): string {
  return boot.current ? `/i/${boot.current}` : '';
}

/** The name shown for the lobby the pages are for. */
export function lobbyLabel(boot: Boot): string {
  if (boot.current) {
    return boot.instances.find(i => i.name === boot.current)?.label ?? boot.current;
  }
  return boot.label;
}
