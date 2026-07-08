const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("finiteChatDesktop", {
  daemonUrl: () => ipcRenderer.invoke("finitechat:daemon-url"),
  consumePendingInviteUrl: () => ipcRenderer.invoke("finitechat:consume-pending-invite-url"),
  identityStatus: () => ipcRenderer.invoke("finitechat:identity-status"),
  onboardingStatus: () => ipcRenderer.invoke("finitechat:onboarding-status"),
  completeOnboarding: () => ipcRenderer.invoke("finitechat:complete-onboarding"),
  importAccountSecret: (secret) => ipcRenderer.invoke("finitechat:import-account-secret", secret),
  clearAccountSecret: () => ipcRenderer.invoke("finitechat:clear-account-secret"),
  copyText: (text) => ipcRenderer.invoke("finitechat:copy-text", text),
  onInviteUrl: (callback) => {
    const listener = (_event, url) => callback(url);
    ipcRenderer.on("finitechat:invite-url", listener);
    return () => ipcRenderer.removeListener("finitechat:invite-url", listener);
  },
});
