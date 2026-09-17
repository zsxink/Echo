import { useRef } from "react";

import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";

/**
 * The prototype's `.confirmation-dialog`: a blocking panel used for the delete
 * confirmation. It is the `BlockingDialog` tier, so Escape closes it before
 * anything beneath it.
 */
export function ConfirmationDialog(props: {
  readonly title: string;
  readonly description: string;
  readonly confirmLabel: string;
  readonly onConfirm: () => void;
  readonly onCancel: () => void;
  readonly cancelLabel?: string;
  readonly testId?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useOverlay({ tier: OverlayTier.BlockingDialog, onClose: props.onCancel, containerRef: ref });
  useFocusTrap(ref);

  return (
    <section
      className="confirmation-dialog"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="confirmation-title"
      data-testid={props.testId}
      ref={ref}
    >
      <div className="confirmation-panel">
        <h2 id="confirmation-title">{props.title}</h2>
        <p>{props.description}</p>
        <div className="confirmation-actions">
          <button type="button" className="btn" onClick={props.onCancel}>
            {props.cancelLabel ?? "取消"}
          </button>
          <button type="button" className="btn btn-danger" onClick={props.onConfirm}>
            {props.confirmLabel}
          </button>
        </div>
      </div>
    </section>
  );
}
