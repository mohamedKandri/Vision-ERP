// This device's role, as stored by the Rust side (device.json). Loaded once at start-up.
import { rpc } from "./rpc";

export interface DeviceInfo {
  mode: "host" | "client";
  host: string;
  name: string;
  kind: "pos" | "kds" | "display" | "office";
  register: string;
  port: number;
  shop_name: string;
  paired: boolean;
  addresses: string[];
  online: boolean;
  outbox: number;
  server_error: string | null;
}

let current: DeviceInfo | null = null;

export async function loadDevice(): Promise<DeviceInfo> {
  current = await rpc<DeviceInfo>("device.get");
  return current;
}

export function device(): DeviceInfo | null {
  return current;
}

export function setDevice(d: DeviceInfo) {
  current = d;
}
