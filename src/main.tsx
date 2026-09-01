import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";

if (new URLSearchParams(window.location.search).get("overlay") === "1") {
  document.documentElement.classList.add("overlay-document");
}

createRoot(document.getElementById("root")!).render(<StrictMode><App /></StrictMode>);
