<script setup lang="ts">
import type { Connection } from "~/types"
import { invoke } from "@tauri-apps/api/core"
import { ask } from "@tauri-apps/plugin-dialog"

const route = useRoute()
const connectionId = route.params.id

const isNewConnection = connectionId === "new-connection"

const groups: string[] = await invoke<string[]>("get_all_groups")
const engineTypes: string[] = await invoke<string[]>("get_all_engine_types")

// Where connections are persisted, resolved by the backend so the path shown
// uses native separators. Named next to the password field because telling
// someone their password is stored unencrypted is only actionable if they can
// find the file.
const storePath: string = JSON.parse(
  await invoke<string>("get_launcher_info"),
).store_path

const showFieldHelp = ref(false)

// Kept as data rather than markup so the dialog is a loop and the copy can be
// read in one place. One dialog rather than a marker per field: seven of those
// read as clutter on a panel this dense.
const fieldHelp: { name: string; text: string }[] = [
  { name: "Name", text: "What this connection is called in the list." },
  {
    name: "Address",
    text: `The engine's base URL, or the full webstart.jnlp URL it gives you. Both forms work: https://hostname:port or https://hostname:port/webstart.jnlp`,
  },
  {
    name: "Engine Type",
    text: "Gives each engine fork and version its own cache of jars and extensions.",
  },
  {
    name: "Security",
    text: "The server's certificate is trusted on first connect and pinned afterwards. Forget it to be asked again on the next launch.",
  },
  {
    name: "Java Home",
    text: "The JDK used to launch the administrator. It must include JavaFX. Blank uses JAVA_HOME, or java on PATH.",
  },
  { name: "JVM Arguments", text: "Extra flags for the administrator's JVM." },
  {
    name: "Heap Size",
    text: "Maximum heap for the administrator. Raise it if the administrator runs out of memory.",
  },
  { name: "Icon", text: "The icon shown for this connection and for its console window." },
  {
    name: "Username and Password",
    text: `Optional. Leave both blank and the administrator will prompt you instead. They are stored unencrypted in ${storePath}. The password is also passed to the administrator on its command line, so it appears in the process list while the administrator is running.`,
  },
  { name: "Group", text: "Type a new group or select an existing one. Groups organise the connection list." },
  { name: "Show console", text: "Shows the Administrator's Java console." },
  {
    name: "Do not cache",
    text: "Re-downloads every jar on each launch. Slower, and uses a separate cache directory.",
  },
  { name: "Notes", text: "Free text about this connection. The Notes tab shows a dot when it has any." },
]

const isConnectionEdited = ref<boolean>(false)

// Two tabs, not a general tabbed layout. Notes is the one field that wants room
// and is rarely used, so it lives apart to keep the settings panel short rather
// than to organise anything.
const activeTab = ref<"settings" | "notes">("settings")


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
    <div class="px-5 pt-5 pb-4 flex items-start justify-between gap-2">
      <h1 class="font-semibold text-lg text-text-primary">
        {{ isNewConnection ? "New Connection" : "Edit Connection" }}
      </h1>
      <button
        type="button"
        class="text-text-tertiary hover:text-text-primary hover:cursor-pointer shrink-0"
        aria-label="What do these fields mean?"
        title="What do these fields mean?"
        @click="showFieldHelp = true"
      >
        <icon name="ph:question" class="text-lg" />
      </button>
    </div>

    <!-- Tabs -->
    <div class="flex-none px-5 border-b border-border">
      <div class="flex gap-1 -mb-px">
        <button
          v-for="tab in ([
            { id: 'settings', label: 'Settings' },
            { id: 'notes', label: 'Notes' },
          ] as const)"
          :key="tab.id"
          type="button"
          class="px-3 py-2 text-sm border-b-2 transition-colors hover:cursor-pointer select-none"
          :class="activeTab === tab.id
            ? 'border-accent text-text-primary'
            : 'border-transparent text-text-tertiary hover:text-text-secondary'"
          @click="activeTab = tab.id"
        >
          {{ tab.label }}
          <span
            v-if="tab.id === 'notes' && server.notes"
            class="ml-1.5 inline-block w-1.5 h-1.5 rounded-full bg-accent align-middle"
            title="This connection has notes"
          />
        </button>
      </div>
    </div>

    <!-- Scrollable form area -->
    <div class="flex-1 overflow-y-auto px-5 pt-5 pb-6">
      <form v-show="activeTab === 'settings'" class="grid grid-cols-2 gap-x-8 gap-y-6" @submit.prevent>
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
          <connection-input type="text" label="Java Home" placeholder="/usr/lib/jvm/java-11" note="JavaFX required" v-model="server.javaHome" />
          <!-- An input, not a textarea: sanitize_vm_args splits on whitespace,
               so newlines mean nothing here, and a one-line-tall textarea grew
               a scrollbar no other field in this form has. -->
          <connection-input type="text" label="JVM Arguments" placeholder="Additional JVM options" v-model="server.javaArgs" />
          <connection-input type="text" label="Heap Size" placeholder="512m" v-model="server.heapSize" />
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

      <!-- Field help. One dialog for the whole panel: it can hold a sentence per
           field and stay readable, which a native tooltip cannot, and it costs
           the form no height. -->
      <div
        v-if="showFieldHelp"
        class="absolute inset-0 z-[100] flex items-center justify-center bg-black/50 p-6"
        @click.self="showFieldHelp = false"
      >
        <div class="bg-surface-1 border border-border rounded-lg shadow-overlay w-[34rem] max-h-full flex flex-col">
          <div class="flex items-center justify-between p-5 pb-3">
            <h2 class="font-semibold text-text-primary">Connection fields</h2>
            <button
              type="button"
              class="text-text-tertiary hover:text-text-primary hover:cursor-pointer"
              aria-label="Close"
              @click="showFieldHelp = false"
            >
              <icon name="ph:x" class="text-sm" />
            </button>
          </div>
          <div class="overflow-y-auto px-5 pb-5 space-y-3">
            <div v-for="f in fieldHelp" :key="f.name" class="space-y-0.5">
              <p class="text-sm font-medium text-text-primary select-none">{{ f.name }}</p>
              <!-- select-text: the store path is in here and has to be copyable,
                   which is the whole reason it is named rather than described. -->
              <p class="text-xs text-text-tertiary leading-relaxed select-text break-words">{{ f.text }}</p>
            </div>
          </div>
        </div>
      </div>

      <!-- Notes tab. Fills the panel it was given rather than stretching the
           settings form, which is the whole reason it moved here. -->
      <div v-show="activeTab === 'notes'" class="h-full flex flex-col">
        <textarea
          class="w-full flex-1 min-h-64 bg-surface-1 border border-border rounded-md px-3 py-2 text-sm text-text-primary placeholder:text-text-disabled outline-none transition-colors duration-100 focus:border-border-focus focus:ring-1 focus:ring-accent/30 resize-none"
          placeholder="Notes about this connection"
          v-model="server.notes"
        ></textarea>
      </div>
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
