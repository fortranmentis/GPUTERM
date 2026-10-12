import React from "react";
import ReactDOM from "react-dom/client";
import "@xterm/xterm/css/xterm.css";
import "./styles.css";
import App from "./App";
import { DetailWindow } from "./components/DetailWindow";
import { ErrorBoundary } from "./components/ErrorBoundary";

const params = new URLSearchParams(window.location.search);
const isDetailWindow = params.get("window") === "detail";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>{isDetailWindow ? <DetailWindow /> : <App />}</ErrorBoundary>
  </React.StrictMode>,
);
