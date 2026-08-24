import path from "node:path";

const executableName = process.platform === "win32" ? "aes67-desktop.exe" : "aes67-desktop";
const appBinaryPath = path.resolve("src-tauri", "target", "debug", executableName);

export const config = {
  runner: "local",
  specs: ["./e2e/**/*.e2e.mjs"],
  maxInstances: 1,
  capabilities: [
    {
      browserName: "tauri",
      "tauri:options": {
        application: appBinaryPath,
      },
      "wdio:tauriServiceOptions": {
        appBinaryPath,
        driverProvider: "embedded",
        embeddedPort: 4445,
      },
    },
  ],
  services: [
    [
      "@wdio/tauri-service",
      {
        appBinaryPath,
        driverProvider: "embedded",
        embeddedPort: 4445,
      },
    ],
  ],
  framework: "mocha",
  reporters: ["spec"],
  logLevel: "warn",
  waitforTimeout: 10_000,
  connectionRetryTimeout: 90_000,
  connectionRetryCount: 2,
  mochaOpts: {
    ui: "bdd",
    timeout: 60_000,
  },
};
