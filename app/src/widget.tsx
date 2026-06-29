import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { ActivityWidget } from "./components/ActivityWidget";
import "./styles.css";

/**
 * Entry point for the floating mini-player window. Renders the *same*
 * `ActivityWidget` as the in-app dock, in `window` variant. Opening a PR routes
 * back to the main window via a Tauri command (this window can't drive the main
 * window's React state directly).
 */
function openInMain(prRef: string) {
  invoke("open_pr_in_main", { prRef }).catch(() => {});
}

ReactDOM.createRoot(document.getElementById("widget-root")!).render(
  <React.StrictMode>
    <ActivityWidget variant="window" onOpenPr={openInMain} />
  </React.StrictMode>
);
