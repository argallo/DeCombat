// The changes the dashboard makes to an account (DC_DEFAULTS.md LG-451): what is sent for each, what the dialog asks
// before it is sent, and why a change is not offered. The words are the rule's. No drawing here,
// so every sentence is tested without a browser.
// Provenance: ORIGINAL (DeCombat operator tooling)

import {when} from './time.ts';

/** What a change needs to know of the account: GET /ops/player. */
export interface Target {
  account: number;
  login: string;
  nick: string;
  has_character: boolean;
  gm: boolean;
  online: boolean;
  where: string;
  disabled: boolean;
  has_email: boolean;
  email: string;
  mail_on: boolean;
  exp: number;
  gp: number;
  nx: number;
  ban: {until: number; permanent: boolean} | null;
  mute: {minutes_left: number} | null;
  /** The most of each that one change may give or take. */
  caps: {nx: number; gp: number; exp: number};
  max_mute_minutes: number;
}

/** What a dialog asks before a change is sent. */
export interface Ask {
  /** The change as a question, with its object. */
  title: string;
  /** What happens, one sentence each. */
  lines: string[];
  /** The confirming button: the verb and the object again. */
  button: string;
  /** The button is red. */
  danger: boolean;
  /** The word that must be typed first (a change that cannot be undone). */
  type?: string;
}

/** A change ready to send: POST /ops/player/<action> with `body`, after `ask` was confirmed. */
export interface Plan {
  action: string;
  body: Record<string, unknown>;
  ask: Ask;
}

/** A plan, or the sentence that says what the form still lacks. */
export type Planned = Plan | string;

export type Unit = 'nx' | 'gp' | 'exp';
export const UNITS: {value: Unit; label: string}[] = [
  {value: 'nx', label: 'NX'},
  {value: 'gp', label: 'GP'},
  {value: 'exp', label: 'EXP'},
];

/** The longest ban that is not permanent: ten years, as the lobby has it. */
export const MAX_BAN_HOURS = 24 * 3650;
/** The longest reason the lobby keeps. */
export const MAX_REASON = 200;

const num = (n: number) => n.toLocaleString('en-US');
const whole = (n: number | null | undefined) => (typeof n === 'number' && Number.isFinite(n) ? Math.floor(n) : 0);

/** A whole number as it is typed ("1,500", " 60 "): null for anything else, so a form never sends a guess. */
export function typedWhole(text: string): number | null {
  const t = text.replace(/[,\s]/g, '');
  return /^\d{1,12}$/.test(t) ? Number(t) : null;
}

/** The name a sentence calls the account by. */
export const who = (t: Pick<Target, 'nick' | 'login'>) => t.nick || t.login;

export function planGrant(
  t: Target,
  f: {unit: Unit; amount: number | null; take: boolean; reason: string},
): Planned {
  const n = whole(f.amount);
  const reason = f.reason.trim();
  const unit = f.unit.toUpperCase();
  if (!t.has_character) return `${who(t)} has no character yet: there is no balance to change.`;
  if (n <= 0) return 'Give an amount above 0.';
  const cap = t.caps[f.unit];
  if (n > cap) return `At most ${num(cap)} ${unit} in one change.`;
  const old = t[f.unit];
  if (f.take && n > old) return `They have only ${num(old)} ${unit}: a balance never goes below 0.`;
  if (!reason) return 'Give a reason.';
  const now = f.take ? old - n : old + n;
  return {
    action: 'grant',
    body: {account: t.account, [f.unit]: f.take ? -n : n, reason},
    ask: {
      title: f.take ? `Take ${num(n)} ${unit} from ${who(t)}?` : `Give ${num(n)} ${unit} to ${who(t)}?`,
      lines: [`Balance ${num(old)} → ${num(now)}.`, `Reason: ${reason}`],
      button: `${f.take ? 'Take' : 'Give'} ${num(n)} ${unit}`,
      danger: false,
    },
  };
}

export function planBan(
  t: Target,
  f: {length: number | null; unit: 'hours' | 'days'; permanent: boolean; reason: string},
  nowSecs: number,
): Planned {
  const n = whole(f.length);
  const reason = f.reason.trim();
  const hours = f.unit === 'days' ? n * 24 : n;
  if (!f.permanent && n <= 0) return 'Give the length of the ban, or make it permanent.';
  if (!f.permanent && hours > MAX_BAN_HOURS) return 'At most ten years: make it permanent instead.';
  if (!reason) return 'Give a reason for the ban.';
  const length = `${n} ${n === 1 ? f.unit.slice(0, -1) : f.unit}`;
  const lines = [
    f.permanent
      ? 'Every connection of the account drops now. Its logins are refused until you lift the ban.'
      : `Every connection of the account drops now. Its logins are refused until ${when(nowSecs + hours * 3600)}.`,
  ];
  if (t.online) lines.push(`They are online (${t.where}): they are dropped now.`);
  if (t.ban) lines.push('This replaces the ban they have now.');
  lines.push(`Reason: ${reason}`);
  return {
    action: 'ban',
    body: f.permanent ? {account: t.account, permanent: true, reason} : {account: t.account, [f.unit]: n, reason},
    ask: {
      title: f.permanent ? `Ban ${who(t)} permanently?` : `Ban ${who(t)} for ${length}?`,
      lines,
      button: f.permanent ? 'Ban permanently' : `Ban for ${length}`,
      danger: true,
    },
  };
}

