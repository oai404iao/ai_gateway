import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AppProviders } from "@/app/providers";
import { AppRouterProvider } from "@/app/router";
import "@/index.css";

const root = document.getElementById("root");
if (!root) throw new Error("Root element #root not found");

createRoot(root).render(
  <StrictMode>
    <AppProviders>
      <AppRouterProvider />
    </AppProviders>
  </StrictMode>,
);
