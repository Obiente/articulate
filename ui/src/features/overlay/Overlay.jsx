import { useEffect, useState } from "react";
import { onOverlay, overlaySnapshot } from "../../bridge.js";
import "./overlay.css";

// A slow check in case an update event was missed. Updates are pushed.
const FALLBACK_INTERVAL = 3000;

export function Overlay() {
  const [state, setState] = useState({
    message: "",
    recording: false,
    finishing: false,
  });
  useEffect(() => {
    let active = true,
      timer,
      unlisten;
    const show = (next) => {
      if (active && next) setState(next);
    };
    async function refresh() {
      try {
        show(await overlaySnapshot());
      } catch {
        /* Native window owns visibility. */
      } finally {
        if (active) timer = setTimeout(refresh, FALLBACK_INTERVAL);
      }
    }
    onOverlay(show)
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      })
      .catch(() => {
        /* The fallback check keeps the indicator current. */
      });
    void refresh();
    return () => {
      active = false;
      clearTimeout(timer);
      unlisten?.();
    };
  }, []);
  return (
    <div className="dictation-overlay" role="status">
      <div
        className={`overlay-signal ${state.recording ? "listening" : ""} ${state.finishing ? "finishing" : ""}`}
        aria-hidden="true"
      >
        {[0, 1, 2, 3, 4].map((i) => (
          <i key={i} />
        ))}
      </div>
      <div>
        <strong>
          {state.recording
            ? "Listening"
            : state.finishing
              ? "Finishing your words"
              : "Articulate"}
        </strong>
        <p>{state.message || "Ready when you are"}</p>
      </div>
    </div>
  );
}
