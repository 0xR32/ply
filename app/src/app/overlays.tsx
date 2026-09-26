import { DispatchForm } from '../features/dispatch/dispatch-form';
import { QueueSheet } from '../features/dispatch/queue-sheet';
import { NewPane } from '../features/new-pane/new-pane';
import { Palette } from '../features/palette/palette';
import { QuitConfirm } from '../features/palette/quit-confirm';
import { CloseConfirm } from '../features/panes/close-confirm';
import { Settings } from '../features/settings/settings';
import { useAppSelector, useDispatch } from '../state/store';
import { Backdrop } from '../ui/overlay-card';

/** The open overlay, if any, over a dimmed backdrop that closes it on click. */
export function Overlays() {
  const dispatch = useDispatch();
  const overlay = useAppSelector((s) => s.overlay);
  if (!overlay) return null;
  const close = () => dispatch({ type: 'overlay/close' });
  switch (overlay.kind) {
    case 'palette':
      return (
        <Backdrop onClose={close}>
          <Palette />
        </Backdrop>
      );
    case 'new-pane':
      return (
        <Backdrop onClose={close}>
          <NewPane target={overlay.target} />
        </Backdrop>
      );
    case 'settings':
      return (
        <Backdrop onClose={close}>
          <Settings />
        </Backdrop>
      );
    case 'close-confirm':
      return (
        <Backdrop onClose={close}>
          <CloseConfirm paneId={overlay.paneId} />
        </Backdrop>
      );
    case 'quit-confirm':
      return (
        <Backdrop onClose={close}>
          <QuitConfirm />
        </Backdrop>
      );
    case 'dispatch':
      return (
        <Backdrop onClose={close}>
          <DispatchForm paneId={overlay.paneId} />
        </Backdrop>
      );
    case 'queue':
      return (
        <Backdrop onClose={close}>
          <QueueSheet />
        </Backdrop>
      );
  }
}
