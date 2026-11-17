// A list of the lobby's events (GET /ops/events and the events of a match or a person), newest first as the lobby
// sends them: the kind, who and which match, the detail, how long ago.
// Provenance: ORIGINAL (DeCombat operator tooling)

import {useState} from 'react';
import {toHash} from '../routes.ts';
import {ago, when} from '../time.ts';
import {Button, Card, HStack, List, ListItem, Text, VStack} from '../ui/index.ts';
import {Quiet} from './parts.tsx';

export interface EventRow {
  at: number;
  kind: string;
  /** The lobby marks what went wrong. */
  problem?: boolean;
  nick?: string | null;
  match?: number | string | null;
  detail?: string | null;
}

/** How many rows are shown before "Show all". */
const FIRST = 50;

/** `ofOne`: the list is one person's own, so a row names no person and opens its match. */
export function EventLog({events, now, empty, ofOne = false}: {
  events: EventRow[];
  now: number;
  empty: string;
  ofOne?: boolean;
}) {
  const [all, setAll] = useState(false);
  if (events.length === 0) return <Quiet>{empty}</Quiet>;
  const rows = all ? events : events.slice(0, FIRST);
  return (
    <VStack gap={2}>
      <Card padding={0}>
        <List hasDividers>
          {rows.map((e, i) => {
            const match = e.match == null || e.match === '' || e.match === 0 ? '' : String(e.match);
            // a row opens the person it is about, else the match
            const nick = ofOne ? '' : e.nick;
            const href = nick ? toHash('players', nick) : match ? toHash('matches', match) : undefined;
            const who = [nick, match ? `match ${match}` : ''].filter(Boolean).join(' · ');
            return (
              <ListItem
                key={`${e.at}:${i}`}
                label={
                  <Text type="body" weight="medium">
                    {e.kind}
                    {who ? (
                      <Text type="body" color="secondary">
                        {' '}
                        · {who}
                      </Text>
                    ) : null}
                  </Text>
                }
                description={
                  e.detail ? (
                    <Text type="supporting" as="div" wordBreak="break-word">
                      {e.detail}
                    </Text>
                  ) : undefined
                }
                href={href}
                align="first-line"
                endContent={
                  <Text type="supporting" textWrap="nowrap">
                    <time dateTime={new Date(e.at * 1000).toISOString()} title={when(e.at)}>
                      {ago(e.at, now)}
                    </time>
                  </Text>
                }
              />
            );
          })}
        </List>
      </Card>
      {events.length > FIRST && (
        <HStack gap={2} align="center">
          <Button
            label={all ? `Show the newest ${FIRST}` : `Show all ${events.length}`}
            variant="secondary"
            size="sm"
            onClick={() => setAll(!all)}
          />
        </HStack>
      )}
    </VStack>
  );
}
