import React, { lazy, Suspense } from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
const DesktopTerminalWindow = lazy(() => import("./DesktopTerminalWindow"));
const desktopTerminal = new URLSearchParams(window.location.search).has("desktopTerminal");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {desktopTerminal ? <Suspense fallback={null}><DesktopTerminalWindow /></Suspense> : <App />}
  </React.StrictMode>,
);
