const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

function normalizePath(p) {
  if (!p) return "";
  p = p.replace('\\\\?\\', '');
  p = p.replace(/\\/g, '/');
  return p.toLowerCase().trim();
}

function isInAnyWorkspace(cwd, workspaceRoots) {
  if (!cwd) return false;
  const normCwd = normalizePath(cwd);
  for (const root of workspaceRoots) {
    const normRoot = normalizePath(root);
    if (!normRoot) continue;
    if (normCwd === normRoot || normCwd.startsWith(normRoot + '/')) {
      return true;
    }
  }
  return false;
}

function main() {
  try {
    const home = process.env.HOME || process.env.USERPROFILE;
    if (!home) {
      throw new Error("User home directory (HOME or USERPROFILE env var) is not defined. Fallbacks are disabled.");
    }
    const codexDir = path.join(home, '.codex');
    const globalStatePath = path.join(codexDir, '.codex-global-state.json');
    const sessionIndexPath = path.join(codexDir, 'session_index.jsonl');

    // 1. Read workspace roots
    let workspaceRoots = [];
    if (fs.existsSync(globalStatePath)) {
      try {
        const state = JSON.parse(fs.readFileSync(globalStatePath, 'utf8'));
        const active = state['active-workspace-roots'] || [];
        const saved = state['electron-saved-workspace-roots'] || [];
        workspaceRoots = Array.from(new Set(active.concat(saved)));
      } catch (e) {}
    }

    // 2. Build thread_name map from session_index.jsonl
    const nameMap = {};
    if (fs.existsSync(sessionIndexPath)) {
      try {
        const lines = fs.readFileSync(sessionIndexPath, 'utf8').split(/\r?\n/);
        for (const line of lines) {
          if (!line.trim()) continue;
          try {
            const data = JSON.parse(line);
            if (data.id && data.thread_name) {
              nameMap[data.id] = data.thread_name;
            }
          } catch (e) {}
        }
      } catch (e) {}
    }

    // 3. Find database file
    let dbPath = null;
    try {
      const files = fs.readdirSync(codexDir);
      const dbFiles = files
        .filter(f => f.startsWith('state_') && f.endsWith('.sqlite'))
        .map(f => path.join(codexDir, f));
      if (dbFiles.length > 0) {
        dbFiles.sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs);
        dbPath = dbFiles[0];
      }
    } catch (e) {}

    let rows = [];
    // 4. Try sqlite3 CLI tool
    if (dbPath && fs.existsSync(dbPath)) {
      try {
        const sql = "SELECT id, title, agent_nickname, agent_role, cwd FROM threads WHERE archived = 0 AND thread_source = 'user'";
        const cmd = `sqlite3 -json ${dbPath} "${sql}"`;
        const output = execSync(cmd, { encoding: 'utf8', timeout: 5000 });
        if (output.trim()) {
          rows = JSON.parse(output);
        }
      } catch (e) {
        // If -json is not supported by remote sqlite3
        try {
          const sql = "SELECT id, title, agent_nickname, agent_role, cwd FROM threads WHERE archived = 0 AND thread_source = 'user'";
          const cmd = `sqlite3 -separator '|' ${dbPath} "${sql}"`;
          const output = execSync(cmd, { encoding: 'utf8', timeout: 5000 });
          const lines = output.split(/\r?\n/).filter(Boolean);
          for (const line of lines) {
            const parts = line.split('|');
            if (parts.length >= 5) {
              rows.push({
                id: parts[0],
                title: parts[1],
                agent_nickname: parts[2],
                agent_role: parts[3],
                cwd: parts[4]
              });
            }
          }
        } catch (e2) {}
      }
    }

    // 5. Fallback to hints if no rows found
    if (rows.length === 0 && Object.keys(nameMap).length > 0) {
      let hints = {};
      if (fs.existsSync(globalStatePath)) {
        try {
          const state = JSON.parse(fs.readFileSync(globalStatePath, 'utf8'));
          hints = state['thread-workspace-root-hints'] || {};
        } catch (e) {}
      }
      for (const tid of Object.keys(nameMap)) {
        const cwd = hints[tid] || '';
        rows.push({
          id: tid,
          title: nameMap[tid],
          agent_nickname: '',
          agent_role: '',
          cwd: cwd
        });
      }
    }

    // Filter by workspace roots if available
    if (workspaceRoots.length > 0 && rows.length > 0) {
      rows = rows.filter(r => isInAnyWorkspace(r.cwd, workspaceRoots));
    }

    // Map custom titles
    for (const r of rows) {
      if (nameMap[r.id]) {
        r.title = nameMap[r.id];
      }
    }

    console.log(JSON.stringify({ threads: rows }));
  } catch (err) {
    console.log(JSON.stringify({ error: err.message }));
    process.exit(1);
  }
}

main();
