<script setup lang="ts">
import type { Connection } from "~/types"
import { invoke } from "@tauri-apps/api/core"
import { ask, open } from "@tauri-apps/plugin-dialog"

const route = useRoute()
const connectionId = route.params.id

const isNewConnection = connectionId === "new-connection"

const groups: string[] = await invoke<string[]>("get_all_groups")
const engineTypes: string[] = await invoke<string[]>("get_all_engine_types")

const isConnectionEdited = ref<boolean>(false)

const serverObject: Connection =
  isNewConnection
    ? await invoke<Connection>("get_default_connectionentry")
    : await invoke<Connection>("load_single_connection", {
        connectionId: connectionId,
      })

const server = ref<Connection>(serverObject)

watch(
  server,
  () => (isConnectionEdited.value = true),
  { deep: true },
)

const errorMessage = ref<string | null>(null)

const presetIcons = await invoke<{ name: string; data: string }[]>("list_preset_icons")

// Full local Phosphor collection (bundled, no network) for the search picker.
const phCollection = (await import("@iconify-json/ph")).icons as {
  icons: Record<string, { body: string }>
}

const iconSearch = ref("")
const selectedGlyph = ref<string | null>(null)
const badgeColors = [
  "#E5484D", "#F76B15", "#FFB224", "#30A46C", "#12A594", "#00A2C7",
  "#0091FF", "#3E63DD", "#6E56CF", "#8E4EC6", "#CA244D", "#64748B",
]
const selectedColor = ref(badgeColors[7])

const searchResults = computed(() => {
  const q = iconSearch.value.trim().toLowerCase()
  if (q.length < 2) return []
  return Object.keys(phCollection.icons).filter((n) => n.includes(q)).slice(0, 48)
})

