import { Channel } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useRef, useState } from "react";
import { errorMessage } from "../api";
import {
  type SessionView,
  onDisplayChanged,
  onSessionEnded,
  viewerInput,
  viewerRequestKeyframe,
  viewerSelectDisplay,
  viewerStart,
} from "./api";
import { H264Decoder } from "./decoder";
import { parseFrame } from "./frame";
import { attachInput } from "./input";

/** Jarak minimum antar permintaan keyframe, supaya perangkat tujuan tidak dibanjiri. */
const KEYFRAME_REQUEST_INTERVAL_MS = 500;

const TOOLBAR_BUTTON =
  "rounded-md px-2.5 py-1 text-xs text-neutral-200 hover:bg-neutral-700 aria-pressed:bg-teal-700 aria-pressed:text-white";

export function Viewer({ fingerprint }: { fingerprint: string }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const started = useRef(false);
  const swapModifiers = useRef(false);
  const [session, setSession] = useState<SessionView | null>(null);
  const [displayId, setDisplayId] = useState<number | null>(null);
  const [swapped, setSwapped] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  // Video: frame dari Rust → decoder → kanvas.
  useEffect(() => {
    // StrictMode menjalankan efek dua kali saat development; sesi cukup dimulai sekali.
    if (started.current || canvas.current === null) return;
    started.current = true;

    const target = canvas.current;
    const context = target.getContext("2d");
    let lastKeyframeRequest = 0;
    const requestKeyframe = () => {
      if (performance.now() - lastKeyframeRequest < KEYFRAME_REQUEST_INTERVAL_MS) return;
      lastKeyframeRequest = performance.now();
      void viewerRequestKeyframe(fingerprint);
    };

    const decoder = new H264Decoder(
      (frame) => {
        if (target.width !== frame.displayWidth || target.height !== frame.displayHeight) {
          target.width = frame.displayWidth;
          target.height = frame.displayHeight;
        }
        context?.drawImage(frame, 0, 0);
        frame.close();
      },
      requestKeyframe,
    );

    const frames = new Channel<ArrayBuffer>();
    frames.onmessage = (buffer) => {
      if (!decoder.decode(parseFrame(buffer))) requestKeyframe();
    };

    viewerStart(fingerprint, frames).then(
      (view) => {
        setSession(view);
        setDisplayId(view.displayId);
      },
      (error) => setProblem(errorMessage(error)),
    );
    void onDisplayChanged(setDisplayId);
    void onSessionEnded(() => setProblem("Sesi berakhir."));
  }, [fingerprint]);

  // Input: hanya kalau perangkat tujuan mengizinkan kontrol dan sesi masih jalan.
  const controllable = session?.inputEnabled === true && problem === null;
  useEffect(() => {
    if (!controllable || canvas.current === null) return;
    return attachInput(canvas.current, (events) => void viewerInput(fingerprint, events), {
      swapModifiers: () => swapModifiers.current,
    });
  }, [controllable, fingerprint]);

  const toggleSwap = () => {
    swapModifiers.current = !swapModifiers.current;
    setSwapped(swapModifiers.current);
  };

  const toggleFullscreen = async () => {
    const current = getCurrentWindow();
    await current.setFullscreen(!(await current.isFullscreen()));
  };

  return (
    <div className="flex h-full flex-col bg-black text-neutral-100">
      <header className="flex h-9 shrink-0 items-center gap-2 bg-neutral-900 px-3">
        <span className="truncate text-sm font-medium">{session?.name ?? "Menghubungkan…"}</span>
        {session && !session.inputEnabled && (
          <span className="rounded bg-amber-600/30 px-2 py-0.5 text-xs text-amber-200">Hanya lihat</span>
        )}
        <div className="flex-1" />
        {session && session.displays.length > 1 && (
          <select
            aria-label="Pilih layar"
            value={displayId ?? session.displayId}
            onChange={(event) => void viewerSelectDisplay(fingerprint, Number(event.target.value))}
            className="rounded-md bg-neutral-800 px-2 py-1 text-xs"
          >
            {session.displays.map((display, index) => (
              <option key={display.id} value={display.id}>
                Layar {index + 1} ({display.width}×{display.height}){display.isPrimary ? " · utama" : ""}
              </option>
            ))}
          </select>
        )}
        {session?.inputEnabled && (
          <button type="button" className={TOOLBAR_BUTTON} aria-pressed={swapped} onClick={toggleSwap}>
            Tukar Cmd ⇄ Ctrl
          </button>
        )}
        <button type="button" className={TOOLBAR_BUTTON} onClick={() => void toggleFullscreen()}>
          Layar penuh
        </button>
        <button type="button" className={TOOLBAR_BUTTON} onClick={() => void getCurrentWindow().close()}>
          Putuskan
        </button>
      </header>

      <main className="relative flex min-h-0 flex-1 items-center justify-center">
        <canvas
          ref={canvas}
          className={`max-h-full max-w-full ${controllable ? "cursor-none" : ""}`}
          // Tanpa ini, menyeret di atas kanvas memicu gestur sentuh/seleksi bawaan.
          style={{ touchAction: "none" }}
        />
        {problem !== null && (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-4 bg-black/80 p-8 text-center">
            <p className="max-w-md text-sm">{problem}</p>
            <button
              type="button"
              className="rounded-lg bg-neutral-200 px-4 py-2 text-sm font-medium text-neutral-900"
              onClick={() => void getCurrentWindow().close()}
            >
              Tutup
            </button>
          </div>
        )}
      </main>
    </div>
  );
}
