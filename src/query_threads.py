import sqlite3
import json
import os
import sys
import re
import time
import urllib.parse

def normalize_path(p):
    if not p:
        return ""
    # Strip Win32 long path namespace prefix (e.g. \\?\C:\...)
    p = p.replace('\\\\?\\', '')
    # Normalize slashes to backslashes for uniform comparison on Windows
    p = p.replace('/', '\\')
    return p.lower().strip()

def is_in_any_workspace(cwd, workspace_roots):
    if not cwd or cwd == "outside-of-project":
        return False
    norm_cwd = normalize_path(cwd)
    for root in workspace_roots:
        norm_root = normalize_path(root)
        if not norm_root:
            continue
        if norm_cwd == norm_root or norm_cwd.startswith(norm_root + '\\'):
            return True
    return False

def load_codex_global_state(codex_dir):
    global_state_path = os.path.join(codex_dir, '.codex-global-state.json')
    if not os.path.exists(global_state_path):
        return {}
    try:
        with open(global_state_path, 'r', encoding='utf-8') as f:
            return json.load(f)
    except Exception:
        return {}

def remote_connection_aliases(state):
    aliases = {}
    for conn in state.get('codex-managed-remote-connections', []) or []:
        host_id = conn.get('hostId') or ''
        alias = conn.get('alias') or conn.get('displayName') or host_id.replace('remote-ssh-discovered:', '')
        if host_id and alias:
            aliases[host_id] = alias
    return aliases

def infer_route_metadata(cwd, thread_source, state):
    aliases = remote_connection_aliases(state)
    metadata = {
        "nodeName": os.environ.get("CAM_NODE_NAME") or os.environ.get("COMPUTERNAME") or os.environ.get("HOSTNAME") or "local",
        "sourceHost": "local",
        "hostKind": "local",
        "transport": "local",
        "route": "local",
    }
    if thread_source == "antigravity":
        metadata["transport"] = "antigravity"
        metadata["route"] = "antigravity-local"
        return metadata

    normalized = (cwd or "").replace('\\\\?\\', '').replace('\\', '/')
    for project in state.get('remote-projects', []) or []:
        remote_path = (project.get('remotePath') or '').rstrip('/')
        host_id = project.get('hostId') or ''
        alias = aliases.get(host_id) or host_id.replace('remote-ssh-discovered:', '')
        if remote_path and alias and normalized.startswith(remote_path.rstrip('/') + '/'):
            metadata["nodeName"] = alias
            metadata["sourceHost"] = alias
            metadata["hostKind"] = "remote"
            metadata["transport"] = "codex-managed"
            metadata["route"] = f"codex-managed:{alias}"
            return metadata
    if normalized.startswith('/home/') or normalized.startswith('/root/') or normalized.startswith('/opt/'):
        selected = state.get('selected-remote-host-id') or ''
        alias = aliases.get(selected) or selected.replace('remote-ssh-discovered:', '') or "remote"
        metadata["nodeName"] = alias
        metadata["sourceHost"] = alias
        metadata["hostKind"] = "remote"
        metadata["transport"] = "codex-managed"
        metadata["route"] = f"codex-managed:{alias}"
    return metadata

def decode_varint(buffer, pos):
    val = 0
    shift = 0
    while True:
        b = buffer[pos]
        val |= (b & 0x7f) << shift
        pos += 1
        if not (b & 0x80):
            break
        shift += 7
    return val, pos

