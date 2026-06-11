import http from "node:http";
import fs from "node:fs";
import { loadConfig, localApiBase } from "./config.js";
import { paths } from "./paths.js";

export function readLocalToken() {
  return fs.readFileSync(paths().localToken, "utf8").trim();
}

export async function apiRequest(method, path, body = undefined) {
  const config = loadConfig();
  const base = new URL(localApiBase(config));
  const payload = body === undefined ? null : JSON.stringify(body);
  const options = {
    hostname: base.hostname,
    port: base.port,
    method,
    path,
    headers: {
      authorization: `Bearer ${readLocalToken()}`,
    },
  };
  if (payload) {
    options.headers["content-type"] = "application/json";
    options.headers["content-length"] = Buffer.byteLength(payload);
  }
  return new Promise((resolve, reject) => {
    const req = http.request(options, (res) => {
      let data = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => { data += chunk; });
      res.on("end", () => {
        let parsed = data;
        try { parsed = data ? JSON.parse(data) : null; } catch {}
        if (res.statusCode >= 400) {
          const error = new Error(parsed?.error || `HTTP ${res.statusCode}`);
          error.response = parsed;
          reject(error);
        } else {
          resolve(parsed);
        }
      });
    });
    req.on("error", reject);
    if (payload) req.write(payload);
    req.end();
  });
}
