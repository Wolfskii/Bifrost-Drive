import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { TransferWindow } from "./TransferWindow";
import "./styles.css";

// The same bundle serves both windows; the transfer window is opened with ?window=transfers.
const transfersOnly =
    new URLSearchParams(window.location.search).get("window") === "transfers";
if (transfersOnly) {
    document.documentElement.classList.add("transfer-root");
}

ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
        {transfersOnly ? <TransferWindow /> : <App />}
    </React.StrictMode>,
);
