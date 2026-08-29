<script setup lang="ts">
import type { Connection } from "~/types"
import { invoke } from "@tauri-apps/api/core"
import { ask } from "@tauri-apps/plugin-dialog"

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

// Escape mirrors the Cancel button, including its unsaved-changes prompt.
// An open popover owns Escape first: the marker is still in the DOM when this
// runs, since Vue flushes the close on the next tick.
const isCancelling = ref(false)
const onKeydown = async (e: KeyboardEvent) => {
  if (e.key !== "Escape" || isCancelling.value) return
  if (document.querySelector("[data-popover-open]")) return
  isCancelling.value = true
  try {
    await handleCancel()
  } finally {
    isCancelling.value = false
  }
}
onMounted(() => window.addEventListener("keydown", onKeydown))
onBeforeUnmount(() => window.removeEventListener("keydown", onKeydown))

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
              <div class="flex items-center gap-2">
                <span class="text-xs text-text-tertiary select-none shrink-0">SHA-256</span>
                <!-- A readonly input, not a <p>: main.css disables user-select
                     on body and exempts only inputs, and this value has to be
                     copyable to be verified out-of-band. Clicking selects the
                     whole hash; it also scrolls, so the truncation hides
                     nothing. -->
                <input
                  type="text"
                  readonly
                  spellcheck="false"
                  :value="server.pinnedCertSha256"
                  :title="server.pinnedCertSha256"
                  class="flex-1 min-w-0 font-mono text-xs bg-surface-2 rounded-md px-2 py-1 text-text-secondary outline-none cursor-text"
                  @focus="($event.target as HTMLInputElement).select()"
                />
                <button
                  type="button"
                  class="px-2 py-1 rounded-md text-xs text-danger hover:bg-danger/10 hover:cursor-pointer transition-colors whitespace-nowrap shrink-0"
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
              class="w-full bg-surface-1 border border-border rounded-md px-2.5 py-1.5 text-sm text-text-primary placeholder:text-text-disabled outline-none transition-colors duration-100 focus:border-border-focus focus:ring-1 focus:ring-accent/30 resize-y min-h-9"
              placeholder="Additional JVM options"
              v-model="server.javaArgs"
            ></textarea>
          </div>
          <admin-icon-picker
            :connection-id="server.id"
            v-model:icon-path="server.iconPath"
            v-model:icon-glyph="server.iconGlyph"
            v-model:icon-color="server.iconColor"
          />
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
            <div class="flex flex-wrap items-center gap-x-5 gap-y-2">
              <label class="flex items-center gap-2 text-sm text-text-primary hover:cursor-pointer select-none">
                <input type="checkbox" class="accent-accent" v-model="server.showConsole" />
                Show console
              </label>
              <label class="flex items-center gap-2 text-sm text-text-primary hover:cursor-pointer select-none">
                <input type="checkbox" class="accent-accent" v-model="server.donotcache" />
                Do not cache
              </label>
            </div>
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
