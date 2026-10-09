import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";

function App() {
  return (
    <main className="flex h-full items-center justify-center bg-neutral-50 text-neutral-900 dark:bg-neutral-900 dark:text-neutral-100">
      <h1 className="text-2xl font-semibold">Sambung</h1>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
