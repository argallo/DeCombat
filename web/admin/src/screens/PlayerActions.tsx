// The changes to one account on a person's page (DC_DEFAULTS.md LG-451): a short row of buttons, each opening its
// own small form and then the question of confirm.tsx. The writes are the lobby's own (POST /ops/player/<action>),
// unchanged. The loadout is still given on the classic page.
// Provenance: ORIGINAL (DeCombat operator tooling)

import {useState} from 'react';
import {
  type Change,
  type Plan,
  type Target,
  type Unit,
  MAX_REASON,
  UNITS,
  blocked,
  planBan,
  planDelete,
  planDisable,
  planGrant,
  planKick,
  planMute,
  planResetMail,
  planUnban,
  typedWhole,
  who,
} from '../actions.ts';
import type {Api} from '../api.ts';
import {lobbyLabel} from '../boot.ts';
import {Flow} from '../confirm.tsx';
import {classicFor} from '../routes.ts';
import {
  Button,
  CheckboxInput,
  HStack,
  Selector,
  Text,
  TextInput,
  VStack,
  useToast,
} from '../ui/index.ts';

interface Common {
  t: Target;
  lobby: string | null;
  send: (plan: Plan) => Promise<void>;
  onClose: () => void;
}

function Reason({value, onChange, optional}: {value: string; onChange: (v: string) => void; optional?: boolean}) {
  return (
    <TextInput
      label="Reason"
      description="Kept in the account's events with who made the change."
      value={value}
      onChange={v => onChange(v.slice(0, MAX_REASON))}
      isRequired={!optional}
      isOptional={optional}
      autoComplete="off"
    />
  );
}

/** A whole number, kept as it is typed: what the form sends is always what the field shows. */
function Whole({label, value, onChange, width, description, isDisabled, isRequired}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  width: number;
  description?: string;
  isDisabled?: boolean;
  isRequired?: boolean;
}) {
  return (
    <TextInput
      label={label}
      description={description}
      value={value}
      onChange={onChange}
      inputMode="numeric"
      autoComplete="off"
      width={width}
      isDisabled={isDisabled}
      isRequired={isRequired}
      status={value.trim() && typedWhole(value) === null ? {type: 'error', message: 'A whole number.'} : undefined}
    />
  );
}

function GrantFlow({t, ...rest}: Common) {
  const [way, setWay] = useState('give');
  const [unit, setUnit] = useState<Unit>('nx');
  const [amount, setAmount] = useState('');
  const [reason, setReason] = useState('');
  const name = unit.toUpperCase();
  return (
    <Flow
      title={`Give or take: ${who(t)}`}
      plan={() => planGrant(t, {unit, amount: typedWhole(amount), take: way === 'take', reason})}
      {...rest}>
      <HStack gap={2} align="end" wrap="wrap">
        <Selector
          label="Give or take"
          options={[
            {value: 'give', label: 'Give'},
            {value: 'take', label: 'Take'},
          ]}
          value={way}
          onChange={setWay}
          width={120}
        />
        <Selector label="What" options={UNITS} value={unit} onChange={v => setUnit(v as Unit)} width={120} />
        <Whole label="Amount" value={amount} onChange={setAmount} width={180} isRequired />
      </HStack>
      <Text type="supporting" as="p">
        They have {t[unit].toLocaleString('en-US')} {name}. At most {t.caps[unit].toLocaleString('en-US')} {name} in one
        change; a balance never goes below 0. An online player's game shows new NX and GP at once; in a match, GP and
        EXP show when it ends.
      </Text>
      <Reason value={reason} onChange={setReason} />
    </Flow>
  );
}

function BanFlow({t, ...rest}: Common) {
  const [length, setLength] = useState('1');
  const [unit, setUnit] = useState<'hours' | 'days'>('days');
  const [permanent, setPermanent] = useState(false);
  const [reason, setReason] = useState('');
  return (
    <Flow
      title={`${t.ban ? 'Change the ban' : 'Ban'}: ${who(t)}`}
      plan={() => planBan(t, {length: typedWhole(length), unit, permanent, reason}, Date.now() / 1000)}
      {...rest}>
      <HStack gap={2} align="end" wrap="wrap">
        <Whole label="Length" value={length} onChange={setLength} width={140} isDisabled={permanent} />
        <Selector
          label="Unit"
          options={[
            {value: 'hours', label: 'hours'},
            {value: 'days', label: 'days'},
          ]}
          value={unit}
          onChange={v => setUnit(v as 'hours' | 'days')}
          width={120}
          isDisabled={permanent}
        />
      </HStack>
      <CheckboxInput label="Permanent" value={permanent} onChange={setPermanent} />
      <Text type="supporting" as="p">
        A ban drops every connection of the account and refuses its logins with the banned box until it ends.
      </Text>
      <Reason value={reason} onChange={setReason} />
    </Flow>
  );
}