const glyphSvg = (name: string) => {
  const body = phCollection.icons[name]?.body ?? ""
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="20" height="20">${body}</svg>`
}

const composeAndSave = async (glyphName: string) => {
  iconError.value = null
  try {
    const body = (phCollection.icons[glyphName]?.body ?? "").split("currentColor").join("#ffffff")
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
      connection_id: server.value.id,
      png_base64: dataUrl.split(",")[1],
    })
    server.value.iconPath = path
    iconPreview.value = dataUrl
    selectedGlyph.value = glyphName
  } catch (e) {
    iconError.value = `Could not create icon: ${e}`
  }
}

const pickColor = async (c: string) => {
  selectedColor.value = c
  if (selectedGlyph.value) await composeAndSave(selectedGlyph.value)
}

const iconPreview = ref<string | null>(null)
const iconError = ref<string | null>(null)

const isCustomIcon = computed(
  () => !!server.value.iconPath && !server.value.iconPath.startsWith("preset:"),
)

const loadIconPreview = async () => {
  iconError.value = null
  if (!isCustomIcon.value) {
    iconPreview.value = null
    return
  }
  try {
    iconPreview.value = await invoke<string>("read_icon_preview", { path: server.value.iconPath })
  } catch (e) {
    iconPreview.value = null
    iconError.value = `Could not preview icon: ${e}`
  }
}
await loadIconPreview()

const selectPreset = (name: string) => {
  server.value.iconPath = name === "default" ? null : `preset:${name}`
  iconPreview.value = null
  iconError.value = null
  selectedGlyph.value = null
}

const isSelectedPreset = (name: string) =>
  name === "default"
    ? !server.value.iconPath
    : server.value.iconPath === `preset:${name}`

const handlePickIcon = async () => {
  const picked = await open({
    multiple: false,
    filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "gif"] }],
  })
  if (typeof picked === "string") {
    server.value.iconPath = picked
    await loadIconPreview()
  }
}

const handleSave = async () => {
  try {
    await invoke("save", { ce: JSON.stringify(server.value) })
    navigateTo("/")
  } catch (e) {
    errorMessage.value = `Save failed: ${e}`
  }
}

const handleCancel = async () => {
  if (isConnectionEdited.value) {
    const confirmed = await ask(
      "You have unsaved changes. Discard them?",
      { title: "Discard changes?", kind: "warning" },
    )
    if (!confirmed) return
  }
  navigateTo("/")
}

const handleDelete = async () => {
  const confirmed = await ask(
    `Do you want to delete connection ${server.value.name}?`,
    { title: "Are you sure?", kind: "warning" },
  )
  if (!confirmed) return

  try {
    await invoke("delete", { id: server.value.id })
    navigateTo("/")
  } catch (e) {
    errorMessage.value = `Delete failed: ${e}`
  }
}
</script>

<template>
  <div class="bg-surface-0 flex flex-col h-full overflow-hidden">
    <!-- Header -->
    <div class="px-5 pt-5 pb-4">
      <h1 class="font-semibold text-lg text-text-primary">
        {{ isNewConnection ? "New Connection" : "Edit Connection" }}
      </h1>
    </div>

    <!-- Scrollable form area -->
    <div class="flex-1 overflow-y-auto px-5 pb-24">
      <form class="grid grid-cols-2 gap-x-8 gap-y-6" @submit.prevent>
        <!-- Left column: Connection -->
        <section class="space-y-3">
          <h2 class="text-xs font-medium text-text-tertiary uppercase tracking-wider">Connection</h2>
          <connection-input type="text" label="Name" placeholder="My Server" v-model="server.name" />
          <connection-input type="text" label="Address" placeholder="https://hostname:8443" v-model="server.address" />
          <div class="space-y-1">
            <label class="block text-sm font-medium text-text-secondary select-none">Engine Type</label>
            <insertable-dropdown :options="engineTypes" v-model="server.engineType" />
          </div>
          <div class="space-y-2 pt-1">
            <p class="text-sm font-medium text-text-secondary select-none">Security</p>
            <template v-if="server.pinnedCertSha256">
              <p class="text-xs text-text-tertiary select-none">Trusted certificate (SHA-256)</p>
              <div class="flex items-start gap-2">
                <p
                  class="flex-1 font-mono text-xs bg-surface-2 rounded-md px-3 py-2 text-text-secondary break-all leading-relaxed"
                >
                  {{ server.pinnedCertSha256 }}
                </p>
                <button
                  type="button"
                  class="px-2.5 py-1.5 rounded-md text-xs text-danger hover:bg-danger/10 hover:cursor-pointer transition-colors whitespace-nowrap"
                  @click="server.pinnedCertSha256 = null"
                >
                  Forget
                </button>
              </div>
            </template>
            <p v-else class="text-xs text-text-tertiary select-none">
              No certificate trusted yet — you'll be asked to trust one on first connect.
            </p>
          </div>
        </section>

        <!-- Right column: Java -->
        <section class="space-y-3">
          <h2 class="text-xs font-medium text-text-tertiary uppercase tracking-wider">Configuration</h2>
          <connection-input type="text" label="Java Home" placeholder="/usr/lib/jvm/java-11" hint="Requires a JavaFX-enabled JDK" v-model="server.javaHome" />
          <div class="space-y-1">
            <label class="block text-sm font-medium text-text-secondary select-none">JVM Arguments</label>
            <textarea
              class="w-full bg-surface-1 border border-border rounded-md px-2.5 py-1.5 text-sm text-text-primary placeholder:text-text-disabled outline-none transition-colors duration-100 focus:border-border-focus focus:ring-1 focus:ring-accent/30 resize-y min-h-16"
              placeholder="Additional JVM options"
              v-model="server.javaArgs"
            ></textarea>
          </div>
          <div class="space-y-1.5">
            <label class="block text-sm font-medium text-text-secondary select-none">Admin Icon</label>
            <div class="flex flex-wrap items-center gap-1.5">
              <button
                v-for="icon in presetIcons"
                :key="icon.name"
                type="button"
                :title="icon.name"
                class="rounded-lg p-0.5 hover:cursor-pointer transition-all duration-100"
                :class="isSelectedPreset(icon.name) ? 'ring-2 ring-accent' : 'opacity-80 hover:opacity-100'"
                @click="selectPreset(icon.name)"
              >
                <img :src="icon.data" class="w-8 h-8 rounded-md" :alt="icon.name" />
              </button>
              <button
                type="button"
                title="Choose an image file"
                class="rounded-lg p-0.5 hover:cursor-pointer transition-all duration-100"
                :class="isCustomIcon ? 'ring-2 ring-accent' : 'opacity-80 hover:opacity-100'"
                @click="handlePickIcon"
              >
                <img v-if="iconPreview" :src="iconPreview" class="w-8 h-8 rounded-md" alt="Custom icon" />
                <span
                  v-else
                  class="flex items-center justify-center w-8 h-8 rounded-md border border-dashed border-border text-text-tertiary text-lg leading-none select-none"
                >…</span>
              </button>
            </div>
            <input
              v-model="iconSearch"
              type="text"
              placeholder="Search all icons…"
              class="w-full bg-surface-1 border border-border rounded-md px-2.5 py-1.5 text-sm text-text-primary placeholder:text-text-disabled outline-none transition-colors duration-100 focus:border-border-focus focus:ring-1 focus:ring-accent/30"
            />
            <div v-if="searchResults.length" class="flex flex-wrap gap-1 max-h-28 overflow-y-auto text-text-primary">
              <button
                v-for="n in searchResults"
                :key="n"
                type="button"
                :title="n"
                class="flex items-center justify-center size-8 rounded-md hover:bg-surface-2 hover:cursor-pointer transition-colors"
                :class="selectedGlyph === n ? 'ring-2 ring-accent' : ''"
                @click="composeAndSave(n)"
                v-html="glyphSvg(n)"
              />
            </div>
            <p v-else-if="iconSearch.trim().length >= 2" class="text-xs text-text-tertiary select-none">No icons match</p>
            <div v-if="searchResults.length || selectedGlyph" class="flex flex-wrap items-center gap-1.5">
              <button
                v-for="c in badgeColors"
                :key="c"
                type="button"
                class="size-5 rounded-full hover:cursor-pointer transition-all"
                :class="selectedColor === c ? 'ring-2 ring-accent ring-offset-1' : 'opacity-80 hover:opacity-100'"
                :style="{ backgroundColor: c }"
                @click="pickColor(c)"
              />
            </div>
            <p v-if="iconError" class="text-xs text-danger">{{ iconError }}</p>
            <p class="text-xs text-text-tertiary select-none">Dock/taskbar icon for this connection's administrator</p>
          </div>
        </section>

        <!-- Left column: Authentication -->
        <section class="space-y-3">
          <h2 class="text-xs font-medium text-text-tertiary uppercase tracking-wider">Authentication</h2>
          <connection-input type="text" label="Username" placeholder="admin" v-model="server.username" />
          <connection-input type="password" label="Password" v-model="server.password" />
        </section>

        <!-- Right column: Group, Notes, Options -->
        <section class="space-y-3">
          <h2 class="text-xs font-medium text-text-tertiary uppercase tracking-wider">Organization</h2>
          <div class="space-y-1">
            <label class="block text-sm font-medium text-text-secondary select-none">Group</label>
            <insertable-dropdown :options="groups" v-model="server.group" />
          </div>
          <connection-input type="text" label="Heap Size" placeholder="512m" v-model="server.heapSize" />
          <connection-input type="text" label="Notes" placeholder="Optional notes" v-model="server.notes" />
          <div class="space-y-2 pt-1">
            <p class="text-sm font-medium text-text-secondary select-none">Options</p>
            <label class="flex items-center gap-2 text-sm text-text-primary hover:cursor-pointer select-none">
              <input type="checkbox" class="accent-accent" v-model="server.showConsole" />
              Show console
            </label>
            <label class="flex items-center gap-2 text-sm text-text-primary hover:cursor-pointer select-none">
              <input type="checkbox" class="accent-accent" v-model="server.donotcache" />
              Do not cache
            </label>
          </div>
        </section>
      </form>
    </div>

    <!-- Error message -->
    <div v-if="errorMessage" class="flex-none px-5 py-2 bg-danger/10 border-t border-danger/30">
      <p class="text-sm text-danger">{{ errorMessage }}</p>
    </div>

    <!-- Action bar -->
    <div class="flex-none flex items-center justify-between px-5 py-3 border-t border-border bg-surface-0">
      <button
        @click="handleCancel"
        class="px-3 py-1.5 text-sm rounded-md text-text-secondary hover:bg-surface-2 hover:cursor-pointer transition-colors duration-100"
      >
        Cancel
      </button>
      <div class="flex items-center gap-2">
        <button
          v-if="!isNewConnection"
          @click="handleDelete"
          class="px-3 py-1.5 text-sm rounded-md text-danger hover:bg-danger/10 hover:cursor-pointer transition-colors duration-100"
        >
          Delete
        </button>
        <button
          :disabled="!isConnectionEdited"
          @click="handleSave"
          class="px-4 py-1.5 text-sm rounded-md bg-accent text-white hover:bg-accent-hover hover:cursor-pointer transition-colors duration-100 disabled:opacity-40 disabled:cursor-not-allowed"
        >
          {{ isNewConnection ? "Create" : "Save" }}
        </button>
      </div>
    </div>
  </div>
</template>
