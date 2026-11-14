// The navigation's pictures, one per screen: 24 x 24 line drawings in the ink of the text beside them. Drawn for
// this dashboard; the design library ships none for these subjects.
// Provenance: ORIGINAL (DeCombat operator tooling)

import type {ReactNode, SVGProps} from 'react';
import type {ScreenId} from '../routes.ts';

type IconFn = (props: SVGProps<SVGSVGElement>) => ReactNode;

const draw =
  (body: ReactNode): IconFn =>
  props => (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 24 24"
      width="1em"
      height="1em"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...props}>
      {body}
    </svg>
  );

export const SCREEN_ICON: Record<ScreenId, IconFn> = {
  // four panels
  overview: draw(
    <>
      <rect x="3.5" y="3.5" width="7" height="9" rx="1.5" />
      <rect x="13.5" y="3.5" width="7" height="5" rx="1.5" />
      <rect x="13.5" y="11.5" width="7" height="9" rx="1.5" />
      <rect x="3.5" y="15.5" width="7" height="5" rx="1.5" />
    </>,
  ),
  // a sight
  matches: draw(
    <>
      <circle cx="12" cy="12" r="7" />
      <path d="M12 2.5v4M12 17.5v4M2.5 12h4M17.5 12h4" />
      <circle cx="12" cy="12" r="1" />
    </>,
  ),
  // a pulse
  health: draw(<path d="M2.5 12.5h4l2.5-6.5 4.5 12 2.5-5.5h5.5" />),
  bugs: draw(
    <>
      <rect x="8" y="8" width="8" height="11.5" rx="4" />
      <path d="M9.5 8a2.5 2.5 0 0 1 5 0M9 5.5 7.5 4M15 5.5 16.5 4M8 11.5H4.5M8 15H4M16 11.5h3.5M16 15h4M8.6 18 6 20M15.4 18 18 20M12 12v7.5" />
    </>,
  ),
  players: draw(
    <>
      <circle cx="9" cy="8" r="3.2" />
      <path d="M3 19.5a6 6 0 0 1 12 0M15.5 5.2a3 3 0 0 1 0 5.8M17.5 14.3a5.5 5.5 0 0 1 3.5 5.2" />
    </>,
  ),
  // a list with a tick
  beta: draw(
    <>
      <rect x="5" y="4.5" width="14" height="16" rx="2" />
      <path d="M9 4.5v-1h6v1M9 13.2l2.2 2.2 4.3-4.6" />
    </>,
  ),
  bots: draw(
    <>
      <rect x="5" y="8.5" width="14" height="11" rx="2.5" />
      <path d="M12 8.5V5.5M9.5 16.5h5M3 13v3M21 13v3" />
      <circle cx="12" cy="4.3" r="1.2" />
      <circle cx="9.3" cy="12.8" r="1" />
      <circle cx="14.7" cy="12.8" r="1" />
    </>,
  ),
  // up into place
  deploy: draw(<path d="M12 15.5V4M7.5 8.5 12 4l4.5 4.5M4 15.5v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3" />),
  packs: draw(<path d="M3.5 8 12 3.5 20.5 8v8L12 20.5 3.5 16zM3.5 8l8.5 4.5L20.5 8M12 12.5v8" />),
  // a tag
  release: draw(
    <>
      <path d="M3.5 12.2V5a1.5 1.5 0 0 1 1.5-1.5h7.2l8.3 8.3a1.5 1.5 0 0 1 0 2.1l-6.6 6.6a1.5 1.5 0 0 1-2.1 0z" />
      <circle cx="8" cy="8" r="1.2" />
    </>,
  ),
  keys: draw(
    <>
      <circle cx="8" cy="15.5" r="4" />
      <path d="M11 12.5 19.5 4M16.5 7l2.5 2.5M13.8 9.7l2 2" />
    </>,
  ),
};

/** The earlier dashboard: a clock running back. */
export const ClassicIcon = draw(<path d="M4.2 12a7.8 7.8 0 1 0 2.4-5.6M4 3.8v4.3h4.3M12 8v4.4l3 1.8" />);
