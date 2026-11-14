// The dashboard's requests: the lobby's JSON under /ops, with the page token on every write and, on the loopback admin
// port, the admin token of the page's own address.
// Provenance: ORIGINAL (DeCombat operator tooling)

import {type Boot, basePath, parseBoot} from './boot.ts';

export class ApiError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
  /** The admin web's session ended (a lobby restart signs everyone out). */
  get signedOut(): boolean {
    return this.status === 401 && /sign in/i.test(this.message);
  }
}

export interface Api {
  boot: Boot;
  /** "/i/<name>" on a further instance, else "". */
  base: string;
  /** The loopback port's admin token from the address ("" on the admin web). */
  token: string;
  get<T>(path: string): Promise<T>;
  post<T>(path: string, body: unknown): Promise<T>;
  /** A link to a server page under /ops (the earlier dashboard, the beta list), keeping base and token. */
  page(path: string): string;
}

async function answer<T>(r: Response): Promise<T> {
  let body: unknown;
  try {
    body = await r.json();
  } catch {
    body = {message: 'the answer was not JSON'};
  }
  if (!r.ok) {
    const m = (body as {message?: unknown} | null)?.message;
    throw new ApiError(r.status, typeof m === 'string' && m ? m : `HTTP ${r.status}`);
  }
  return body as T;
}

export function createApi(doc: Document, loc: Location): Api {
  const boot = parseBoot(doc.body.getAttribute('data-boot'));
  const base = basePath(boot);
  const token = new URLSearchParams(loc.search).get('token') ?? '';
  const csrf = doc.querySelector('meta[name="csrf"]')?.getAttribute('content') ?? '';
  const auth: Record<string, string> = token ? {'X-Admin-Token': token} : {};
  return {
    boot,
    base,
    token,
    async get<T>(path: string) {
      return answer<T>(await fetch(base + path, {headers: auth, cache: 'no-store'}));
    },
    async post<T>(path: string, body: unknown) {
      return answer<T>(
        await fetch(base + path, {
          method: 'POST',
          headers: {...auth, 'Content-Type': 'application/json', 'X-Admin-CSRF': csrf},
          body: JSON.stringify(body),
          cache: 'no-store',
        }),
      );
    },
    page(path: string) {
      return base + path + (token ? `?token=${encodeURIComponent(token)}` : '');
    },
  };
}
