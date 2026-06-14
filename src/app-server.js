import { spawn } from "node:child_process";
import { EventEmitter } from "node:events";

export class AppServerClient extends EventEmitter {
  constructor({ codexPath, log = () => {} }) {
    super();
    this.codexPath = codexPath;
    this.log = log;
    this.child = null;
    this.nextId = 1;
    this.pending = new Map();
    this.buffer = "";
    this.initialized = false;
  }

  async start() {
    if (this.child) return;
    this.child = spawn(this.codexPath, ["app-server", "--listen", "stdio://"], {
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    this.child.stdout.setEncoding("utf8");
    this.child.stdout.on("data", (chunk) => this.#onStdout(chunk));
    this.child.stderr.setEncoding("utf8");
    this.child.stderr.on("data", (chunk) => this.log("app-server.stderr", chunk.trim()));
    this.child.on("error", (error) => {
      this.log("app-server.spawn.error", { error: error.message, codexPath: this.codexPath });
      this.child = null;
      for (const [id, pending] of this.pending) {
        pending.reject(error);
      }
      this.pending.clear();
      this.initialized = false;
    });
    this.child.on("exit", (code, signal) => {
      this.log("app-server.exit", { code, signal });
      this.child = null;
      for (const [id, pending] of this.pending) {
        pending.reject(new Error(`app-server exited before response ${id}`));
      }
      this.pending.clear();
      this.initialized = false;
    });

    const result = await this.request("initialize", {
      clientInfo: { name: "qexow-cam", version: "0.1.0" },
      capabilities: { experimentalApi: true },
    });
    this.notify("initialized");
    this.initialized = true;
    this.log("app-server.initialized", result);
    return result;
  }

  stop() {
    if (this.child) {
      this.child.kill();
      this.child = null;
    }
  }

  request(method, params = undefined, timeoutMs = 30000) {
    if (!this.child && method !== "initialize") {
      return Promise.reject(new Error("app-server is not running"));
    }
    const id = this.nextId++;
    const message = { id, method };
    if (params !== undefined) message.params = params;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`app-server request timed out: ${method}`));
      }, timeoutMs);
      this.pending.set(id, {
        method,
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
      this.#write(message);
    });
  }

  notify(method, params = undefined) {
    const message = { method };
    if (params !== undefined) message.params = params;
    this.#write(message);
  }

  #write(message) {
    if (!this.child?.stdin?.writable) throw new Error("app-server stdin is not writable");
    this.child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  #onStdout(chunk) {
    this.buffer += chunk;
    while (true) {
      const idx = this.buffer.indexOf("\n");
      if (idx < 0) return;
      const line = this.buffer.slice(0, idx).trim();
      this.buffer = this.buffer.slice(idx + 1);
      if (!line) continue;
      let message;
      try {
        message = JSON.parse(line);
      } catch (error) {
        this.log("app-server.parse-error", { line, error: error.message });
        continue;
      }
      this.#onMessage(message);
    }
  }

  #onMessage(message) {
    if (Object.prototype.hasOwnProperty.call(message, "id") && (message.result || message.error)) {
      const pending = this.pending.get(message.id);
      if (!pending) {
        this.log("app-server.unmatched-response", message);
        return;
      }
      this.pending.delete(message.id);
      if (message.error) {
        const error = new Error(message.error.message || `app-server error for ${pending.method}`);
        error.data = message.error;
        pending.reject(error);
      } else {
        pending.resolve(message.result);
      }
      return;
    }

    if (message.method && Object.prototype.hasOwnProperty.call(message, "id")) {
      this.emit("serverRequest", message);
      this.#write({ id: message.id, error: { code: -32601, message: `CAM does not handle ${message.method}` } });
      return;
    }

    if (message.method) {
      this.emit("notification", message);
      if (message.method === "error") {
        this.emit("app-server/error", message.params);
        if (this.listenerCount("error") > 0) this.emit("error", message.params);
        return;
      }
      this.emit(message.method, message.params);
    }
  }
}

export function textInput(text) {
  return [{ type: "text", text, text_elements: [] }];
}
