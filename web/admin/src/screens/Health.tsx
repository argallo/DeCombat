// Health: the lobby's alert checks and its problem log, the classic page's tabs Alerts and Problems (DC_DEFAULTS.md
// LG-446).
// It only reads. The charts are those of LG-449.
// Provenance: ORIGINAL (DeCombat operator tooling)

import {useCallback} from 'react';
import type {Api} from '../api.ts';
import {plural} from '../overview.ts';
import {usePoll} from '../poll.ts';
import {Badge, Banner, Card, Grid, HStack, Icon, Text, VStack} from '../ui/index.ts';
import {PROBLEM_LIMIT, ProblemsPerHour} from '../viz/activity.tsx';
import {Meter} from '../viz/charts.tsx';
import {type EventRow, EventLog} from './EventLog.tsx';
import {Part, Quiet, ScreenHead} from './parts.tsx';

/** One row of GET /ops/alerts. */
interface Check {
  name: string;
  ok: boolean;
  text?: string;
}

interface Reading {
  /** null: the checks did not answer (the log still shows). */
  checks: Check[] | null;
  problems: EventRow[];
}

async function load(api: Api): Promise<Reading> {
  const [checks, problems] = await Promise.all([
    api.get<Check[]>('/ops/alerts').catch(() => null),
    api.get<EventRow[]>(`/ops/events?problems=1&limit=${PROBLEM_LIMIT}`),
  ]);
  return {checks, problems};
}

function Checks({checks}: {checks: Check[] | null}) {
  if (checks === null) return <Quiet>The checks did not answer.</Quiet>;
  if (checks.length === 0) return <Quiet>This lobby has no checks.</Quiet>;
  // the failing ones first, each group in the lobby's order
  const rows = [...checks.filter(c => !c.ok), ...checks.filter(c => c.ok)];
  return (
    // one tile per check, its state as a sign and a word
    <Grid columns={{minWidth: 240, max: 3}} gap={3}>
      {rows.map(c => (
        <Card key={c.name} padding={3} variant={c.ok ? 'default' : 'red'}>
          <HStack gap={2} align="start">
            <Icon icon={c.ok ? 'success' : 'error'} color={c.ok ? 'success' : 'error'} size="lg" />
            <VStack gap={0.5}>
              <HStack gap={2} align="center" wrap="wrap">
                <Text type="body" weight="medium">
                  {c.name}
                </Text>
                <Badge variant={c.ok ? 'success' : 'error'} label={c.ok ? 'OK' : 'Failing'} />
              </HStack>
              {c.text && (
                <Text type="supporting" as="div" wordBreak="break-word">
                  {c.text}
                </Text>
              )}
            </VStack>
          </HStack>
        </Card>
      ))}
    </Grid>
  );
}

/** How many of the last day's problems are of each kind, the most frequent first. */
function Kinds({problems, since}: {problems: EventRow[]; since: number}) {
  const count = new Map<string, number>();
  for (const e of problems) if (e.at >= since) count.set(e.kind, (count.get(e.kind) ?? 0) + 1);
  const rows = [...count].sort((x, y) => y[1] - x[1] || (x[0] < y[0] ? -1 : 1)).slice(0, 8);
  if (rows.length === 0) return <Quiet>Nothing went wrong in the last 24 hours.</Quiet>;
  const top = rows[0]?.[1] ?? 1;
  return (
    <VStack gap={2}>
      {rows.map(([kind, n]) => (
        <VStack gap={0.5} key={kind}>
          <HStack gap={2} justify="between">
            <Text type="body" wordBreak="break-word">
              {kind}
            </Text>
            <Text type="body" hasTabularNumbers>
              {n}
            </Text>
          </HStack>
          <Meter value={n} max={top} paint="s2" label={`${kind}: ${n}`} />
        </VStack>
      ))}
    </VStack>
  );
}

export function Health({api}: {api: Api}) {
  const poll = usePoll(useCallback(() => load(api), [api]));
  const r = poll.data;
  const now = Date.now() / 1000;
  const failing = r?.checks?.filter(c => !c.ok) ?? [];
  const recent = r?.problems.filter(e => e.at >= now - 3600).length ?? 0;

  return (
    <VStack gap={5}>
      <ScreenHead title="Health" poll={poll} />
      {r && (
        <>
          <Banner
            status={failing.length || recent ? 'warning' : 'success'}
            title={
              failing.length
                ? `${plural(failing.length, 'check')} failing: ${failing.map(c => c.name).join(', ')}.`
                : recent
                  ? `${plural(recent, 'problem')} logged in the last hour.`
                  : 'Every check passes and nothing went wrong in the last hour.'
            }
            description={
              failing.length && recent ? `${plural(recent, 'problem')} logged in the last hour.` : undefined
            }
          />
          <Part
            title="Alert checks"
            note="The default thresholds. healthcheck.sh on the server runs the same checks with its own.">
            <Checks checks={r.checks} />
          </Part>
          {r.problems.length > 0 && (
            <Grid columns={{minWidth: 300, max: 2}} gap={5} align="start">
              <Part title="Problems per hour" note="The last 24 hours.">
                <Card padding={3}>
                  <ProblemsPerHour events={r.problems} now={now} height={140} />
                </Card>
              </Part>
              <Part title="By kind" note="The last 24 hours, the most frequent first.">
                <Card padding={3}>
                  <Kinds problems={r.problems} since={now - 86400} />
                </Card>
              </Part>
            </Grid>
          )}
          <Part
            title="Problem log"
            note="Dropped links, failed starts, refused sign-ins and other things that went wrong, newest first.">
            <EventLog events={r.problems} now={now} empty="No problems logged." />
          </Part>
        </>
      )}
    </VStack>
  );
}
