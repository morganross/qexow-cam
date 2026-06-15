const { app, BrowserWindow, ipcMain } = require('electron');
const path = require('node:path');
const fs = require('node:fs/promises');
const os = require('node:os');
const http = require('node:http');
const { execFile } = require('node:child_process');

const rootDir = path.resolve(__dirname, '..', '..');
const appLogPath = path.join(camHome(), 'logs', 'desktop.log');

function camHome() {
  return process.env.CAM_HOME || path.join(os.homedir(), '.qexow-cam-rust');
}

async function fileExists(file) {
  try {
    await fs.access(file);
    return true;
  } catch {
    return false;
  }
}

async function camProgram() {
  const configured = process.env.QEXOW_CAM_EXE || process.env.CAM_EXE;
  if (configured && configured.trim()) return configured.trim();
  const resourceBin = app.isPackaged
    ? path.join(process.resourcesPath, 'bin', process.platform === 'win32' ? 'cam.exe' : 'cam')
    : path.join(rootDir, 'gui', 'resources', 'bin', process.platform === 'win32' ? 'cam.exe' : 'cam');
  const candidates = process.platform === 'win32'
    ? [
        resourceBin,
        path.join(process.env.ProgramFiles || 'C:\\Program Files', 'qexow-cam', 'cam.exe'),
        'C:\\nvm4w\\nodejs\\cam.exe',
        path.join(rootDir, 'target', 'release', 'qexow-cam.exe')
      ]
    : [resourceBin, '/usr/local/bin/cam', path.join(rootDir, 'target', 'release', 'qexow-cam')];
  for (const candidate of candidates) {
    if (await fileExists(candidate)) return candidate;
  }
  return 'cam';
}

async function readJson(file) {
  const text = await fs.readFile(file, 'utf8');
  return JSON.parse(text);
}

async function readToken() {
  const tokenPath = path.join(camHome(), 'secrets', 'local-api-token');
  return (await fs.readFile(tokenPath, 'utf8')).trim();
}

async function readDaemonState() {
  return readJson(path.join(camHome(), 'daemon.json'));
}

async function daemonBaseUrl() {
  const daemon = await readDaemonState();
  return `http://${daemon.bind || '127.0.0.1'}:${daemon.port || 37631}`;
}

function httpRequest(url, { method = 'GET', token, body } = {}) {
  return new Promise((resolve, reject) => {
    const parsed = new URL(url);
    const payload = body === undefined ? undefined : JSON.stringify(body);
    const req = http.request(
      {
        hostname: parsed.hostname,
        port: parsed.port,
        path: `${parsed.pathname}${parsed.search}`,
        method,
        headers: {
          ...(token ? { Authorization: `Bearer ${token}` } : {}),
          ...(payload ? { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) } : {})
        }
      },
      (res) => {
        let data = '';
        res.setEncoding('utf8');
        res.on('data', (chunk) => {
          data += chunk;
        });
        res.on('end', () => {
          const contentType = res.headers['content-type'] || '';
          let parsedBody = data;
          if (contentType.includes('application/json') && data.trim()) {
            try {
              parsedBody = JSON.parse(data);
            } catch (error) {
              return reject(Object.assign(new Error(`invalid JSON from ${url}: ${error.message}`), {
                code: 'CAM_INVALID_JSON',
                url,
                method,
                status: res.statusCode,
                responsePreview: data.slice(0, 500)
              }));
            }
          }
          resolve({ ok: res.statusCode >= 200 && res.statusCode < 300, status: res.statusCode, body: parsedBody });
        });
      }
    );
    req.on('error', (error) => {
      reject(Object.assign(error, { url, method }));
    });
    if (payload) req.write(payload);
    req.end();
  });
}

function runCam(args) {
  return new Promise(async (resolve) => {
    const program = await camProgram();
    execFile(program, args, { windowsHide: true, timeout: 180000 }, (error, stdout, stderr) => {
      resolve({
        ok: !error,
        program,
        code: error && typeof error.code === 'number' ? error.code : 0,
        stdout: stdout.trim(),
        stderr: stderr.trim(),
        error: error ? error.message : null
      });
    });
  });
}

async function api(pathname, options = {}) {
  const base = await daemonBaseUrl();
  const token = pathname.startsWith('/v1/') || pathname === '/shutdown' ? await readToken() : undefined;
  return httpRequest(`${base}${pathname}`, { ...options, token });
}

