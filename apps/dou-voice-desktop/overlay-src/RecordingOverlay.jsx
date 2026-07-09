import React, { useEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { motion } from "framer-motion";

import { command, listen } from "../web/scripts/tauri-api.js";

const WAVE_BARS = 9;
const CARD = {
  rest: 172,
  work: 210,
  open: 372,
};

const phaseLabels = {
  idle: "Done",
  starting: "Starting",
  loading_auth: "Loading auth",
  recording: "Listening",
  stopping: "Stopping",
  recognizing: "Transcribing",
  typing: "Typing",
  error: "Error",
};

function RecordingOverlay() {
  const [status, setStatus] = useState({
    phase: "idle",
    message: "Ready.",
    lastText: "",
  });
  const [elapsed, setElapsed] = useState(0);
  const [levels, setLevels] = useState(Array(WAVE_BARS).fill(0));
  const [overflowing, setOverflowing] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const textCapRef = useRef(null);
  const peakRef = useRef(0.08);

  const phase = status.phase || "idle";
  const transcript = status.lastText || "";
  const open = transcript.length > 0;
  const textOpen = open && !leaving;
  const rowMode = rowModeForPhase(phase, open);
  const label = phaseLabels[phase] || phase;

  useEffect(() => {
    let active = true;
    let unlistenStatus;
    let unlistenLevel;
    let unlistenHide;

    listen("voice-status", (event) => {
      if (!active) return;
      setLeaving(false);
      setStatus(normalizeStatus(event.payload));
    }).then((callback) => {
      unlistenStatus = callback;
    });

    listen("overlay-hide-request", () => {
      if (active) setLeaving(true);
    }).then((callback) => {
      unlistenHide = callback;
    });

    listen("mic-level", (event) => {
      if (!active) return;
      const next = Array.isArray(event.payload) ? event.payload : [];
      const rawPeak = Math.max(...next.map((value) => Number(value) || 0), 0.001);
      peakRef.current = Math.max(rawPeak, peakRef.current * 0.92, 0.035);
      setLevels((previous) =>
        Array.from({ length: WAVE_BARS }, (_, index) => {
          const raw = Math.max(0, Math.min(1, Number(next[index] || 0)));
          const normalized = Math.min(1, raw / peakRef.current);
          const boosted = Math.pow(normalized, 0.45);
          return previous[index] * 0.55 + boosted * 0.45;
        }),
      );
    }).then((callback) => {
      unlistenLevel = callback;
    });

    command("get_voice_status")
      .then((payload) => {
        if (!active) return;
        setLeaving(false);
        setStatus(normalizeStatus(payload));
      })
      .catch(() => {});

    return () => {
      active = false;
      if (unlistenStatus) unlistenStatus();
      if (unlistenLevel) unlistenLevel();
      if (unlistenHide) unlistenHide();
    };
  }, []);

  useEffect(() => {
    if (phase !== "recording") {
      setElapsed(0);
      return undefined;
    }
    const started = Date.now();
    const id = window.setInterval(() => {
      setElapsed(Math.max(0, Math.floor((Date.now() - started) / 1000)));
    }, 250);
    return () => window.clearInterval(id);
  }, [phase]);

  useEffect(() => {
    const el = textCapRef.current;
    if (!el) return;
    setOverflowing(el.scrollHeight > el.clientHeight + 1);
    el.scrollTop = el.scrollHeight;
  }, [transcript]);

  const cardMotion = useMemo(() => {
    const width = open ? CARD.open : rowMode === "working" ? CARD.work : CARD.rest;
    const borderRadius = open ? 16 : rowMode === "working" ? 18 : 24;
    return { width, borderRadius };
  }, [open, rowMode]);
  const leavingMotion = useMemo(
    () => ({
      width: [cardMotion.width, CARD.rest, CARD.rest],
      borderRadius: [cardMotion.borderRadius, 24, 24],
      opacity: [1, 1, 0],
      scale: [1, 1, 0.96],
      y: [0, 0, 8],
    }),
    [cardMotion],
  );

  return (
    <main className="ov-stage" aria-live="polite">
      <motion.section
        className={`scard ${open ? "open" : ""} ${rowModeClass(rowMode)}`}
        data-phase={phase}
        initial={{ opacity: 0, scale: 0.92 }}
        animate={leaving ? leavingMotion : { opacity: 1, scale: 1, y: 0, ...cardMotion }}
        transition={
          leaving
            ? { duration: 0.68, times: [0, 0.62, 1], ease: [0.22, 1, 0.36, 1] }
            : { type: "spring", stiffness: 430, damping: 34, mass: 0.8 }
        }
      >
        <motion.div
          className="stext"
          initial={false}
          animate={{ height: textOpen ? "auto" : 0, opacity: textOpen ? 1 : 0 }}
          transition={{ duration: leaving ? 0.26 : 0.34, ease: [0.22, 1, 0.36, 1] }}
        >
          <div className="stext-clip">
            <div
              ref={textCapRef}
              className={`stext-cap ${overflowing ? "is-overflowing" : ""}`}
            >
              <p className={transcript ? "" : "is-empty"}>
                {transcript || transcriptPlaceholder(phase)}
                {phase === "recording" && <span className="scaret" />}
              </p>
            </div>
          </div>
        </motion.div>

        <div className="sbase">
          <div className="sbase-l">
            <span className="sdot" aria-hidden="true" />
            <span className="sspinner" aria-hidden="true" />
          </div>

          <div className="swave" aria-hidden="true">
            {Array.from({ length: WAVE_BARS }, (_, index) => (
              <i key={index} style={{ height: `${barHeight(levels[index])}px` }} />
            ))}
          </div>

          <span className="swork-label">{label}</span>

          <div className="sbase-r">
            <span className="stimer">
              {phase === "recording" ? formatElapsed(elapsed) : ""}
            </span>
          </div>
        </div>
      </motion.section>
    </main>
  );
}

function normalizeStatus(payload = {}) {
  return {
    phase: payload.phase || "idle",
    message: payload.message || "",
    lastText: payload.lastText || "",
  };
}

function rowModeForPhase(phase, open) {
  if (phase === "recording") return "listening";
  if (phase === "idle" && open) return "done";
  if (phase === "error") return "done";
  return "working";
}

function rowModeClass(mode) {
  if (mode === "working") return "is-working";
  if (mode === "done") return "is-done";
  return "";
}

function transcriptPlaceholder(phase) {
  if (phase === "recording") return "Listening...";
  if (phase === "recognizing" || phase === "stopping") return "Waiting for result...";
  if (phase === "typing") return "Typing into target...";
  if (phase === "error") return "Something went wrong.";
  return "Waiting for speech...";
}

function formatElapsed(seconds) {
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}

function barHeight(level = 0) {
  return Math.max(4, Math.min(18, 4 + level * 16));
}

const rootElement = document.querySelector("#root");
if (!rootElement) {
  document.body.textContent = "Dou Voice overlay failed to initialize: missing root element.";
  throw new Error("missing #root element");
}

createRoot(rootElement).render(<RecordingOverlay />);
