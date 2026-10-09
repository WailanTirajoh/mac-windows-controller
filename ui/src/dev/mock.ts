// Backend tiruan untuk melihat UI di browser biasa: `pnpm dev`, lalu buka
// http://localhost:1420/?mock (atau ?mock=incoming untuk dialog kode masuk).
// Kode pairing yang "benar" di mode ini adalah 123456.

import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { ConnectOutcome, Device, PairingRequest, SelfInfo } from "../api";

const self: SelfInfo = {
  fingerprint: "a1".repeat(32),
  id: "a1a1a1a1a1a1a1a1",
  name: "MacBook-Air",
  os: "macos",
  port: 47823,
  acceptIncoming: true,
};

const device = (seed: string, name: string, os: Device["os"], online: boolean, paired: boolean): Device => ({
  fingerprint: seed.repeat(32),
  id: seed.repeat(8),
  name,
  os,
  online,
  paired,
  allowIncoming: paired,
});

let devices: Device[] = [
  device("b2", "PC-Kantor", "windows", true, true),
  device("c3", "DESKTOP-RUMAH", "windows", true, false),
  device("d4", "Mac mini", "macos", false, true),
];
let attemptsLeft = 3;

const find = (fingerprint: unknown) => devices.find((item) => item.fingerprint === fingerprint);

export function installMock(): void {
  mockWindows("main");
  mockIPC(
    (command, payload) => {
      const args = (payload ?? {}) as Record<string, unknown>;
      switch (command) {
        case "get_self":
          return self;
        case "list_devices":
          return devices;
        case "connect_device": {
          const target = find(args.fingerprint);
          if (!target) throw "Perangkat itu sedang tidak terlihat di jaringan.";
          if (!target.online) throw "Tidak bisa terhubung: tidak ada jawaban dari 192.168.1.30:47823";
          attemptsLeft = 3;
          return (
            target.paired
              ? { status: "connected", device: target }
              : { status: "needs_code", device: target, expiresInSecs: 120 }
          ) satisfies ConnectOutcome;
        }
        case "connect_address":
          throw `Tidak ada jawaban dari ${String(args.address)}:47823.`;
        case "submit_pairing_code":
          if (args.code !== "123456") {
            attemptsLeft -= 1;
            throw `kode salah, sisa percobaan: ${attemptsLeft}`;
          }
          devices = devices.map((item) =>
            item.fingerprint === args.fingerprint ? { ...item, paired: true, allowIncoming: true } : item,
          );
          void emit("devices-changed");
          return null;
        case "forget_device":
          devices = devices
            .map((item) => (item.fingerprint === args.fingerprint ? { ...item, paired: false } : item))
            .filter((item) => item.online || item.paired);
          void emit("devices-changed");
          return null;
        case "reject_pairing":
          void emit("pairing-finished", { fingerprint: args.fingerprint, paired: false });
          return null;
        default:
          return null;
      }
    },
    { shouldMockEvents: true },
  );

  if (new URLSearchParams(window.location.search).get("mock") === "incoming") {
    const request: PairingRequest = {
      fingerprint: "e5".repeat(32),
      name: "DESKTOP-RUMAH",
      os: "windows",
      code: "482913",
      expiresInSecs: 120,
    };
    window.setTimeout(() => void emit("pairing-requested", request), 300);
  }
}