async function apiWithDiagnostics(request) {
  const method = request.method || 'GET';
  const pathname = request.path;
  const started = Date.now();
  let base = null;
  let daemon = null;
  try {
    daemon = await readDaemonState();
    base = `http://${daemon.bind || '127.0.0.1'}:${daemon.port || 37631}`;
    const token = pathname.startsWith('/v1/') || pathname === '/shutdown' ? await readToken() : undefined;
    const response = await httpRequest(`${base}${pathname}`, { ...request, method, token });
    appendDesktopLog(`api ok method=${method} path=${pathname} status=${response.status} duration_ms=${Date.now() - started}`);
    return response;
  } catch (caught) {
    const diagnostic = {
      ok: false,
      status: 0,
      body: {
        error: 'CAM API request failed',
        detail: caught.message,
        code: caught.code || caught.errno || 'unknown',
        method,
        path: pathname,
        url: caught.url || (base ? `${base}${pathname}` : null),
        cam_home: camHome(),
        daemon_state_path: path.join(camHome(), 'daemon.json'),
        daemon,
        duration_ms: Date.now() - started,
        next_action: 'Check the daemon metric, read the logs panel, then restart the daemon if the process is not healthy.'
      }
    };
    appendDesktopLog(`api failed ${JSON.stringify(redactApiDiagnostic(diagnostic.body))}`);
    return diagnostic;
  }
}

function redactApiDiagnostic(value) {
  return {
    ...value,
    daemon: value.daemon ? {
      bind: value.daemon.bind,
      port: value.daemon.port,
      pid: value.daemon.pid,
      observed_state: value.daemon.observed_state,
      startup_phase: value.daemon.startup_phase,
      started_at: value.daemon.started_at,
      last_heartbeat_at: value.daemon.last_heartbeat_at
    } : null
  };
}

function createWindow() {
  const window = new BrowserWindow({
    width: 1220,
    height: 820,
    minWidth: 980,
    minHeight: 680,
    title: 'Qexow CAM',
    backgroundColor: '#f7f8f4',
    webPreferences: {
      preload: path.join(__dirname, 'preload.cjs'),
      contextIsolation: true,
      nodeIntegration: false
    }
  });

  const devServerUrl = process.env.VITE_DEV_SERVER_URL || process.env.CAM_GUI_DEV_SERVER_URL;
  const shouldUseDevServer = process.env.npm_lifecycle_event === 'dev';
  window.webContents.on('did-fail-load', (_event, errorCode, errorDescription, validatedUrl) => {
    appendDesktopLog(`renderer load failed ${errorCode}: ${errorDescription}; url=${validatedUrl}`);
  });
  window.webContents.on('console-message', (_event, level, message, line, sourceId) => {
    if (level >= 2) appendDesktopLog(`renderer console level=${level} ${sourceId}:${line} ${message}`);
  });
  if (devServerUrl || shouldUseDevServer) {
    window.loadURL(devServerUrl || 'http://127.0.0.1:5173');
  } else {
    window.loadFile(path.join(rootDir, 'dist', 'index.html'));
  }
}

function appendDesktopLog(line) {
  fs.mkdir(path.dirname(appLogPath), { recursive: true })
    .then(() => fs.appendFile(appLogPath, `${new Date().toISOString()} ${line}\n`, 'utf8'))
    .catch(() => {});
}

ipcMain.handle('cam:home', async () => camHome());
ipcMain.handle('cam:daemon-command', async (_event, args) => {
  const result = await runCam(args);
  appendDesktopLog(`command ${result.ok ? 'ok' : 'failed'} program=${result.program} args=${JSON.stringify(args)} code=${result.code} error=${result.error || ''}`);
  return result;
});
ipcMain.handle('cam:status', async () => {
  const home = camHome();
  let daemon = null;
  let health = null;
  let error = null;
  try {
    daemon = await readDaemonState();
    const response = await api('/health');
    health = response.body;
  } catch (caught) {
    error = caught.message;
  }
  return { home, daemon, health, error };
});
ipcMain.handle('cam:api', async (_event, request) => apiWithDiagnostics(request));

app.whenReady().then(createWindow);
app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit();
});
app.on('activate', () => {
  if (BrowserWindow.getAllWindows().length === 0) createWindow();
});
