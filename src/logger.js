import fs from "node:fs";
import path from "node:path";
import { paths } from "./paths.js";

export function getLogsDir() {
  return paths().logsDir;
}

export function getDaemonLogPath() {
  return paths().daemonLog;
}

// Enforce rotation of daemon.log
export function rotateLog(logFilePath, maxSizeBytes = 5 * 1024 * 1024, maxFiles = 5) {
  try {
    if (!fs.existsSync(logFilePath)) return;
    const stats = fs.statSync(logFilePath);
    if (stats.size >= maxSizeBytes) {
      for (let i = maxFiles - 1; i >= 1; i--) {
        const oldFile = `${logFilePath}.${i}`;
        const newFile = `${logFilePath}.${i + 1}`;
        if (fs.existsSync(oldFile)) {
          fs.renameSync(oldFile, newFile);
        }
      }
      fs.renameSync(logFilePath, `${logFilePath}.1`);
    }
  } catch (err) {
    console.error("Log rotation failed:", err);
  }
}

// Enforce retention policy (cleanup files older than maxAgeDays)
export function enforceRetention(logsDir = getLogsDir(), maxAgeDays = 14) {
  try {
    if (!fs.existsSync(logsDir)) return;
    const files = fs.readdirSync(logsDir);
    const now = Date.now();
    const maxAgeMs = maxAgeDays * 24 * 60 * 60 * 1000;

    for (const file of files) {
      // Don't delete active log files
      if (file === "daemon.log" || file === "tray.log") continue;

      const fullPath = path.join(logsDir, file);
      const stat = fs.statSync(fullPath);
      if (now - stat.mtimeMs > maxAgeMs) {
        fs.unlinkSync(fullPath);
      }
    }
  } catch (err) {
    console.error("Log retention cleanup failed:", err);
  }
}

export function logEvent(type, payload = {}) {
  const logFile = getDaemonLogPath();
  const logsDir = getLogsDir();
  
  // Clean up old files on startup/event log calls periodically
  if (Math.random() < 0.05) {
    enforceRetention(logsDir);
  }

  // Rotate check
  rotateLog(logFile);

  const entry = {
    timestamp: new Date().toISOString(),
    type,
    payload,
  };

  try {
    fs.mkdirSync(logsDir, { recursive: true });
    fs.appendFileSync(logFile, JSON.stringify(entry) + "\n", "utf8");
  } catch (err) {
    console.error("Failed to write to daemon.log:", err);
  }
}
