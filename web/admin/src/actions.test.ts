// Provenance: ORIGINAL (DeCombat operator tooling)
import assert from 'node:assert/strict';
import {test} from 'node:test';
import {
  type Plan,
  type Planned,
  type Target,
  blocked,
  planBan,
  planDelete,
  planDisable,
  planGrant,
  planKick,
  planMute,
  planResetMail,
  planUnban,
  typedIt,
  typedWhole,
} from './actions.ts';
import {when} from './time.ts';

const T: Target = {
  account: 1002,
  login: 'river_login',
  nick: 'quietriver',
  has_character: true,
  gm: false,
  online: true,
  where: 'room 1-3, in a match',
  disabled: false,
  has_email: true,
  email: 'q***@e***.example',
  mail_on: true,
  exp: 184220,
  gp: 52310,
  nx: 4200,
  ban: null,
  mute: null,
  caps: {nx: 100000, gp: 1000000, exp: 1000000},
  max_mute_minutes: 1440,
};
const NOW = 1_790_000_000;

const plan = (p: Planned): Plan => {
  assert.equal(typeof p, 'object', String(p));
  return p as Plan;
};

test('give and take', () => {
  const give = plan(planGrant(T, {unit: 'nx', amount: 500, take: false, reason: ' event prize '}));
  assert.equal(give.action, 'grant');
  assert.deepEqual(give.body, {account: 1002, nx: 500, reason: 'event prize'});
  assert.deepEqual(give.ask, {
    title: 'Give 500 NX to quietriver?',
    lines: ['Balance 4,200 → 4,700.', 'Reason: event prize'],
    button: 'Give 500 NX',
    danger: false,
  });
  const take = plan(planGrant(T, {unit: 'gp', amount: 2310, take: true, reason: 'refund'}));
  assert.deepEqual(take.body, {account: 1002, gp: -2310, reason: 'refund'});
  assert.equal(take.ask.title, 'Take 2,310 GP from quietriver?');
  assert.equal(take.ask.lines[0], 'Balance 52,310 → 50,000.');
  assert.equal(take.ask.button, 'Take 2,310 GP');

  assert.equal(planGrant(T, {unit: 'nx', amount: null, take: false, reason: 'x'}), 'Give an amount above 0.');
  assert.equal(planGrant(T, {unit: 'nx', amount: 100001, take: false, reason: 'x'}), 'At most 100,000 NX in one change.');
  assert.equal(
    planGrant(T, {unit: 'nx', amount: 4201, take: true, reason: 'x'}),
    'They have only 4,200 NX: a balance never goes below 0.',
  );
  assert.equal(planGrant(T, {unit: 'nx', amount: 5, take: false, reason: '  '}), 'Give a reason.');
  assert.match(String(planGrant({...T, has_character: false}, {unit: 'nx', amount: 5, take: false, reason: 'x'})), /no character/);
});

test('ban and lifting it', () => {
  const days = plan(planBan(T, {length: 3, unit: 'days', permanent: false, reason: 'wallhack'}, NOW));
  assert.deepEqual(days.body, {account: 1002, days: 3, reason: 'wallhack'});
  assert.deepEqual(days.ask, {
    title: 'Ban quietriver for 3 days?',
    lines: [
      `Every connection of the account drops now. Its logins are refused until ${when(NOW + 3 * 86400)}.`,
      'They are online (room 1-3, in a match): they are dropped now.',
      'Reason: wallhack',
    ],
    button: 'Ban for 3 days',
    danger: true,
  });
  const hour = plan(planBan({...T, online: false}, {length: 1, unit: 'hours', permanent: false, reason: 'x'}, NOW));
  assert.equal(hour.ask.title, 'Ban quietriver for 1 hour?');
  assert.equal(hour.ask.lines.length, 2);
  const ever = plan(
    planBan({...T, online: false, ban: {until: 5, permanent: false}}, {length: null, unit: 'days', permanent: true, reason: 'x'}, NOW),
  );
  assert.deepEqual(ever.body, {account: 1002, permanent: true, reason: 'x'});
  assert.equal(ever.ask.title, 'Ban quietriver permanently?');
  assert.equal(ever.ask.button, 'Ban permanently');
  assert.deepEqual(ever.ask.lines, [
    'Every connection of the account drops now. Its logins are refused until you lift the ban.',
    'This replaces the ban they have now.',
    'Reason: x',
  ]);
  assert.match(String(planBan(T, {length: 0, unit: 'days', permanent: false, reason: 'x'}, NOW)), /length/);
  assert.match(String(planBan(T, {length: 3651, unit: 'days', permanent: false, reason: 'x'}, NOW)), /ten years/);
  assert.equal(typeof planBan(T, {length: 3650, unit: 'days', permanent: false, reason: 'x'}, NOW), 'object');
  assert.match(String(planBan(T, {length: 3, unit: 'days', permanent: false, reason: ''}, NOW)), /reason/);

  const lift = planUnban(T);
  assert.deepEqual(
    [lift.action, lift.body, lift.ask.title, lift.ask.button, lift.ask.danger],
    ['unban', {account: 1002}, 'Lift the ban of quietriver?', 'Lift the ban', false],
  );
});

