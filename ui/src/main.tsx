import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import type { Connection } from "./api";
import { App } from "./App";
import "./styles.css";

// The Tauri shell's `server` command: the Connection, once the sidecar's `serve` printed its address and token.
const connect = () => invoke<Connection>("server");

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
    <App connect={connect} appWindow={getCurrentWindow()} />
  </StrictMode>,
);