def discover_antigravity(workspace_roots, state):
    agy_dir = os.path.expanduser('~/.gemini/antigravity')
    brain_dir = os.path.join(agy_dir, 'brain')
    pb_path = os.path.join(agy_dir, 'agyhub_summaries_proto.pb')
    
    if not os.path.exists(brain_dir):
        return []
        
    # 1. Parse titles from pb file
    titles = {}
    if os.path.exists(pb_path):
        try:
            with open(pb_path, 'rb') as f:
                data = f.read()
            uuid_regex = re.compile(rb'\x0a\x24([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})', re.IGNORECASE)
            for match in uuid_regex.finditer(data):
                uuid_str = match.group(1).decode('utf-8')
                uuid_end = match.end()
                if uuid_end < len(data) and data[uuid_end] == 0x12:
                    submsg_len, pos = decode_varint(data, uuid_end + 1)
                    if pos < len(data) and data[pos] == 0x0a:
                        title_len, title_pos = decode_varint(data, pos + 1)
                        if title_pos + title_len <= len(data):
                            title_bytes = data[title_pos : title_pos + title_len]
                            title_str = title_bytes.decode('utf-8', errors='ignore').strip()
                            if title_str and not any(x in title_str for x in ["{", "}", "\"", ":", "CommandLine", "Cwd"]):
                                titles[uuid_str] = title_str
        except Exception:
            pass

    results = []
    now = time.time()
    
    # 2. Scan folders in brain/
    for d in os.listdir(brain_dir):
        dpath = os.path.join(brain_dir, d)
        if not os.path.isdir(dpath) or len(d) != 36:
            continue
            
        tpath = os.path.join(dpath, ".system_generated", "logs", "transcript.jsonl")
        if not os.path.exists(tpath):
            continue
            
        mtime = os.path.getmtime(tpath)
        
        # Read workspace path from conversations/<uuid>.db
        db_path = os.path.join(agy_dir, 'conversations', f'{d}.db')
        cwd = "outside-of-project"
        if os.path.exists(db_path):
            try:
                # Open read-only connection to avoid database locks
                conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
                cursor = conn.cursor()
                cursor.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='trajectory_metadata_blob'")
                if cursor.fetchone():
                    cursor.execute("SELECT data FROM trajectory_metadata_blob WHERE id='main'")
                    row = cursor.fetchone()
                    if row and row[0]:
                        blob = row[0]
                        paths = re.findall(rb'(?:file:///|[a-zA-Z]:\\)[a-zA-Z0-9_\-\.\:\/\\ \(\)%]+', blob)
                        if paths:
                            decoded = paths[0].decode('utf-8', errors='ignore')
                            if decoded.startswith("file:///"):
                                decoded = decoded[8:]
                            cwd = urllib.parse.unquote(decoded).replace('/', '\\')
                conn.close()
            except Exception:
                pass
                
        # Resolve Title
        title = titles.get(d)
        if not title:
            # Fallback 1: Read CONVERSATION_HISTORY from transcript.jsonl
            try:
                with open(tpath, 'r', encoding='utf-8') as f:
                    for _ in range(15):
                        line = f.readline()
                        if not line:
                            break
                        step = json.loads(line)
                        if step.get("type") == "CONVERSATION_HISTORY" and step.get("content"):
                            m = re.search(rf'## Conversation {d}: (.*?)\n', step["content"], re.IGNORECASE)
                            if m:
                                title = m.group(1).strip()
                                break
            except Exception:
                pass
                
        if not title:
            # Fallback 2: Read first USER_INPUT
            try:
                with open(tpath, 'r', encoding='utf-8') as f:
                    line = f.readline()
                    if line:
                        step = json.loads(line)
                        if step.get("type") == "USER_INPUT" and step.get("content"):
                            content = re.sub(r'<[^>]+>', '', step["content"]).strip()
                            if content:
                                title = content[:40] + ("..." if len(content) > 40 else "")
            except Exception:
                pass
                
        if not title:
            title = "Antigravity Chat"
            
        in_workspace = is_in_any_workspace(cwd, workspace_roots)
        # We consider active if it matches open workspace, or if it was modified in the last 7 days
        is_active = in_workspace or (now - mtime) < 7 * 86400
        
        if is_active:
            results.append({
                "id": d,
                "title": title,
                "agent_nickname": "",
                "agent_role": "",
                "cwd": cwd,
                "thread_source": "antigravity",
                **infer_route_metadata(cwd, "antigravity", state)
            })
            
    return results

def main():
    try:
        # Resolve the SQLite database path
        db_path = os.environ.get("CODEX_DB_PATH")
        if not db_path:
            db_path = os.path.expanduser('~/.codex/state_5.sqlite')
        
        codex_dir = os.path.dirname(db_path)
        session_index_path = os.path.join(codex_dir, 'session_index.jsonl')
        state = load_codex_global_state(codex_dir)
        
        # 1. Read workspace roots
        workspace_roots = []
        if state:
            try:
                active = state.get('active-workspace-roots', [])
                saved = state.get('electron-saved-workspace-roots', [])
                workspace_roots = list(set(active + saved))
            except Exception:
                pass

        # 2. Build thread_name map from session_index.jsonl (keeping the latest name)
        name_map = {}
        if os.path.exists(session_index_path):
            try:
                with open(session_index_path, 'r', encoding='utf-8') as f:
                    for line in f:
                        try:
                            data = json.loads(line)
                            tid = data.get('id')
                            tname = data.get('thread_name')
                            if tid and tname:
                                name_map[tid] = tname
                        except Exception:
                            pass
            except Exception:
                pass

        rows = []
        if os.path.exists(db_path):
            try:
                conn = sqlite3.connect(db_path)
                conn.row_factory = sqlite3.Row
                cursor = conn.cursor()
                
                # Query active user threads
                cursor.execute("""
                    SELECT id, title, agent_nickname, agent_role, cwd, source, thread_source AS codex_thread_source
                    FROM threads 
                    WHERE archived = 0 AND thread_source = 'user'
                """)
                rows = [dict(r) for r in cursor.fetchall()]
                conn.close()
            except Exception:
                pass
        
        # Map custom chat names to titles for Codex threads
        for r in rows:
            latest_name = name_map.get(r['id'])
            if latest_name:
                r['title'] = latest_name
            r['thread_source'] = 'codex'
            r.update(infer_route_metadata(r.get('cwd'), 'codex', state))

        # Discover Antigravity threads
        agy_rows = discover_antigravity(workspace_roots, state)
        rows.extend(agy_rows)
        
        # Filter threads by workspace roots if available
        if workspace_roots:
            filtered_rows = []
            for r in rows:
                # Always include recently active Antigravity threads, or filter Codex threads by workspace
                if r.get('thread_source') == 'antigravity' or is_in_any_workspace(r['cwd'], workspace_roots):
                    filtered_rows.append(r)
            rows = filtered_rows
        
        print(json.dumps({"threads": rows}))
    except Exception as e:
        print(json.dumps({"error": str(e)}))
        sys.exit(1)

if __name__ == '__main__':
    main()