test('kick, mute, login, reset mail', () => {
  const kick = plan(planKick(T, {reason: 'afk in ranked'}));
  assert.deepEqual(kick.body, {account: 1002, reason: 'afk in ranked'});
  assert.deepEqual(kick.ask, {
    title: 'Kick quietriver now?',
    lines: ['Their connection drops. They can log in again right away.', 'Reason: afk in ranked'],
    button: 'Kick',
    danger: true,
  });
  assert.equal(planKick({...T, online: false}, {reason: 'x'}), 'quietriver is not online.');
  assert.match(String(planKick(T, {reason: ''})), /reason/);

  const mute = plan(planMute(T, {minutes: 60, reason: ''}, NOW));
  assert.deepEqual(mute.body, {account: 1002, minutes: 60});
  assert.deepEqual(mute.ask, {
    title: 'Mute quietriver for 60 minutes?',
    lines: [`The server drops their chat and whispers until ${when(NOW + 3600).slice(11, 16)}.`],
    button: 'Mute for 60 minutes',
    danger: false,
  });
  assert.deepEqual(plan(planMute(T, {minutes: 1, reason: 'spam'}, NOW)).body, {account: 1002, minutes: 1, reason: 'spam'});
  assert.equal(plan(planMute(T, {minutes: 1, reason: ''}, NOW)).ask.title, 'Mute quietriver for 1 minute?');
  assert.equal(planMute(T, {minutes: 1441, reason: ''}, NOW), 'At most 1,440 minutes.');
  assert.match(String(planMute(T, {minutes: 0, reason: ''}, NOW)), /minutes/);

  const off = planDisable(T);
  assert.equal(off.ask.title, 'Disable the login of quietriver?');
  assert.ok(off.ask.danger);
  const mail = planResetMail(T);
  assert.equal(mail.ask.title, 'Mail a password reset code to q***@e***.example?');
  assert.equal(mail.ask.button, 'Send the mail');
});

test('delete asks for the login to be typed', () => {
  const del = planDelete({...T, online: false});
  assert.deepEqual(del.body, {account: 1002, confirm: 'river_login'});
  assert.equal(del.ask.title, 'Delete account 1002 (river_login) for good?');
  assert.equal(del.ask.type, 'river_login');
  assert.ok(del.ask.danger);
  assert.ok(typedIt(' River_Login ', 'river_login'));
  assert.ok(!typedIt('river', 'river_login') && !typedIt('', 'river_login'));
});

test('why a change is not offered', () => {
  assert.equal(blocked(T, 'grant'), '');
  assert.equal(blocked(T, 'kick'), '');
  assert.equal(blocked({...T, online: false}, 'kick'), 'Not online.');
  assert.match(blocked({...T, has_character: false}, 'grant'), /No character/);
  assert.match(blocked({...T, has_character: false}, 'mute'), /No character/);
  assert.match(blocked({...T, mail_on: false}, 'reset_mail'), /not set up/);
  assert.match(blocked({...T, has_email: false}, 'reset_mail'), /no recovery address/);
  assert.match(blocked(T, 'delete'), /online/);
  assert.match(blocked({...T, gm: true, online: false}, 'delete'), /GM/);
  assert.equal(blocked({...T, online: false}, 'delete'), '');
  assert.equal(blocked(T, 'ban'), '');
});

test('a number as it is typed', () => {
  assert.equal(typedWhole('500'), 500);
  assert.equal(typedWhole(' 1,500 '), 1500);
  assert.equal(typedWhole(''), null);
  assert.equal(typedWhole('-5'), null);
  assert.equal(typedWhole('1.5'), null);
  assert.equal(typedWhole('5e3'), null);
  assert.equal(typedWhole('abc'), null);
});

test('without a character the name is the login', () => {
  assert.equal(planDisable({...T, nick: ''}).ask.title, 'Disable the login of river_login?');
});
