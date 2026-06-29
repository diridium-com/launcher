import tailwindcss from "@tailwindcss/vite"

export default defineNuxtConfig({
  compatibilityDate: "2025-05-15",

  devtools: { enabled: true },

  ssr: false,

  devServer: {
    host: "0",
  },

  app: {
    head: {
      script: [
        {
          // Apply the correct theme before first paint to avoid a flash. Console
          // windows have their own theme (launcher-console-theme, falling back to
          // the shared theme); every other window uses the shared theme. The
          // window label is read from Tauri's internals, which are injected
          // before this script runs. Must mirror useConsoleTheme/useTheme.
          innerHTML: `(function () {
  try {
    var m = window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.metadata
    var label = (m && m.currentWebview && m.currentWebview.label) ||
                (m && m.currentWindow && m.currentWindow.label) || ""
    var isConsole = label.indexOf("console-") === 0
    var t = localStorage.getItem(isConsole ? "launcher-console-theme" : "launcher-theme")
    if (isConsole && !t) t = localStorage.getItem("launcher-theme")
    document.documentElement.setAttribute("data-theme", t === "light" ? "light" : "dark")
  } catch (e) {}
})()`,
          type: "text/javascript",
        },
      ],
    },
    pageTransition: {
      name: "slide", // we'll define CSS for "slide"
      mode: "out-in", // waits for leave before enter
    },
  },

  modules: ["@nuxt/icon"],

  css: ["~/assets/css/main.css"],

  vite: {
    plugins: [tailwindcss()],

    clearScreen: false,
    envPrefix: ["VITE_", "TAURI_"],
    server: {
      strictPort: true,
    },
    // Pre-bundle the Tauri modules so Vite doesn't discover them mid-load and
    // force a reload (which surfaces as a transient 500 in the webview on the
    // first dev launch after they were added).
    optimizeDeps: {
      include: [
        "@tauri-apps/api/core",
        "@tauri-apps/api/webviewWindow",
        "@tauri-apps/plugin-dialog",
        "@tauri-apps/plugin-clipboard-manager",
      ],
    },
  },
  ignore: ["**/src-tauri/**"],
})
