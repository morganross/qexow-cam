const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('cam', {
  home: () => ipcRenderer.invoke('cam:home'),
  status: () => ipcRenderer.invoke('cam:status'),
  daemonCommand: (args) => ipcRenderer.invoke('cam:daemon-command', args),
  api: (request) => ipcRenderer.invoke('cam:api', request)
});