export function planUnban(t: Target): Plan {
  return {
    action: 'unban',
    body: {account: t.account},
    ask: {
      title: `Lift the ban of ${who(t)}?`,
      lines: ['They can log in again at once.'],
      button: 'Lift the ban',
      danger: false,
    },
  };
}

export function planKick(t: Target, f: {reason: string}): Planned {
  const reason = f.reason.trim();
  if (!t.online) return `${who(t)} is not online.`;
  if (!reason) return 'Give a reason for the kick.';
  return {
    action: 'kick',
    body: {account: t.account, reason},
    ask: {
      title: `Kick ${who(t)} now?`,
      lines: ['Their connection drops. They can log in again right away.', `Reason: ${reason}`],
      button: 'Kick',
      danger: true,
    },
  };
}

export function planMute(t: Target, f: {minutes: number | null; reason: string}, nowSecs: number): Planned {
  const n = whole(f.minutes);
  const reason = f.reason.trim();
  if (!t.has_character) return `${who(t)} has no character yet: there is no chat to mute.`;
  if (n <= 0) return 'Give the minutes of the mute.';
  if (n > t.max_mute_minutes) return `At most ${num(t.max_mute_minutes)} minutes.`;
  const minutes = `${n} ${n === 1 ? 'minute' : 'minutes'}`;
  const lines = [`The server drops their chat and whispers until ${when(nowSecs + n * 60).slice(11, 16)}.`];
  if (t.mute) lines.push('This replaces the mute they have now.');
  if (reason) lines.push(`Reason: ${reason}`);
  return {
    action: 'mute',
    body: {account: t.account, minutes: n, ...(reason ? {reason} : {})},
    ask: {title: `Mute ${who(t)} for ${minutes}?`, lines, button: `Mute for ${minutes}`, danger: false},
  };
}

export function planDisable(t: Target): Plan {
  return {
    action: 'disable',
    body: {account: t.account},
    ask: {
      title: `Disable the login of ${who(t)}?`,
      lines: ['They cannot log in until you enable it. A session that is open stays open: kick them as well.'],
      button: 'Disable login',
      danger: true,
    },
  };
}

export function planResetMail(t: Target): Plan {
  return {
    action: 'reset_mail',
    body: {account: t.account},
    ask: {
      title: `Mail a password reset code to ${t.email}?`,
      lines: ['The code works once, for 15 minutes.'],
      button: 'Send the mail',
      danger: false,
    },
  };
}

export function planDelete(t: Target): Plan {
  return {
    action: 'delete',
    // the lobby compares what was typed with the login itself
    body: {account: t.account, confirm: t.login},
    ask: {
      title: `Delete account ${t.account} (${t.login}) for good?`,
      lines: [
        'The character, items, loadouts, inbox, friends and clan place go.',
        'The match records and events keep the account number.',
        'This cannot be undone.',
      ],
      button: 'Delete account',
      danger: true,
      type: t.login,
    },
  };
}

/** `unmute` and `enable` need no question: they are undone with one press and nobody notices them. */
export type Change = 'grant' | 'ban' | 'unban' | 'kick' | 'mute' | 'unmute' | 'disable' | 'enable' | 'reset_mail' | 'delete';

/** Why a change is not offered for this account right now; "" when it is. */
export function blocked(t: Target, change: Change): string {
  switch (change) {
    case 'grant':
      return t.has_character ? '' : 'No character yet: there is no balance to change.';
    case 'mute':
      return t.has_character ? '' : 'No character yet: there is no chat to mute.';
    case 'kick':
      return t.online ? '' : 'Not online.';
    case 'reset_mail':
      if (!t.mail_on) return 'Mail is not set up on this lobby, so no reset mail can go out.';
      return t.has_email ? '' : 'The account gave no recovery address.';
    case 'delete':
      if (t.gm) return 'A GM account cannot be deleted: take its GM flag away first.';
      return t.online ? 'They are online: kick them first.' : '';
    default:
      return '';
  }
}

/** Whether what was typed is the word a dialog asks for (the case of the letters does not matter). */
export function typedIt(typed: string, word: string): boolean {
  return typed.trim().toLowerCase() === word.toLowerCase();
}
