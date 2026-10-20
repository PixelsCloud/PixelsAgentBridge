import React, { lazy, Suspense } from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
const DesktopTerminalWindow = lazy(() => import("./DesktopTerminalWindow"));
const RemoteDesktopWindow = lazy(() => import("./RemoteDesktopWindow"));
const desktopViewer = new URLSearchParams(window.location.search).has("desktopViewer");
const desktopTerminal = new URLSearchParams(window.location.search).has("desktopTerminal");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {desktopViewer ? <Suspense fallback={null}><RemoteDesktopWindow /></Suspense> : desktopTerminal ? <Suspense fallback={null}><DesktopTerminalWindow /></Suspense> : <App />}
  </React.StrictMode>,
);
