const tauri = window.__TAURI__;

if (!tauri) {
  throw new Error("Tauri API is not available.");
}

export const invoke = tauri.core.invoke;
export const listen = tauri.event.listen;

export function command(name, args) {
  return invoke(name, args);
}
