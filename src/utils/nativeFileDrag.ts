import { invoke } from "@tauri-apps/api/core";
import gtLogoUrl from "../assets/gt-logo.png";

let dragIconPromise: Promise<string> | null = null;

/** Starts an operating-system file drag from the current Tauri window. */
export async function startNativeFileDrag(paths: string[]) {
  if (paths.length === 0) {
    throw new Error("No prepared local files are available to drag");
  }

  await invoke("start_native_file_drag", {
    paths,
    image: await loadDragIcon(),
  });
}

function loadDragIcon() {
  if (!dragIconPromise) {
    dragIconPromise = fetch(gtLogoUrl)
      .then(async (response) => {
        if (!response.ok) {
          throw new Error(`Failed to load drag icon (${response.status})`);
        }
        const bytes = new Uint8Array(await response.arrayBuffer());
        let binary = "";
        const chunkSize = 0x8000;
        for (let offset = 0; offset < bytes.length; offset += chunkSize) {
          binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
        }
        return `data:image/png;base64,${globalThis.btoa(binary)}`;
      })
      .catch((error) => {
        dragIconPromise = null;
        throw error;
      });
  }
  return dragIconPromise;
}