function KickFlow({t, ...rest}: Common) {
  const [reason, setReason] = useState('');
  return (
    <Flow title={`Kick: ${who(t)}`} plan={() => planKick(t, {reason})} {...rest}>
      <Reason value={reason} onChange={setReason} />
    </Flow>
  );
}

function MuteFlow({t, ...rest}: Common) {
  const [minutes, setMinutes] = useState('60');
  const [reason, setReason] = useState('');
  return (
    <Flow
      title={`${t.mute ? 'Change the mute' : 'Mute'}: ${who(t)}`}
      plan={() => planMute(t, {minutes: typedWhole(minutes), reason}, Date.now() / 1000)}
      {...rest}>
      <Whole
        label="Minutes"
        description={`At most ${t.max_mute_minutes.toLocaleString('en-US')}.`}
        value={minutes}
        onChange={setMinutes}
        width={200}
      />
      <Reason value={reason} onChange={setReason} optional />
    </Flow>
  );
}

/** `changed`: read the account again; `deleted`: the account is gone, leave its page. */
export function PlayerActions({api, t, name, changed, deleted}: {
  api: Api;
  t: Target;
  /** The name in the page's address, for the classic page's view of the same player. */
  name: string;
  changed: () => Promise<void>;
  deleted: () => void;
}) {
  const [open, setOpen] = useState<Change | null>(null);
  const toast = useToast();
  const lobby = api.boot.current !== null ? lobbyLabel(api.boot) : null;
  const post = (action: string, body: Record<string, unknown>) =>
    api.post<{message?: string}>(`/ops/player/${action}`, body).then(r => r.message || 'Done.');

  const send = async (plan: Plan) => {
    const said = await post(plan.action, plan.body);
    setOpen(null);
    toast({body: said});
    if (plan.action === 'delete') deleted();
    else await changed();
  };
  // undone with one press and nobody notices: no question, the toast says what happened
  const atOnce = (action: string) => async () => {
    try {
      toast({body: await post(action, {account: t.account})});
    } catch (e) {
      toast({type: 'error', body: e instanceof Error ? e.message : String(e)});
    }
    await changed();
  };

  const common: Common = {t, lobby, send, onClose: () => setOpen(null)};
  const off = (c: Change) => blocked(t, c);
  const why = (
    [
      ['Give or take', 'grant'],
      ['Kick', 'kick'],
      ['Mute', 'mute'],
      ['Reset mail', 'reset_mail'],
      ['Delete', 'delete'],
    ] as [string, Change][]
  ).filter(([, c]) => off(c));
  const classic = classicFor({screen: 'players', arg: name});
  return (
    <VStack gap={2}>
      <HStack gap={2} wrap="wrap">
        <Button label="Give or take" isDisabled={!!off('grant')} onClick={() => setOpen('grant')} />
        <Button label={t.ban ? 'Change the ban' : 'Ban'} onClick={() => setOpen('ban')} />
        {t.ban && <Button label="Lift the ban" onClick={() => setOpen('unban')} />}
        <Button label="Kick" isDisabled={!!off('kick')} onClick={() => setOpen('kick')} />
        <Button label={t.mute ? 'Change the mute' : 'Mute'} isDisabled={!!off('mute')} onClick={() => setOpen('mute')} />
        {t.mute && <Button label="Unmute" clickAction={atOnce('unmute')} />}
        {t.disabled ? (
          <Button label="Enable login" clickAction={atOnce('enable')} />
        ) : (
          <Button label="Disable login" onClick={() => setOpen('disable')} />
        )}
        <Button label="Send a reset mail" isDisabled={!!off('reset_mail')} onClick={() => setOpen('reset_mail')} />
        <Button label="Delete account" variant="destructive" isDisabled={!!off('delete')} onClick={() => setOpen('delete')} />
      </HStack>
      {why.map(([label, c]) => (
        <Text key={c} type="supporting" as="p">
          {label}: {off(c)}
        </Text>
      ))}
      <HStack gap={2} align="center" wrap="wrap">
        <Text type="supporting">The loadout is still given on the classic dashboard:</Text>
        <Button label="Open this player there" variant="ghost" size="sm" href={api.page(classic.path) + classic.hash} />
      </HStack>
      {open === 'grant' && <GrantFlow {...common} />}
      {open === 'ban' && <BanFlow {...common} />}
      {open === 'kick' && <KickFlow {...common} />}
      {open === 'mute' && <MuteFlow {...common} />}
      {open === 'unban' && <Flow title="" plan={() => planUnban(t)} lobby={lobby} send={send} onClose={common.onClose} />}
      {open === 'disable' && (
        <Flow title="" plan={() => planDisable(t)} lobby={lobby} send={send} onClose={common.onClose} />
      )}
      {open === 'reset_mail' && (
        <Flow title="" plan={() => planResetMail(t)} lobby={lobby} send={send} onClose={common.onClose} />
      )}
      {open === 'delete' && <Flow title="" plan={() => planDelete(t)} lobby={lobby} send={send} onClose={common.onClose} />}
    </VStack>
  );
}
