const tauri = window.__TAURI__;

if (!tauri) {
  throw new Error("Tauri API is not available.");
}

export const invoke = tauri.core.invoke;
export const listen = tauri.event.listen;

export function command(name, args) {
  return invoke(name, args);
}

/**
 * Resize the main window to the measured content size.
 * @param {{ contentWidth?: number, contentHeight: number }} size
 */
export function fitMainWindow(size) {
  return command("fit_main_window", {
    contentWidth: size?.contentWidth ?? null,
    contentHeight: size.contentHeight,
  });
}

