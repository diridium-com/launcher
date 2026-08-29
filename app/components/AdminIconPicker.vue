<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core"
import { open } from "@tauri-apps/plugin-dialog"

const props = defineProps<{ connectionId: string }>()

// An absolute path to a composed PNG, or null for the bundled default.
// Legacy `preset:<name>` values from before icons became recolorable still
// resolve on the Rust side; picking anything here replaces them.
const iconPath = defineModel<string | null>("iconPath")
// What iconPath was composed from, so the picker can restore the selection
// and recolor it later. Both null for a hand-picked image file.
const iconGlyph = defineModel<string | null>("iconGlyph")
const iconColor = defineModel<string | null>("iconColor")

// Curated starting set, shown when the search box is empty. These are
// Phosphor names; the Rust side keeps its own copy of the pre-recolor list
// purely to resolve legacy `preset:` values.
const PRESET_GLYPHS = [
  "heartbeat-fill", "stethoscope-fill", "shield-check-fill", "database-fill",
  "plug-fill", "cloud-fill", "globe-fill", "gear-fill", "rocket-fill",
  "flask-fill", "bug-fill", "lightning-fill",
]

const BADGE_COLORS = [
  "#E5484D", "#F76B15", "#FFB224", "#30A46C", "#12A594", "#00A2C7",
  "#0091FF", "#3E63DD", "#6E56CF", "#8E4EC6", "#CA244D", "#64748B",
]

const isOpen = ref(false)
const iconSearch = ref("")
const iconError = ref<string | null>(null)
// A connection saved before icons became recolorable carries `preset:<name>`
// and no glyph. Seed the selection from it so the color swatches act on it
// immediately; the first recolor replaces it with a composed icon.
const legacyPreset = iconPath.value?.startsWith("preset:")
  ? `${iconPath.value.slice("preset:".length)}-fill`
  : null
const selectedGlyph = ref<string | null>(iconGlyph.value ?? legacyPreset)
const selectedColor = ref<string>(iconColor.value ?? BADGE_COLORS[11]!)

// Data URI shown on the trigger. Resolved by the same Rust path the launch
// and the main screen use, so the swatch can't disagree with what launches.
const currentIconData = ref<string | null>(null)
const defaultIconData = ref<string | null>(null)

const refreshSwatch = async () => {
  try {
    currentIconData.value = await invoke<string>("get_connection_icon", {
      icon_path: iconPath.value,
    })
  } catch (e) {
    currentIconData.value = null
    iconError.value = `Could not load icon: ${e}`
  }
}
defaultIconData.value = await invoke<string>("get_connection_icon", { icon_path: null })
await refreshSwatch()

// ~9k glyphs, so the set is pulled in on first open rather than at page load.
// Bundled locally; the CSP allows no CDN.
type PhIcons = Record<string, { body: string }>
const phIcons = ref<PhIcons | null>(null)
const loadGlyphs = async () => {
  if (!phIcons.value) {
    phIcons.value = ((await import("@iconify-json/ph")).icons as { icons: PhIcons }).icons
  }
}

// Fill weight only: the badges are a solid glyph knocked out of a colored
// square, and outline weights read as a different icon set entirely.
const searchResults = computed(() => {
  const q = iconSearch.value.trim().toLowerCase()
  if (q.length < 2 || !phIcons.value) return []
  const out: string[] = []
  for (const name of Object.keys(phIcons.value)) {
    if (!name.endsWith("-fill") || !name.slice(0, -5).includes(q)) continue
    out.push(name)
    if (out.length >= 60) break
  }
  return out
})

const shownGlyphs = computed(() =>
  iconSearch.value.trim().length >= 2 ? searchResults.value : PRESET_GLYPHS,
)

