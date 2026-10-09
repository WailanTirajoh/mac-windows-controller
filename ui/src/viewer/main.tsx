import { StrictMode, useEffect, useRef } from "react";
import { createRoot } from "react-dom/client";
import "../styles.css";
import { runSelftest } from "./selftest";

const selftest = new URLSearchParams(window.location.search).has("selftest");

function Viewer() {
  const canvas = useRef<HTMLCanvasElement>(null);
  const started = useRef(false);

  useEffect(() => {
    // StrictMode menjalankan efek dua kali saat development.
    if (!selftest || started.current || canvas.current === null) return;
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
  <StrictMode>
    <Viewer />
  </StrictMode>,
);
