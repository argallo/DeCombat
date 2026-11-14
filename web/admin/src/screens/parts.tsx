// What every screen has: its heading with the time of the last reading and Refresh, the message when the lobby does
// not answer, a titled part of the page, and the quiet line of an empty one.
// Provenance: ORIGINAL (DeCombat operator tooling)

import type {ReactNode} from 'react';
import {ApiError} from '../api.ts';
import type {Poll} from '../poll.ts';
import {clock} from '../time.ts';
import {Banner, Button, HStack, Heading, Spinner, Text, VStack} from '../ui/index.ts';

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
