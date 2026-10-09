import { StrictMode, useEffect, useRef } from "react";
import { createRoot } from "react-dom/client";
import "../styles.css";
import { runSelftest, runSessionSelftest } from "./selftest";
import { Viewer } from "./Viewer";

const params = new URLSearchParams(window.location.search);
const selftest = params.get("selftest");

/** Halaman uji mandiri video (`?selftest=video`): hanya kanvas, tanpa sesi. */
function Selftest() {
  const canvas = useRef<HTMLCanvasElement>(null);
  const started = useRef(false);

  useEffect(() => {
    // StrictMode menjalankan efek dua kali saat development.
    if (started.current || canvas.current === null) return;
    started.current = true;
    void runSelftest(canvas.current);
  }, []);

  return (
    <main className="flex h-full items-center justify-center bg-black">
      <canvas ref={canvas} className="max-h-full max-w-full" />
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>{selftest === "video" ? <Selftest /> : <Viewer fingerprint={params.get("peer") ?? ""} />}</StrictMode>,
);

// `?selftest=session`: viewer berjalan seperti biasa, lalu diamati dari luar.
if (selftest === "session") void runSessionSelftest();
