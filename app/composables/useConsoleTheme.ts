// Copyright (c) Diridium Technologies Inc. All rights reserved.
// Licensed under the MPL-2.0 License. See LICENSE file in the project root.

import { ref } from "vue"

type Theme = "dark" | "light"

const STORAGE_KEY = "launcher-console-theme"
const SHARED_KEY = "launcher-theme"
const DEFAULT_THEME: Theme = "dark"

// Module-level singleton: every console window has its own JS context, so this
// ref is per-window and independent of the main window's theme.
const theme = ref<Theme>(DEFAULT_THEME)

function applyTheme(t: Theme) {
  theme.value = t
  document.documentElement.setAttribute("data-theme", t)
  localStorage.setItem(STORAGE_KEY, t)
}

export function useConsoleTheme() {
  // The console keeps its own theme preference. On first open it falls back to
  // the main app's current theme so it isn't jarring, then tracks independently.
  function init() {
    const saved = localStorage.getItem(STORAGE_KEY) as Theme | null
    const shared = localStorage.getItem(SHARED_KEY) as Theme | null
    const initial = saved ?? shared ?? DEFAULT_THEME
    applyTheme(initial === "light" ? "light" : "dark")
  }

  function toggle() {
    applyTheme(theme.value === "dark" ? "light" : "dark")
  }

  return { theme, toggle, init }
}
