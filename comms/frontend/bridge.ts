import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface User {
  clientId: number; name: string; volume: number; isCurrentUser: boolean;
  latencyMs: number; isPeerToPeer: boolean; isSpeaking: boolean;
}
export interface Device { id: string; label: string }
export interface Snapshot {
  revision: number; version: string; username: string; server: string;
  status: "disconnected" | "connecting" | "connected" | "disconnecting";
  audioReady: boolean; audioError: string | null; error: string | null;
  users: User[]; inputDevices: Device[]; outputDevices: Device[];
}
export const client = {
  state: () => invoke<Snapshot>("get_state"),
  connect: (username: string, server: string) => invoke<Snapshot>("connect", { username, server }),
  disconnect: () => invoke<Snapshot>("disconnect"),
  volume: (clientId: number, volume: number) => invoke<Snapshot>("set_volume", { clientId, volume }),
  devices: () => invoke<Snapshot>("list_devices"),
  subscribe: (update: (state: Snapshot) => void): Promise<UnlistenFn> =>
    listen<Snapshot>("comms-state", (event) => update(event.payload)),
};
