// Copyright (c) Diridium Technologies Inc. All rights reserved.
// Licensed under the MPL-2.0 License. See LICENSE file in the project root.

import { createVNode, render, type Component } from "vue"
import TrustCertModal from "~/components/TrustCertModal.vue"
import CacheMismatchModal from "~/components/CacheMismatchModal.vue"
import type { CertInfo } from "~/types"

export function useConfirmRejectModal() {
  // Captured during setup so the imperatively-mounted modal inherits global
  // components (e.g. <icon>) and app plugins, which a bare createVNode lacks.
  const appContext = useNuxtApp().vueApp._context

  // Mount a modal component and resolve a boolean on confirm/cancel, so callers
  // can `await` it inline inside the launch flow.
  function mountModal(component: Component, props: Record<string, unknown>): Promise<boolean> {
    return new Promise((resolve) => {
      const container = document.createElement("div")
      document.body.appendChild(container)
      const cleanup = () => {
        render(null, container)
        container.remove()
      }
      const vnode = createVNode(component, {
        ...props,
        onConfirm: () => {
          resolve(true)
          cleanup()
        },
        onCancel: () => {
          resolve(false)
          cleanup()
        },
      })
      vnode.appContext = appContext
      render(vnode, container)
    })
  }

  // First connect to a server: neutral "trust this certificate?" prompt.
  const trustCertificate = (cert: CertInfo) => mountModal(TrustCertModal, { mode: "first-use", cert })

  // Pin mismatch: danger prompt showing the previously trusted vs new fingerprint.
  const confirmCertChange = (cert: CertInfo, previousSha256: string) =>
    mountModal(TrustCertModal, { mode: "changed", cert, previousSha256 })

  // Cache/engine collision: confirm overwriting cached jars that differ from
  // what this server sent (usually a wrong/forgotten engine type).
  const confirmCacheMismatch = (info: { engineType: string; version: string; jars: string[] }) =>
    mountModal(CacheMismatchModal, info)

  return { trustCertificate, confirmCertChange, confirmCacheMismatch }
}
