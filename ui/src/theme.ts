import { createSignal } from "solid-js";

export type Mode = "system" | "light" | "dark";

const KEY = "microtel.theme";
const mq = window.matchMedia("(prefers-color-scheme: dark)");

const saved = localStorage.getItem(KEY);
const [mode, setModeSig] = createSignal<Mode>(saved === "light" || saved === "dark" ? saved : "system");
const [sysDark, setSysDark] = createSignal(mq.matches);
mq.addEventListener("change", (e) => {
  setSysDark(e.matches);
  apply();
});

export { mode };
export const resolved = (): "light" | "dark" =>
  mode() === "system" ? (sysDark() ? "dark" : "light") : (mode() as "light" | "dark");

function apply() {
  const m = mode();
  if (m === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = m;
}

export function setMode(m: Mode) {
  setModeSig(m);
  if (m === "system") localStorage.removeItem(KEY);
  else localStorage.setItem(KEY, m);
  apply();
}

export function cycleMode() {
  setMode(mode() === "system" ? "light" : mode() === "light" ? "dark" : "system");
}

apply();
