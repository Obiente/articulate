import { useEffect, useState } from "react";
import { overlaySnapshot } from "./bridge";
import "./overlay.css";

export function Overlay() {
  const [state, setState] = useState({
    message: "",
    recording: false,
    finishing: false,
  });
  useEffect(() => {
    let active = true,
      timer;
    async function poll() {
      try {
        const next = await overlaySnapshot();
        if (active) setState(next);
      } catch {
        /* Native window owns visibility. */
      } finally {
        if (active) timer = setTimeout(poll, 150);
      }
    }
    void poll();
    return () => {
      active = false;
      clearTimeout(timer);
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
