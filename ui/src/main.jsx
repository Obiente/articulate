import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App.jsx";
import { Overlay } from "./Overlay.jsx";
import { MotionShell, dialogTransition } from "./motion.jsx";
import { MantineProvider, createTheme } from "@mantine/core";
import { Notifications } from "@mantine/notifications";
import "@mantine/core/styles.css";
import "@mantine/notifications/styles.css";
import "./styles.css";
import "./motion.css";

const theme = createTheme({
  fontFamily: "Inter, Segoe UI, sans-serif",
  headings: { fontFamily: "Inter, Segoe UI, sans-serif", fontWeight: "500" },
  primaryColor: "mint",
  primaryShade: 3,
  defaultRadius: "md",
  autoContrast: true,
  respectReducedMotion: true,
  colors: {
    mint: [
      "#edfcf6",
      "#d3f7e7",
      "#b4edda",
      "#93dfc2",
      "#73c7aa",
      "#55af90",
      "#369477",
      "#23765e",
      "#195e4b",
      "#12493a",
    ],
    dark: [
      "#e9eeeb",
      "#c4d0ca",
      "#a0b2a9",
      "#647c6e",
      "#3a4d42",
      "#283930",
      "#202d26",
      "#18221e",
      "#131c17",
      "#0e1611",
    ],
  },
  components: {
    Button: { defaultProps: { size: "sm", fw: 500 } },
    TextInput: { defaultProps: { size: "md" } },
    Textarea: { defaultProps: { size: "md" } },
    Select: { defaultProps: { size: "md" } },
    Modal: {
      defaultProps: {
        centered: true,
        radius: "lg",
        padding: "xl",
        closeButtonProps: { "aria-label": "Close dialog" },
        transitionProps: dialogTransition,
        overlayProps: { backgroundOpacity: 0.65, blur: 4 },
      },
    },
    Tooltip: { defaultProps: { openDelay: 450, withArrow: true } },
    Menu: {
      defaultProps: {
        transitionProps: {
          transition: "pop-top-right",
          duration: 140,
          exitDuration: 90,
        },
      },
    },
  },
});
createRoot(document.getElementById("root")).render(
  <React.StrictMode>
    <MotionShell>
      <MantineProvider theme={theme} forceColorScheme="dark">
        {!window.__ARTICULATE_OVERLAY__ && (
          <Notifications position="bottom-right" />
        )}
        {window.__ARTICULATE_OVERLAY__ ? <Overlay /> : <App />}
      </MantineProvider>
    </MotionShell>
  </React.StrictMode>,
);
