interface NextFileBarProps {
  index: number;
  total: number;
  isViewed: boolean;
  nextName: string | null;
  allReviewed: boolean;
  onMarkReviewed: () => void;
  onNext: () => void;
  onFinishReview: () => void;
}

export function NextFileBar({
  index,
  total,
  isViewed,
  nextName,
  allReviewed,
  onMarkReviewed,
  onNext,
  onFinishReview,
}: NextFileBarProps) {
  return (
    <div className="next-bar">
      <button
        className={`next-bar-mark${isViewed ? " next-bar-mark--done" : ""}`}
        onClick={onMarkReviewed}
        title={
          isViewed
            ? "Already reviewed — go to the next unreviewed file"
            : "Mark this file reviewed and go to the next one (V toggles without advancing)"
        }
      >
        {isViewed ? "Reviewed ✓" : "Mark reviewed"}
        {!isViewed && <kbd className="next-bar-key">V</kbd>}
      </button>
      <span className="next-bar-pos">
        {allReviewed ? (
          "All files reviewed"
        ) : (
          <>
            file {index + 1} of {total}
            {nextName && (
              <button className="next-bar-next" onClick={onNext} title="Next file">
                Next: <span className="next-bar-file">{nextName}</span>
                <kbd className="next-bar-key">]</kbd>
              </button>
            )}
          </>
        )}
      </span>
      <button
        className={`next-bar-finish${allReviewed ? " next-bar-finish--ready" : ""}`}
        onClick={onFinishReview}
        title="Submit your review (R)"
      >
        Finish review…
      </button>
    </div>
  );
}
