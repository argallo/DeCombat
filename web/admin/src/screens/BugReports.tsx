// Bug reports: what the report poller collected, the open ones first, each with a Done tick (the classic Updates
// tab's card of that name; DC_DEFAULTS.md LG-448). Every field of a report is a stranger's text and is shown as text;
// only an attachment address the lobby has checked becomes a link.
// Provenance: ORIGINAL (DeCombat operator tooling)

import {useCallback, useState} from 'react';
import type {Api} from '../api.ts';
import {plural} from '../overview.ts';
import {usePoll} from '../poll.ts';
import {ago, when} from '../time.ts';
import {Banner, Button, Card, CheckboxInput, HStack, Text, VStack} from '../ui/index.ts';
import {Part, Quiet, ScreenHead} from './parts.tsx';

interface Report {
  id: string | number;
  at?: number | null;
  author: {name?: string; username?: string};
  content: string;
  attachments: {link?: string | null; text?: string}[];
  done?: boolean;
  done_by?: string;
}

/** GET /ops/triage. */
interface Triage {
  set_up: boolean;
  message?: string;
  state?: {ok?: boolean; message?: string; updated?: number; fetched?: number} | null;
  open?: number;
  total?: number;
  reports: Report[];
}

/** How much of a long report shows before "more". */
const SHORT = 600;

function ReportCard({r, now, mark}: {r: Report; now: number; mark: (id: Report['id'], done: boolean) => Promise<void>}) {
  const [open, setOpen] = useState(false);
  const long = r.content.length > SHORT;
  const who =
    r.author.name && r.author.name !== r.author.username
      ? `${r.author.name} (${r.author.username ?? '?'})`
      : r.author.username || '?';
  return (
    <Card padding={3} variant={r.done ? 'muted' : 'default'}>
      <VStack gap={2}>
        <HStack gap={2} justify="between" align="start" wrap="wrap">
          <VStack gap={0}>
            <Text type="body" weight="medium">
              {who}
            </Text>
            <Text type="supporting">
              #{r.id}
              {r.at ? (
                <>
                  {' '}
                  · <time title={when(r.at)}>{ago(r.at, now)}</time>
                </>
              ) : null}
              {r.done ? ` · done by ${r.done_by || '?'}` : ''}
            </Text>
          </VStack>
          <CheckboxInput label="Done" value={!!r.done} changeAction={checked => mark(r.id, checked)} />
        </HStack>
        <Text type="body" as="p" wordBreak="break-word">
          <span style={{whiteSpace: 'pre-wrap'}}>{long && !open ? r.content.slice(0, SHORT) + '…' : r.content}</span>
        </Text>
        {long && (
          <HStack gap={2}>
            <Button label={open ? 'Show less' : 'Show all of it'} variant="secondary" size="sm" onClick={() => setOpen(!open)} />
          </HStack>
        )}
        {r.attachments.length > 0 && (
          <HStack gap={2} align="center" wrap="wrap">
            <Text type="supporting">Attachments</Text>
            {r.attachments.map((a, i) =>
              a.link ? (
                <Button
                  key={i}
                  label={`File ${i + 1}`}
                  variant="secondary"
                  size="sm"
                  href={a.link}
                  target="_blank"
                  rel="noreferrer noopener"
                />
              ) : (
                // an address the lobby did not accept as a link stays text
                <Text key={i} type="supporting" wordBreak="break-all">
                  {a.text}
                </Text>
              ),
            )}
          </HStack>
        )}
      </VStack>
    </Card>
  );
}

export function BugReports({api}: {api: Api}) {
  const poll = usePoll(useCallback(() => api.get<Triage>('/ops/triage'), [api]));
  const [failed, setFailed] = useState<string | null>(null);
  const t = poll.data;
  const now = Date.now() / 1000;
  const {refresh} = poll;

  const mark = useCallback(
    async (id: Report['id'], done: boolean) => {
      try {
        await api.post('/ops/triage/done', {id, done});
        setFailed(null);
      } catch (e) {
        setFailed(e instanceof Error ? e.message : String(e));
      }
      // the list is the lobby's: read it again whether the change was taken or not
      await refresh();
    },
    [api, refresh],
  );

  const s = t?.state;
  const open = t?.reports.filter(r => !r.done) ?? [];
  const done = t?.reports.filter(r => r.done) ?? [];

  return (
    <VStack gap={5}>
      <ScreenHead title="Bug reports" poll={poll} />
      {failed && <Banner status="error" title="The change was not saved." description={failed} />}
      {t && !t.set_up && (
        <Banner status="info" title="Bug reports are not set up on this lobby." description={t.message} />
      )}
      {t && t.set_up && (
        <>
          <Banner
            status={!s ? 'warning' : s.ok === false ? 'error' : open.length ? 'info' : 'success'}
            title={
              !s
                ? 'No poll yet.'
                : s.ok === false
                  ? `The last poll failed: ${s.message || '?'}`
                  : `${t.open ?? open.length} open of ${plural(t.total ?? t.reports.length, 'report')}.`
            }
            description={
              s?.updated
                ? `Polled ${ago(s.updated, now)}${s.ok === false ? '' : ` (${s.fetched ?? 0} new)`}.`
                : undefined
            }
          />
          <Part title={`Open (${open.length})`}>
            {open.length === 0 ? (
              <Quiet>{t.reports.length ? 'Every report is done.' : 'No reports yet.'}</Quiet>
            ) : (
              <VStack gap={3}>
                {open.map(r => (
                  <ReportCard key={r.id} r={r} now={now} mark={mark} />
                ))}
              </VStack>
            )}
          </Part>
          {done.length > 0 && (
            <Part title={`Done (${done.length})`} note="Untick one to open it again.">
              <VStack gap={3}>
                {done.map(r => (
                  <ReportCard key={r.id} r={r} now={now} mark={mark} />
                ))}
              </VStack>
            </Part>
          )}
          <Text type="supporting">The lobby lists the newest 50 reports.</Text>
        </>
      )}
    </VStack>
  );
}
