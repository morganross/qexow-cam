#!/usr/bin/env node
import { runDaemon } from "./daemon.js";

runDaemon().catch((error) => {
  console.error(error?.stack || error?.message || String(error));
  process.exit(1);
});
