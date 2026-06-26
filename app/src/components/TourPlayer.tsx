import type { TourState } from "../types";

interface TourPlayerProps {
  tour: TourState;
  /** Milliseconds the current stop will dwell before auto-advancing (drives the
   * progress bar animation); 0 when paused or on the last stop. */
  dwellMs: number;
  onPrev: () => void;
  onNext: () => void;
  onPlayPause: () => void;
  onExit: () => void;
}

/**
 * The cinematic guided-tour caption bar: a calm, fixed overlay at the bottom of
 * the window showing the current narration, a dwell progress bar, and minimal
 * transport controls. The scrolling/flashing of the diff is driven by App.
 */
export function TourPlayer({ tour, dwellMs, onPrev, onNext, onPlayPause, onExit }: TourPlayerProps) {
  if (tour.status === "loading") {
    return (
      <div className="tour-player tour-loading">
        <span className="tour-spinner" aria-hidden="true">↻</span>
        Preparing your guided tour…
      </div>
    );
  }
  if (tour.status !== "active") return null;
  const stop = tour.stops[tour.index];
  if (!stop) return null;

  const total = tour.stops.length;
  const atStart = tour.index === 0;
  const atEnd = tour.index >= total - 1;

  return (
    <div className="tour-player">
      {/* Dwell progress: re-keyed per stop so the fill animation restarts. */}
      <div className="tour-dwell-track">
        <div
          key={tour.index}
          className="tour-dwell-fill"
          style={{
            animationDuration: dwellMs > 0 ? `${dwellMs}ms` : "0ms",
            animationPlayState: tour.playing && !atEnd ? "running" : "paused",
          }}
        />
      </div>
      <div className="tour-body">
        <p className="tour-caption" key={tour.index}>{stop.narration}</p>
        <div className="tour-controls">
          <span className="tour-progress">{tour.index + 1} / {total}</span>
          <button className="tour-btn" onClick={onPrev} disabled={atStart} title="Previous stop">
            &lsaquo;
          </button>
          <button className="tour-btn tour-playpause" onClick={onPlayPause} title={tour.playing ? "Pause" : "Play"}>
            {tour.playing ? "❚❚" : "▶"}
          </button>
          <button className="tour-btn" onClick={onNext} disabled={atEnd} title="Next stop">
            &rsaquo;
          </button>
          <button className="tour-btn tour-exit" onClick={onExit} title="Exit tour (Esc)">
            ✕
          </button>
        </div>
      </div>
    </div>
  );
}
