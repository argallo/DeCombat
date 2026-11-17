// What every screen has: its heading with the time of the last reading and Refresh, the message when the lobby does
// not answer, a titled part of the page, the quiet line of an empty one, and the small pieces more than one screen
// shows (a tile with one number, a labelled fact, the badge of a connection's state).
// Provenance: ORIGINAL (DeCombat operator tooling)

import type {ReactNode} from 'react';
import {ApiError} from '../api.ts';
import {type Health, healthOf} from '../overview.ts';
import type {Poll} from '../poll.ts';
import {clock} from '../time.ts';
import {Badge, Banner, Button, Card, HStack, Heading, Spinner, Text, VStack} from '../ui/index.ts';

/** The screen's heading, and under it what went wrong with the newest reading. */
export function ScreenHead({title, poll}: {title: string; poll: Poll<unknown>}) {
  const {error, at, data} = poll;
  const signedOut = error instanceof ApiError && error.signedOut;
  return (
    <>
      <HStack gap={3} justify="between" align="center" wrap="wrap">
        <Heading level={1}>{title}</Heading>
        <HStack gap={2} align="center">
          {at > 0 && (
            <Text type="supporting">
              {error ? 'Last answer' : 'Updated'} {clock(at)}
            </Text>
          )}
          <Button label="Refresh" variant="secondary" size="sm" clickAction={poll.refresh} />
        </HStack>
      </HStack>
      {error && (
        <Banner
          status={signedOut ? 'warning' : 'error'}
          title={signedOut ? 'Your session ended. Sign in again.' : 'The dashboard cannot reach the lobby.'}
          description={
            signedOut
              ? 'A lobby restart ends every admin web session.'
              : `${error.message}${data ? ' What is below is the last answer.' : ''}`
          }
          endContent={signedOut ? <Button label="Sign in" variant="primary" href="/login" /> : undefined}
        />
      )}
      {!data && !error && <Spinner label="Asking the lobby" />}
    </>
  );
}

export function Part({title, note, children}: {title: string; note?: string; children: ReactNode}) {
  return (
    <VStack gap={2}>
      <VStack gap={0.5}>
        <Heading level={2}>{title}</Heading>
        {note && <Text type="supporting">{note}</Text>}
      </VStack>
      {children}
    </VStack>
  );
}

export function Quiet({children}: {children: ReactNode}) {
  return (
    <Text type="body" color="secondary" as="p">
      {children}
    </Text>
  );
}

/** One number with its name, a small picture of it and a line of what it is made of. */
export function Tile({label, value, visual, note}: {label: string; value: string; visual?: ReactNode; note?: ReactNode}) {
  return (
    <Card padding={3}>
      <VStack gap={2}>
        <VStack gap={0}>
          <Text type="supporting">{label}</Text>
          <Text type="display-3">{value}</Text>
        </VStack>
        {visual}
        {/* a tile without a note keeps the height of its neighbours */}
        {typeof note === 'string' || note == null ? <Text type="supporting">{note ?? ' '}</Text> : note}
      </VStack>
    </Card>
  );
}

export function Fact({label, value}: {label: string; value: ReactNode}) {
  return (
    <VStack gap={0}>
      <Text type="supporting">{label}</Text>
      <Text type="body" hasTabularNumbers wordBreak="break-word">
        {value}
      </Text>
    </VStack>
  );
}

const HEALTH_WORD: Record<Health, string> = {good: 'Good', warn: 'Shaky', bad: 'Bad', none: 'No data'};
const HEALTH_BADGE: Record<Health, 'success' | 'warning' | 'error' | 'neutral'> = {
  good: 'success',
  warn: 'warning',
  bad: 'error',
  none: 'neutral',
};

/** The state of a connection as the lobby judged it. */
export function HealthBadge({health}: {health: string | null | undefined}) {
  const h = healthOf(health);
  return <Badge variant={HEALTH_BADGE[h]} label={HEALTH_WORD[h]} />;
}
