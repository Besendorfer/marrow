import type { PrChecksStatus, CheckRunInfo } from "../types";

interface ChecksBlockingModalProps {
  checksStatus: PrChecksStatus;
  onDismiss: () => void;
}

function checkState(check: CheckRunInfo): "success" | "pending" | "fail" {
  if (check.status !== "COMPLETED") return "pending";
  if (check.conclusion === "SUCCESS" || check.conclusion === "NEUTRAL" || check.conclusion === "SKIPPED") return "success";
  return "fail";
}

export function ChecksBlockingModal({ checksStatus, onDismiss }: ChecksBlockingModalProps) {
  const isPending = checksStatus.overall_state === "pending";

  const sorted = [...checksStatus.check_runs].sort((a, b) => {
    const order = { fail: 0, pending: 1, success: 2 };
    return order[checkState(a)] - order[checkState(b)];
  });

  return (
    <div className="checks-modal-overlay">
      <div className="checks-modal">
        <div className="checks-modal-icon">
          {isPending ? (
            <span className="checks-modal-spinner" />
          ) : (
            <span className="checks-modal-x">&times;</span>
          )}
        </div>
        <h2 className="checks-modal-title">
          {isPending ? "Checks are running" : "Checks are failing"}
        </h2>
        <p className="checks-modal-description">
          {isPending
            ? "CI checks are still in progress. The review will be unblocked automatically when all checks pass."
            : "One or more CI checks have failed. You may want to wait for fixes before reviewing."}
        </p>

        {sorted.length > 0 ? (
          <div className="checks-modal-list">
            {sorted.map((check) => {
              const state = checkState(check);
              return (
                <div key={check.name} className={`checks-modal-item checks-modal-item-${state}`}>
                  {state === "success" && <span className="checks-modal-item-icon">&#x2713;</span>}
                  {state === "fail" && <span className="checks-modal-item-icon">&times;</span>}
                  {state === "pending" && <span className="checks-modal-item-icon checks-modal-dot-pulse" />}
                  <span className="checks-modal-item-name">{check.name}</span>
                </div>
              );
            })}
          </div>
        ) : (
          <div className="checks-modal-list">
            <div className="checks-modal-item checks-modal-item-fail">
              <span className="checks-modal-item-icon">&times;</span>
              <span className="checks-modal-item-name">One or more checks failed</span>
            </div>
          </div>
        )}

        <button className="checks-modal-dismiss" onClick={onDismiss}>
          Ignore
        </button>
      </div>
    </div>
  );
}
