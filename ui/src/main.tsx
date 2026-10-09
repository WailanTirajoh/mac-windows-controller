import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";

// `?mock` menjalankan UI di browser biasa dengan backend tiruan (hanya saat development).
if (import.meta.env.DEV && new URLSearchParams(window.location.search).has("mock")) {
  const { installMock } = await import("./dev/mock");
  installMock();
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
