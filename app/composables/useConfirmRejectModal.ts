// Copyright (c) Diridium Technologies Inc. All rights reserved.
// Licensed under the MPL-2.0 License. See LICENSE file in the project root.

import { createVNode, render, type Component } from "vue"
import TrustCertModal from "~/components/TrustCertModal.vue"
import CacheMismatchModal from "~/components/CacheMismatchModal.vue"
import PortMismatchModal from "~/components/PortMismatchModal.vue"
import type { CertInfo } from "~/types"

export function useConfirmRejectModal() {
  // Captured during setup so the imperatively-mounted modal inherits global
  // components (e.g. <icon>) and app plugins, which a bare createVNode lacks.
  const appContext = useNuxtApp().vueApp._context

  // Mount a modal component and resolve on confirm/cancel, so callers can
  // `await` it inline inside the launch flow. A modal may pass a value to
  // confirm (the port-mismatch prompt reports its "don't show again" checkbox);
  // one that emits nothing resolves with payload undefined.
  //
  // Deliberately one implementation. The appContext assignment below is
  // load-bearing and non-obvious, and a second copy of this function would give
  // a future fix to it two places to land and one to be forgotten in.
  function mountModalWithPayload<T>(
    component: Component,
    props: Record<string, unknown>,
  ): Promise<{ confirmed: boolean; payload?: T }> {
    return new Promise((resolve) => {
      const container = document.createElement("div")
      document.body.appendChild(container)
      const cleanup = () => {
        render(null, container)
        container.remove()
      }
      const vnode = createVNode(component, {
        ...props,
        onConfirm: (payload: T) => {
          resolve({ confirmed: true, payload })
          cleanup()
        },
        onCancel: () => {
          resolve({ confirmed: false })
          cleanup()
        },
      })
      vnode.appContext = appContext
      render(vnode, container)
    })
  }

  // For the modals that only answer yes or no.
  const mountModal = (component: Component, props: Record<string, unknown>): Promise<boolean> =>
    mountModalWithPayload(component, props).then((r) => r.confirmed)

  // First connect to a server: neutral "trust this certificate?" prompt.
  const trustCertificate = (cert: CertInfo) => mountModal(TrustCertModal, { mode: "first-use", cert })

  // Pin mismatch: danger prompt showing the previously trusted vs new fingerprint.
  const confirmCertChange = (cert: CertInfo, previousSha256: string) =>
    mountModal(TrustCertModal, { mode: "changed", cert, previousSha256 })

  // Cache/engine collision: confirm overwriting cached jars that differ from
  // what this server sent (usually a wrong/forgotten engine type).
  const confirmCacheMismatch = (info: { engineType: string; version: string; jars: string[] }) =>
    mountModal(CacheMismatchModal, info)

  // The server advertises a port the connection is not configured for, so the
  // administrator will try to log in somewhere else. Resolves the choice plus
  // whether to stop asking for this connection.
  const confirmPortMismatch = (info: {
    configuredPort: number
    advertisedPort: number
    advertisedUrl: string
  }) => mountModalWithPayload<boolean>(PortMismatchModal, info)

  return { trustCertificate, confirmCertChange, confirmCacheMismatch, confirmPortMismatch }
}
