// The dialog every change of the dashboard goes through (DC_DEFAULTS.md LG-451): first the change's form
// when it has one, then the question: the change as a sentence, what happens, on which lobby, the word to type when
// it cannot be undone, and a button that names the change. A refusal of the lobby is shown inside the open dialog,
// so nothing typed is lost.
// Provenance: ORIGINAL (DeCombat operator tooling)

import {type ReactNode, useState} from 'react';
import {type Plan, type Planned, typedIt} from './actions.ts';
import {
  Banner,
  Button,
  Dialog,
  DialogHeader,
  HStack,
  Text,
  TextInput,
  VStack,
} from './ui/index.ts';

export interface FlowProps {
  /** The form's title: the change and whom it is for. */
  title: string;
  /** The further lobby the page is on, null on the lobby that serves it: said in every question. */
  lobby: string | null;
  /** The change as the form stands: a plan, or the sentence that says what is missing. */
  plan: () => Planned;
  /** Sends a confirmed plan; a refusal is thrown and shown in the dialog. */
  send: (plan: Plan) => Promise<void>;
  onClose: () => void;
  /** The form's fields; without them the dialog opens with the question. */
  children?: ReactNode;
}

export function Flow({title, lobby, plan, send, onClose, children}: FlowProps) {
  const hasForm = children != null;
  const [asked, setAsked] = useState<Plan | null>(() => {
    if (hasForm) return null;
    const p = plan();
    return typeof p === 'string' ? null : p;
  });
  const [problem, setProblem] = useState('');
  const [typed, setTyped] = useState('');
  const [refused, setRefused] = useState('');

  const next = () => {
    const p = plan();
    if (typeof p === 'string') {
      setProblem(p);
      return;
    }
    setProblem('');
    setRefused('');
    setTyped('');
    setAsked(p);
  };
  const ask = asked?.ask;
  const ready = !ask?.type || typedIt(typed, ask.type);

  return (
    <Dialog
      isOpen
      onOpenChange={open => {
        if (!open) onClose();
      }}
      // a form, and a word being typed, are not lost to a tap beside the dialog
      purpose={!ask || ask.type ? 'form' : 'info'}
      width={480}
      padding={4}>
      <VStack gap={4}>
        <DialogHeader title={ask ? ask.title : title} onOpenChange={() => onClose()} />
        {/* the header keeps a margin of its own; the body lines up with its title */}
        <div className="dc-dialog-body">
          {ask ? (
            <VStack gap={3}>
              {lobby && (
                <Text type="body" weight="semibold" as="p">
                  On: {lobby}
                </Text>
              )}
              <VStack gap={1}>
                <Text type="supporting">What happens</Text>
                {ask.lines.map(line => (
                  <Text key={line} type="body" as="p" wordBreak="break-word">
                    {line}
                  </Text>
                ))}
              </VStack>
              {ask.type && (
                <TextInput
                  label={`Type ${ask.type} to confirm`}
                  value={typed}
                  onChange={setTyped}
                  autoComplete="off"
                  autoCapitalize="none"
                  hasAutoCorrect={false}
                  hasSpellCheck={false}
                />
              )}
              {refused && <Banner status="error" title="The lobby refused it. Nothing changed." description={refused} />}
            </VStack>
          ) : (
            <VStack gap={3}>
              {children}
              {problem && <Banner status="warning" title={problem} />}
            </VStack>
          )}
          <HStack gap={2} justify="end" wrap="wrap">
            {ask && hasForm && <Button label="Back" variant="ghost" onClick={() => setAsked(null)} />}
            <Button label="Cancel" variant="secondary" onClick={onClose} />
            {ask && asked ? (
              <Button
                label={ask.button}
                variant={ask.danger ? 'destructive' : 'primary'}
                isDisabled={!ready}
                clickAction={async () => {
                  setRefused('');
                  try {
                    await send(asked);
                  } catch (e) {
                    setRefused(e instanceof Error ? e.message : String(e));
                  }
                }}
              />
            ) : (
              <Button label="Continue" variant="primary" onClick={next} />
            )}
          </HStack>
        </div>
      </VStack>
    </Dialog>
  );
}
