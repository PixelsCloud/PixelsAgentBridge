import React, { lazy, Suspense } from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
const DesktopTerminalWindow = lazy(() => import("./DesktopTerminalWindow"));
const RemoteDesktopWindow = lazy(() => import("./RemoteDesktopWindow"));
const FileManagerWindow = lazy(() => import("./FileManagerWindow"));
const ConfirmationWindow = lazy(() => import("./ConfirmationWindow"));
const confirmation = new URLSearchParams(window.location.search).has("confirmation");
const fileManager = new URLSearchParams(window.location.search).has("fileManager");
const desktopViewer = new URLSearchParams(window.location.search).has("desktopViewer");
const desktopTerminal = new URLSearchParams(window.location.search).has("desktopTerminal");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {confirmation ? <Suspense fallback={null}><ConfirmationWindow /></Suspense> : fileManager ? <Suspense fallback={null}><FileManagerWindow /></Suspense> : desktopViewer ? <Suspense fallback={null}><RemoteDesktopWindow /></Suspense> : desktopTerminal ? <Suspense fallback={null}><DesktopTerminalWindow /></Suspense> : <App />}
  </React.StrictMode>,
);
