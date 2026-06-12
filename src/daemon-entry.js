import { runDaemon, showWindowsAlert } from "./daemon.js";

runDaemon().catch((error) => {
  const msg = error?.message || String(error);
  console.error(error?.stack || msg);
  showWindowsAlert("CAM Daemon Fatal Error", msg, "error");
  process.exit(1);
});
