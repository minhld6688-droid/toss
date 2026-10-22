import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "@fontsource-variable/bricolage-grotesque/standard.css";
import "@fontsource-variable/fraunces/full.css";
import "@fontsource-variable/fraunces/full-italic.css";
import "@fontsource-variable/jetbrains-mono/wght.css";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