const glyphSvg = (name: string) => {
  const body = phIcons.value?.[name]?.body ?? ""
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="20" height="20">${body}</svg>`
}

const glyphLabel = (name: string) => name.slice(0, -5).replace(/-/g, " ")

// Draws the glyph onto a rounded colored square at the geometry used by
// src-tauri/tools/mkbadge.py, then hands the PNG to Rust to store.
const composeAndSave = async (glyphName: string) => {
  iconError.value = null
  try {
    await loadGlyphs()
    const body = (phIcons.value?.[glyphName]?.body ?? "").split("currentColor").join("#ffffff")
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256">${body}</svg>`
    const img = new Image()
    const loaded = new Promise((resolve, reject) => {
      img.onload = resolve
      img.onerror = reject
    })
    img.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`
    await loaded

    const canvas = document.createElement("canvas")
    canvas.width = 256
    canvas.height = 256
    const ctx = canvas.getContext("2d")!
    ctx.beginPath()
    ctx.roundRect(8, 8, 240, 240, 58)
    ctx.fillStyle = selectedColor.value
    ctx.fill()
    ctx.drawImage(img, 48, 48, 160, 160)
    const dataUrl = canvas.toDataURL("image/png")

    const path = await invoke<string>("save_connection_icon", {
      connection_id: props.connectionId,
      png_base64: dataUrl.split(",")[1],
    })
    selectedGlyph.value = glyphName
    iconPath.value = path
    iconGlyph.value = glyphName
    iconColor.value = selectedColor.value
    // Same path every time, so the swatch is set from the fresh bytes rather
    // than re-read through a URL the webview would serve from cache.
    currentIconData.value = dataUrl
  } catch (e) {
    iconError.value = `Could not create icon: ${e}`
  }
}

const pickColor = async (c: string) => {
  selectedColor.value = c
  if (selectedGlyph.value) await composeAndSave(selectedGlyph.value)
}

const useDefault = async () => {
  iconPath.value = null
  iconGlyph.value = null
  iconColor.value = null
  selectedGlyph.value = null
  iconError.value = null
  await refreshSwatch()
  isOpen.value = false
}

const isDefault = computed(() => !iconPath.value)

const handlePickFile = async () => {
  const picked = await open({
    multiple: false,
    filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "gif"] }],
  })
  if (typeof picked === "string") {
    iconPath.value = picked
    iconGlyph.value = null
    iconColor.value = null
    selectedGlyph.value = null
    await refreshSwatch()
    isOpen.value = false
  }
}

// Window-level so it fires regardless of where focus sits. The page's own
// Escape handler defers to the `data-popover-open` marker below, so the first
// Escape closes this popover and a second one cancels the edit.
const onEsc = (e: KeyboardEvent) => {
  if (e.key === "Escape" && isOpen.value) isOpen.value = false
}
onMounted(() => window.addEventListener("keydown", onEsc))
onBeforeUnmount(() => window.removeEventListener("keydown", onEsc))

const toggle = async () => {
  isOpen.value = !isOpen.value
  if (isOpen.value) await loadGlyphs()
}
</script>

<template>
  <div class="space-y-1.5">
    <label class="block text-sm font-medium text-text-secondary select-none">Admin Icon</label>

    <div class="relative">
      <button
        type="button"
        class="flex items-center gap-2 rounded-md border border-border bg-surface-1 p-1 pr-2 hover:cursor-pointer transition-colors duration-100"
        :class="isOpen ? 'border-border-focus ring-1 ring-accent/30' : ''"
        @click="toggle"
      >
        <img v-if="currentIconData" :src="currentIconData" class="size-8 rounded-md" alt="Admin icon" />
        <span
          v-else
          class="block size-8 rounded-md border border-dashed border-border"
        />
        <icon
          name="ph:caret-down"
          class="text-sm text-text-tertiary transition-transform duration-150"
          :class="isOpen ? 'rotate-180' : ''"
        />
      </button>

      <!-- Click anywhere outside to dismiss. -->
      <div v-if="isOpen" class="fixed inset-0 z-10" @click="isOpen = false" />

      <Transition
        enter-active-class="transition duration-100 ease-out"
        enter-from-class="opacity-0 scale-95"
        enter-to-class="opacity-100 scale-100"
        leave-active-class="transition duration-75 ease-in"
        leave-from-class="opacity-100 scale-100"
        leave-to-class="opacity-0 scale-95"
      >
        <div
          v-if="isOpen"
          data-popover-open
          class="absolute z-20 mt-1 w-72 bg-surface-1 border border-border rounded-md shadow-md p-2 space-y-2"
        >
          <div class="flex items-center gap-1.5">
            <input
              v-model="iconSearch"
              type="text"
              placeholder="Search icons…"
              class="flex-1 min-w-0 bg-surface-0 border border-border rounded px-2 py-1 text-sm text-text-primary placeholder:text-text-disabled outline-none focus:border-border-focus focus:ring-1 focus:ring-accent/30"
            />
            <button
              type="button"
              title="Close"
              aria-label="Close icon picker"
              class="flex items-center justify-center size-6 shrink-0 rounded text-text-tertiary hover:bg-surface-2 hover:text-text-primary hover:cursor-pointer transition-colors"
              @click="isOpen = false"
            >
              <icon name="ph:x" class="text-sm" />
            </button>
          </div>

          <!-- Step 1: pick a shape. Unpainted, so the grid reads as a set of
               choices rather than as one color repeated twelve times. -->
          <div v-if="shownGlyphs.length" class="grid grid-cols-6 gap-1 max-h-40 overflow-y-auto">
            <button
              v-for="g in shownGlyphs"
              :key="g"
              type="button"
              :title="glyphLabel(g)"
              class="flex items-center justify-center size-8 rounded-md hover:bg-surface-2 hover:cursor-pointer transition-colors"
              :class="selectedGlyph === g ? 'ring-2 ring-accent text-accent' : 'text-text-secondary hover:text-text-primary'"
              @click="composeAndSave(g)"
              v-html="glyphSvg(g)"
            />
          </div>
          <p v-else class="text-xs text-text-tertiary select-none py-1">No icons match</p>

          <!-- Step 2: paint it. Only meaningful once a shape is selected, so
               it stays out of the way until then. -->
          <!-- Step 2: paint it. Present from the start so the panel keeps a
               stable height, but inert until there is a shape to paint. -->
          <div
            class="border-t border-border pt-2 flex items-center gap-2 transition-opacity duration-150"
            :class="selectedGlyph ? '' : 'opacity-40'"
          >
            <img
              v-if="selectedGlyph && currentIconData"
              :src="currentIconData"
              class="size-9 rounded-md shrink-0"
              alt="Selected icon"
            />
            <span
              v-else
              class="size-9 rounded-md shrink-0 border border-dashed border-border"
            />
            <div class="flex flex-wrap items-center gap-1.5">
              <button
                v-for="c in BADGE_COLORS"
                :key="c"
                type="button"
                :disabled="!selectedGlyph"
                :title="selectedGlyph ? `Recolor ${glyphLabel(selectedGlyph)} ${c}` : 'Pick an icon first'"
                class="size-5 rounded-full transition-all disabled:cursor-default enabled:hover:cursor-pointer"
                :class="selectedGlyph && selectedColor === c ? 'ring-2 ring-accent ring-offset-1' : 'opacity-80 enabled:hover:opacity-100'"
                :style="{ backgroundColor: c }"
                @click="pickColor(c)"
              />
            </div>
          </div>

          <div class="border-t border-border pt-2 space-y-0.5">
            <button
              type="button"
              class="w-full flex items-center gap-2 text-left px-1 py-1 rounded text-sm hover:bg-surface-2 hover:cursor-pointer transition-colors"
              :class="isDefault ? 'text-accent' : 'text-text-secondary'"
              @click="useDefault"
            >
              <img v-if="defaultIconData" :src="defaultIconData" class="size-5 rounded shrink-0" alt="" />
              Use default icon
            </button>
            <button
              type="button"
              class="w-full text-left px-1 py-1 rounded text-sm text-text-secondary hover:bg-surface-2 hover:cursor-pointer transition-colors"
              @click="handlePickFile"
            >
              Choose image file…
            </button>
          </div>
        </div>
      </Transition>
    </div>

    <p v-if="iconError" class="text-xs text-danger">{{ iconError }}</p>
    <p class="text-xs text-text-tertiary select-none">
      Dock/taskbar icon for this connection's administrator
    </p>
  </div>
</template>
